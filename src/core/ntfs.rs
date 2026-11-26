use std::collections::{HashMap, HashSet};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::core::device::BlockDevice;
use crate::core::types::{
    FileCategory, FileHealth, FilePayload, FileSystemKind, RecoveredEntry, ScanEvent,
};

const ATTR_STANDARD_INFO: u32 = 0x10;
const ATTR_ATTRIBUTE_LIST: u32 = 0x20;
const ATTR_FILE_NAME: u32 = 0x30;
const ATTR_DATA: u32 = 0x80;
const ATTR_END: u32 = 0xFFFF_FFFF;

const FLAG_IN_USE: u16 = 0x0001;
const FLAG_IS_DIR: u16 = 0x0002;
const FILE_NAME_DOS: u8 = 0x02;
const ATTR_FLAG_COMPRESSED: u16 = 0x00FF;
const FILE_NUMBER_ROOT: u64 = 5;

#[derive(Debug, Clone)]
pub struct NtfsVolumeInfo {
    pub sector_size: u64,
    pub cluster_size: u64,
    pub record_size: usize,
    pub total_clusters: u64,
    pub mft_start_lcn: u64,
}

#[derive(Debug, Clone)]
struct CompactRecord {
    seq: u16,
    in_use: bool,
    is_dir: bool,
    name: String,
    parent_seg: u64,
    parent_seq: u16,
    modified_unix: i64,
    has_data: bool,
}

#[derive(Debug, Clone)]
struct DataStreamSpec {
    size: u64,
    resident: Option<Vec<u8>>,
    runs: Vec<(Option<u64>, u64)>,
    compressed: bool,
    compression_unit_clusters: u64,
}

/// Convert Windows 100-ns FILETIME since 1601-01-01 to Unix seconds.
pub fn filetime_to_unix(ft: u64) -> i64 {
    const EPOCH_DIFF_SECS: i64 = 11_644_473_600;
    if ft == 0 {
        return 0;
    }
    ((ft / 10_000_000) as i64).saturating_sub(EPOCH_DIFF_SECS)
}

/// Apply NTFS Multi-Sector Update Sequence Array (USA) fixup in place.
pub fn apply_usa_fixup(buf: &mut [u8], sector_size: usize) -> bool {
    if buf.len() < 48 || &buf[0..4] != b"FILE" || sector_size < 512 {
        return false;
    }
    let usa_off = u16::from_le_bytes([buf[4], buf[5]]) as usize;
    let usa_cnt = u16::from_le_bytes([buf[6], buf[7]]) as usize;
    if usa_cnt < 2 || (usa_off & 1) != 0 {
        return false;
    }
    let sectors = buf.len() / sector_size;
    if usa_cnt - 1 < sectors || usa_off + usa_cnt * 2 > sector_size {
        return false;
    }
    let sample = [buf[usa_off], buf[usa_off + 1]];
    for i in 0..sectors {
        let end_off = (i + 1) * sector_size - 2;
        if buf[end_off] != sample[0] || buf[end_off + 1] != sample[1] {
            return false;
        }
        let rep_off = usa_off + 2 + i * 2;
        buf[end_off] = buf[rep_off];
        buf[end_off + 1] = buf[rep_off + 1];
    }
    true
}

/// Parse NTFS variable-length signed/unsigned mapping pairs into `(Option<lcn>, cluster_count)`.
pub fn parse_dataruns(data: &[u8]) -> Vec<(Option<u64>, u64)> {
    let mut runs = Vec::new();
    let mut pos = 0usize;
    let mut cur_lcn: i64 = 0;

    while pos < data.len() {
        let header = data[pos];
        pos += 1;
        if header == 0 {
            break;
        }
        let len_size = (header & 0x0F) as usize;
        let off_size = ((header >> 4) & 0x0F) as usize;
        if len_size == 0 || len_size > 8 || off_size > 8 || pos + len_size + off_size > data.len() {
            break;
        }
        let mut count: u64 = 0;
        for i in 0..len_size {
            count |= (data[pos + i] as u64) << (8 * i);
        }
        pos += len_size;

        if off_size == 0 {
            runs.push((None, count));
        } else {
            let mut raw = [0u8; 8];
            let neg = (data[pos + off_size - 1] & 0x80) != 0;
            if neg {
                raw.fill(0xFF);
            }
            raw[..off_size].copy_from_slice(&data[pos..pos + off_size]);
            pos += off_size;
            let delta = i64::from_le_bytes(raw);
            cur_lcn = cur_lcn.saturating_add(delta);
            if cur_lcn < 0 {
                break;
            }
            runs.push((Some(cur_lcn as u64), count));
        }
    }
    runs
}

/// Parse NTFS `$Boot` sector metadata.
pub fn parse_boot_sector(boot: &[u8]) -> io::Result<NtfsVolumeInfo> {
    if boot.len() < 80 || &boot[3..11] != b"NTFS    " {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid NTFS boot sector signature",
        ));
    }
    let sector_size = u16::from_le_bytes([boot[11], boot[12]]) as u64;
    if !sector_size.is_power_of_two() || sector_size < 512 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid NTFS sector size",
        ));
    }
    let spc_raw = boot[13] as i8;
    let spc: u64 = if spc_raw < 0 {
        1u64 << ((-spc_raw) as u32)
    } else {
        (spc_raw as u64).max(1)
    };
    let cluster_size = sector_size * spc;
    let total_sectors = u64::from_le_bytes(boot[40..48].try_into().unwrap());
    let total_clusters = total_sectors / spc.max(1);
    let mft_start_lcn = u64::from_le_bytes(boot[48..56].try_into().unwrap());

    let cpr_raw = boot[64] as i8;
    let record_size: usize = if cpr_raw < 0 {
        1usize << ((-cpr_raw) as u32)
    } else {
        ((cpr_raw as u64) * cluster_size) as usize
    };
    if record_size < 512 || record_size > 16384 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unsupported MFT record size",
        ));
    }

    Ok(NtfsVolumeInfo {
        sector_size,
        cluster_size,
        record_size,
        total_clusters,
        mft_start_lcn,
    })
}

fn read_runs_bytes(
    dev: &dyn BlockDevice,
    runs: &[(Option<u64>, u64)],
    unit_bytes: u64,
    offset: u64,
    len: usize,
) -> io::Result<Vec<u8>> {
    if len == 0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(len);
    let mut voff: u64 = 0;
    let end_off = offset.saturating_add(len as u64);

    for &(lcn_opt, count) in runs {
        let run_bytes = count.saturating_mul(unit_bytes);
        let run_end = voff.saturating_add(run_bytes);
        if run_end <= offset {
            voff = run_end;
            continue;
        }
        if voff >= end_off {
            break;
        }
        let start_in_run = offset.saturating_sub(voff);
        let end_in_run = end_off.min(run_end) - voff;
        let need = (end_in_run - start_in_run) as usize;

        match lcn_opt {
            None => {
                out.resize(out.len() + need, 0);
            }
            Some(lcn) => {
                let disk_off = lcn.saturating_mul(unit_bytes).saturating_add(start_in_run);
                let mut chunk = dev.read_at(disk_off, need)?;
                if chunk.len() < need {
                    chunk.resize(need, 0);
                }
                out.extend_from_slice(&chunk);
            }
        }
        voff = run_end;
    }
    Ok(out)
}

/// Parse lightweight metadata (`CompactRecord`) from an already-fixed-up MFT record buffer.
fn parse_compact_record(buf: &[u8]) -> Option<(u64, CompactRecord)> {
    if buf.len() < 48 || &buf[0..4] != b"FILE" {
        return None;
    }
    let seq = u16::from_le_bytes([buf[16], buf[17]]);
    let first_attr = u16::from_le_bytes([buf[20], buf[21]]) as usize;
    let flags = u16::from_le_bytes([buf[22], buf[23]]);
    let base_ref = u64::from_le_bytes(buf[32..40].try_into().ok()?) & 0x0000_FFFF_FFFF_FFFF;

    let in_use = (flags & FLAG_IN_USE) != 0;
    let is_dir = (flags & FLAG_IS_DIR) != 0;

    let mut off = first_attr;
    let mut best_name: Option<(u8, String, u64, u16)> = None;
    let mut modified_unix = 0i64;
    let mut has_data = false;

    while off + 16 <= buf.len() {
        let attr_type = u32::from_le_bytes(buf[off..off + 4].try_into().ok()?);
        if attr_type == ATTR_END {
            break;
        }
        let rec_len = u32::from_le_bytes(buf[off + 4..off + 8].try_into().ok()?) as usize;
        if rec_len < 16 || off + rec_len > buf.len() {
            break;
        }
        let non_res = buf[off + 8];
        let name_len = buf[off + 9];

        if attr_type == ATTR_STANDARD_INFO && non_res == 0 && off + 24 <= buf.len() {
            let vlen = u32::from_le_bytes(buf[off + 16..off + 20].try_into().ok()?) as usize;
            let voff = u16::from_le_bytes(buf[off + 20..off + 22].try_into().ok()?) as usize;
            let abs = off + voff;
            if vlen >= 24 && abs + 24 <= off + rec_len {
                let mft_chg = u64::from_le_bytes(buf[abs + 16..abs + 24].try_into().ok()?);
                modified_unix = filetime_to_unix(mft_chg);
            }
        } else if attr_type == ATTR_FILE_NAME && non_res == 0 && off + 24 <= buf.len() {
            let vlen = u32::from_le_bytes(buf[off + 16..off + 20].try_into().ok()?) as usize;
            let voff = u16::from_le_bytes(buf[off + 20..off + 22].try_into().ok()?) as usize;
            let abs = off + voff;
            if vlen >= 66 && abs + 66 <= off + rec_len {
                let p_raw = u64::from_le_bytes(buf[abs..abs + 8].try_into().ok()?);
                let parent_seg = p_raw & 0x0000_FFFF_FFFF_FFFF;
                let parent_seq = u16::from_le_bytes([buf[abs + 6], buf[abs + 7]]);
                let fname_len = buf[abs + 64] as usize;
                let ns = buf[abs + 65];
                let byte_len = fname_len * 2;
                if abs + 66 + byte_len <= off + rec_len && fname_len > 0 {
                    let u16s: Vec<u16> = buf[abs + 66..abs + 66 + byte_len]
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect();
                    let fname = String::from_utf16_lossy(&u16s);
                    if fname != "." && fname != ".." {
                        let rank = if ns == FILE_NAME_DOS { 2 } else { ns };
                        let replace = match &best_name {
                            None => true,
                            Some((cur_rank, _, _, _)) => rank < *cur_rank && *cur_rank == 2 || (*cur_rank == 2 && ns != FILE_NAME_DOS),
                        };
                        if replace {
                            best_name = Some((if ns == FILE_NAME_DOS { 2 } else { 0 }, fname, parent_seg, parent_seq));
                        }
                    }
                }
            }
        } else if attr_type == ATTR_DATA && name_len == 0 {
            has_data = true;
        } else if attr_type == ATTR_ATTRIBUTE_LIST {
            // Extension records may hold the default $DATA stream.
            has_data = true;
        }

        off += rec_len;
    }

    let (_, name, parent_seg, parent_seq) = best_name?;
    Some((
        base_ref,
        CompactRecord {
            seq,
            in_use,
            is_dir,
            name,
            parent_seg,
            parent_seq,
            modified_unix,
            has_data,
        },
    ))
}

/// Extract the unnamed `$DATA` stream spec from an MFT record (and optional extension records).
fn parse_unnamed_data_spec(
    buf: &[u8],
    cluster_size: u64,
    total_clusters: u64,
    ext_reader: Option<&dyn Fn(u64) -> Option<Vec<u8>>>,
) -> Option<DataStreamSpec> {
    let first_attr = u16::from_le_bytes([buf[20], buf[21]]) as usize;
    let mut off = first_attr;
    let mut non_res_segments: Vec<(u64, Vec<(Option<u64>, u64)>, u64, bool, u64)> = Vec::new();
    let mut attr_list_entries: Vec<u64> = Vec::new();

    while off + 16 <= buf.len() {
        let attr_type = u32::from_le_bytes(buf[off..off + 4].try_into().ok()?);
        if attr_type == ATTR_END {
            break;
        }
        let rec_len = u32::from_le_bytes(buf[off + 4..off + 8].try_into().ok()?) as usize;
        if rec_len < 16 || off + rec_len > buf.len() {
            break;
        }
        let non_res = buf[off + 8];
        let name_len = buf[off + 9];
        let attr_flags = u16::from_le_bytes([buf[off + 12], buf[off + 13]]);

        if attr_type == ATTR_DATA && name_len == 0 {
            if non_res == 0 && off + 24 <= buf.len() {
                let vlen = u32::from_le_bytes(buf[off + 16..off + 20].try_into().ok()?) as usize;
                let voff = u16::from_le_bytes(buf[off + 20..off + 22].try_into().ok()?) as usize;
                let abs = off + voff;
                if abs + vlen <= off + rec_len {
                    return Some(DataStreamSpec {
                        size: vlen as u64,
                        resident: Some(buf[abs..abs + vlen].to_vec()),
                        runs: Vec::new(),
                        compressed: false,
                        compression_unit_clusters: 0,
                    });
                }
            } else if non_res == 1 && off + 56 <= buf.len() {
                let lowest_vcn = u64::from_le_bytes(buf[off + 16..off + 24].try_into().ok()?);
                let mp_off = u16::from_le_bytes(buf[off + 32..off + 34].try_into().ok()?) as usize;
                let comp_unit = buf[off + 34];
                let data_size = u64::from_le_bytes(buf[off + 48..off + 56].try_into().ok()?);
                if mp_off < rec_len {
                    let runs = parse_dataruns(&buf[off + mp_off..off + rec_len]);
                    let valid = runs.iter().all(|&(lcn_opt, cnt)| {
                        cnt > 0 && lcn_opt.is_none_or(|lcn| lcn.saturating_add(cnt) <= total_clusters)
                    });
                    if valid {
                        let compressed = (attr_flags & ATTR_FLAG_COMPRESSED) != 0 && comp_unit > 0;
                        let cu_clusters = if compressed { 1u64 << comp_unit } else { 0 };
                        non_res_segments.push((lowest_vcn, runs, data_size, compressed, cu_clusters));
                    }
                }
            }
        } else if attr_type == ATTR_ATTRIBUTE_LIST && non_res == 0 && ext_reader.is_some() {
            let vlen = u32::from_le_bytes(buf[off + 16..off + 20].try_into().ok()?) as usize;
            let voff = u16::from_le_bytes(buf[off + 20..off + 22].try_into().ok()?) as usize;
            let abs = off + voff;
            let mut p = abs;
            let end = (abs + vlen).min(off + rec_len);
            while p + 26 <= end {
                let entry_type = u32::from_le_bytes(buf[p..p + 4].try_into().ok()?);
                let entry_len = u16::from_le_bytes(buf[p + 4..p + 6].try_into().ok()?) as usize;
                let entry_name_len = buf[p + 6];
                if entry_len < 26 || p + entry_len > end {
                    break;
                }
                if entry_type == ATTR_DATA && entry_name_len == 0 {
                    let seg = u64::from_le_bytes(buf[p + 16..p + 24].try_into().ok()?)
                        & 0x0000_FFFF_FFFF_FFFF;
                    if seg != 0 && !attr_list_entries.contains(&seg) {
                        attr_list_entries.push(seg);
                    }
                }
                p += entry_len;
            }
        }
        off += rec_len;
    }

    if let Some(reader) = ext_reader {
        for ext_seg in attr_list_entries {
            if let Some(ext_buf) = reader(ext_seg) {
                if let Some(ext_spec) = parse_unnamed_data_spec(&ext_buf, cluster_size, total_clusters, None) {
                    if ext_spec.resident.is_some() && non_res_segments.is_empty() {
                        return Some(ext_spec);
                    }
                    if !ext_spec.runs.is_empty() {
                        non_res_segments.push((
                            0,
                            ext_spec.runs,
                            ext_spec.size,
                            ext_spec.compressed,
                            ext_spec.compression_unit_clusters,
                        ));
                    }
                }
            }
        }
    }

    if non_res_segments.is_empty() {
        return None;
    }

    non_res_segments.sort_by_key(|s| s.0);
    let size = non_res_segments[0].2;
    let compressed = non_res_segments[0].3;
    let cu_clusters = non_res_segments[0].4;
    let mut combined_runs = Vec::new();
    for seg in non_res_segments {
        combined_runs.extend(seg.1);
    }

    Some(DataStreamSpec {
        size,
        resident: None,
        runs: combined_runs,
        compressed,
        compression_unit_clusters: cu_clusters,
    })
}

/// Decompress a single NTFS LZNT1 compression unit.
pub fn lznt1_decompress(input: &[u8], expected_len: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(expected_len);
    let mut pos = 0usize;

    while pos + 2 <= input.len() && out.len() < expected_len {
        let header = u16::from_le_bytes([input[pos], input[pos + 1]]);
        pos += 2;
        if header == 0 {
            break;
        }
        let chunk_len = ((header & 0x0FFF) as usize) + 1;
        let is_compressed = (header & 0x8000) != 0;
        if pos + chunk_len > input.len() {
            break;
        }
        let chunk = &input[pos..pos + chunk_len];
        pos += chunk_len;

        if !is_compressed {
            out.extend_from_slice(chunk);
            continue;
        }

        let chunk_start = out.len();
        let mut cpos = 0usize;
        while cpos < chunk.len() && out.len() < expected_len {
            let flags = chunk[cpos];
            cpos += 1;
            for bit in 0..8 {
                if cpos >= chunk.len() || out.len() >= expected_len {
                    break;
                }
                if (flags & (1 << bit)) == 0 {
                    out.push(chunk[cpos]);
                    cpos += 1;
                } else {
                    if cpos + 2 > chunk.len() {
                        return None;
                    }
                    let token = u16::from_le_bytes([chunk[cpos], chunk[cpos + 1]]);
                    cpos += 2;
                    let rel_pos = out.len() - chunk_start;
                    let mut shift = 0u32;
                    let mut mask = rel_pos.saturating_sub(1);
                    while mask >= 0x10 {
                        mask >>= 1;
                        shift += 1;
                    }
                    let offset_shift = 12 - shift;
                    let len_mask = (1u16 << offset_shift) - 1;
                    let match_len = ((token & len_mask) as usize) + 3;
                    let match_off = ((token >> offset_shift) as usize) + 1;
                    if match_off > out.len() {
                        return None;
                    }
                    for _ in 0..match_len {
                        let b = out[out.len() - match_off];
                        out.push(b);
                        if out.len() >= expected_len {
                            break;
                        }
                    }
                }
            }
        }
    }

    if out.len() > expected_len {
        out.truncate(expected_len);
    }
    Some(out)
}

/// Reconstruct relative path for a deleted NTFS MFT record, handling deleted parents (`seq + 1`).
fn reconstruct_path(seg: u64, records: &HashMap<u64, CompactRecord>) -> String {
    let Some(rec) = records.get(&seg) else {
        return String::new();
    };
    let mut parts = vec![rec.name.clone()];
    let mut cur = rec.parent_seg;
    let mut exp_seq = rec.parent_seq;
    let mut seen = HashSet::new();
    seen.insert(seg);

    for _ in 0..64 {
        if cur == FILE_NUMBER_ROOT {
            parts.reverse();
            return parts.join("/");
        }
        if !seen.insert(cur) {
            break;
        }
        let Some(parent) = records.get(&cur) else {
            break;
        };
        if !parent.is_dir {
            break;
        }
        let seq_ok = if parent.in_use {
            parent.seq == exp_seq
        } else {
            parent.seq == exp_seq || parent.seq == exp_seq.wrapping_add(1)
        };
        if !seq_ok {
            break;
        }
        parts.push(parent.name.clone());
        cur = parent.parent_seg;
        exp_seq = parent.parent_seq;
    }

    parts.push("_Orphaned".to_string());
    parts.reverse();
    parts.join("/")
}

/// Scan an NTFS volume/partition for recoverable deleted files.
pub fn scan_ntfs(
    dev: &Arc<dyn BlockDevice>,
    cancel: &AtomicBool,
    progress_cb: &mut dyn FnMut(ScanEvent),
) -> io::Result<(Vec<RecoveredEntry>, Vec<String>)> {
    let mut warnings = Vec::new();
    let boot = dev.read_at(0, 512)?;
    let info = parse_boot_sector(&boot)?;

    let mft_offset = info.mft_start_lcn.saturating_mul(info.cluster_size);
    let mut mft0 = dev.read_at(mft_offset, info.record_size)?;
    if !apply_usa_fixup(&mut mft0, info.sector_size as usize) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid $MFT record 0 fixup",
        ));
    }

    let mft_spec = parse_unnamed_data_spec(&mft0, info.cluster_size, info.total_clusters, None)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Cannot locate $MFT $DATA runs"))?;
    let mut mft_runs = mft_spec.runs;
    let mft_size = mft_spec.size;

    // Resolve any $ATTRIBUTE_LIST extensions for heavily fragmented $MFT
    let first_attr = u16::from_le_bytes([mft0[20], mft0[21]]) as usize;
    let mut off = first_attr;
    let mut extra_mft_segs = Vec::new();
    while off + 16 <= mft0.len() {
        let attr_type = u32::from_le_bytes(mft0[off..off + 4].try_into().unwrap());
        if attr_type == ATTR_END {
            break;
        }
        let rec_len = u32::from_le_bytes(mft0[off + 4..off + 8].try_into().unwrap()) as usize;
        if rec_len < 16 || off + rec_len > mft0.len() {
            break;
        }
        if attr_type == ATTR_ATTRIBUTE_LIST && mft0[off + 8] == 0 {
            let vlen = u32::from_le_bytes(mft0[off + 16..off + 20].try_into().unwrap()) as usize;
            let voff = u16::from_le_bytes(mft0[off + 20..off + 22].try_into().unwrap()) as usize;
            let mut p = off + voff;
            let end = (p + vlen).min(off + rec_len);
            while p + 26 <= end {
                let etype = u32::from_le_bytes(mft0[p..p + 4].try_into().unwrap());
                let elen = u16::from_le_bytes(mft0[p + 4..p + 6].try_into().unwrap()) as usize;
                if elen < 26 || p + elen > end {
                    break;
                }
                if etype == ATTR_DATA {
                    let s = u64::from_le_bytes(mft0[p + 16..p + 24].try_into().unwrap())
                        & 0x0000_FFFF_FFFF_FFFF;
                    if s != 0 && !extra_mft_segs.contains(&s) {
                        extra_mft_segs.push(s);
                    }
                }
                p += elen;
            }
        }
        off += rec_len;
    }
    for s in extra_mft_segs {
        let byte_off = s * info.record_size as u64;
        if let Ok(mut ext_buf) =
            read_runs_bytes(dev.as_ref(), &mft_runs, info.cluster_size, byte_off, info.record_size)
        {
            if apply_usa_fixup(&mut ext_buf, info.sector_size as usize) {
                if let Some(ext_spec) =
                    parse_unnamed_data_spec(&ext_buf, info.cluster_size, info.total_clusters, None)
                {
                    mft_runs.extend(ext_spec.runs);
                }
            }
        }
    }

    let total_segments = mft_size / info.record_size as u64;
    let mut records: HashMap<u64, CompactRecord> = HashMap::with_capacity((total_segments as usize).min(2_000_000));
    let mut candidate_segs: Vec<u64> = Vec::new();

    // Sequential 2 MB buffered MFT pass
    let batch_records = 2048usize;
    let batch_bytes = batch_records * info.record_size;
    let mut seg = 0u64;

    while seg < total_segments {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let byte_off = seg * info.record_size as u64;
        let to_read = batch_bytes.min((mft_size - byte_off) as usize);
        let chunk = read_runs_bytes(dev.as_ref(), &mft_runs, info.cluster_size, byte_off, to_read)?;
        if chunk.is_empty() {
            break;
        }
        let count = chunk.len() / info.record_size;
        for i in 0..count {
            let cur_seg = seg + i as u64;
            let mut rec_buf = chunk[i * info.record_size..(i + 1) * info.record_size].to_vec();
            if !apply_usa_fixup(&mut rec_buf, info.sector_size as usize) {
                continue;
            }
            if let Some((base_ref, compact)) = parse_compact_record(&rec_buf) {
                if base_ref != 0 {
                    continue;
                }
                let deleted_file = !compact.in_use && !compact.is_dir && compact.has_data;
                if deleted_file {
                    candidate_segs.push(cur_seg);
                }
                records.insert(cur_seg, compact);
            }
        }
        seg += count as u64;
        if seg % 65536 == 0 || seg >= total_segments {
            let frac = (seg as f32 / total_segments.max(1) as f32) * 0.6;
            progress_cb(ScanEvent::Progress {
                stage: "MFT".to_string(),
                fraction: frac,
                found_count: candidate_segs.len(),
                detail: format!("{}/{}", seg, total_segments),
            });
        }
    }

    let read_mft_seg = |s: u64| -> Option<Vec<u8>> {
        let off = s * info.record_size as u64;
        let mut buf =
            read_runs_bytes(dev.as_ref(), &mft_runs, info.cluster_size, off, info.record_size).ok()?;
        if apply_usa_fixup(&mut buf, info.sector_size as usize) {
            Some(buf)
        } else {
            None
        }
    };

    let mut entries = Vec::with_capacity(candidate_segs.len());
    let max_sane_size = dev.total_size().max(info.total_clusters * info.cluster_size);
    let total_cands = candidate_segs.len().max(1);

    for (idx, &s_id) in candidate_segs.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Some(rec_buf) = read_mft_seg(s_id) else {
            continue;
        };
        let Some(spec) =
            parse_unnamed_data_spec(&rec_buf, info.cluster_size, info.total_clusters, Some(&read_mft_seg))
        else {
            continue;
        };
        if spec.size > max_sane_size {
            continue;
        }

        let rel_path = reconstruct_path(s_id, &records);
        if rel_path.is_empty() {
            continue;
        }
        let meta = &records[&s_id];
        let ext = rel_path
            .rsplit('.')
            .next()
            .filter(|e| !e.contains('/'))
            .unwrap_or("")
            .to_ascii_lowercase();
        let folder_path = rel_path
            .rsplit_once('/')
            .map(|(p, _)| p.to_string())
            .unwrap_or_default();

        let (health, payload) = if spec.size == 0 {
            (FileHealth::ZeroByte, FilePayload::Empty)
        } else if let Some(inline) = spec.resident {
            let h = if inline.iter().all(|&b| b == 0) {
                FileHealth::LikelyTrimmedZero
            } else {
                FileHealth::Resident
            };
            (h, FilePayload::Inline(inline))
        } else {
            // Fast 4 KB header check to detect SSD TRIM-zeroed files upfront
            let health = if spec.runs.is_empty() {
                FileHealth::PartialOrCorrupted
            } else {
                let peek_len = (spec.size as usize).min(4096);
                let first_bytes = if spec.compressed {
                    Vec::new()
                } else {
                    read_runs_bytes(dev.as_ref(), &spec.runs, info.cluster_size, 0, peek_len)
                        .unwrap_or_default()
                };
                if !spec.compressed && !first_bytes.is_empty() && first_bytes.iter().all(|&b| b == 0) {
                    FileHealth::LikelyTrimmedZero
                } else {
                    FileHealth::Intact
                }
            };
            (
                health,
                FilePayload::Extents {
                    runs: spec.runs,
                    unit_bytes: info.cluster_size,
                    file_size: spec.size,
                    compressed: spec.compressed,
                    compression_unit_bytes: spec.compression_unit_clusters * info.cluster_size,
                },
            )
        };

        entries.push(RecoveredEntry {
            id: s_id,
            name: meta.name.clone(),
            rel_path_lower: rel_path.to_ascii_lowercase(),
            rel_path,
            folder_path,
            category: FileCategory::from_ext(&ext),
            ext,
            size: spec.size,
            modified_unix: meta.modified_unix,
            fs_kind: FileSystemKind::Ntfs,
            health,
            source_desc: format!("MFT #{}", s_id),
            payload,
        });

        if (idx + 1) % 2000 == 0 || idx + 1 == candidate_segs.len() {
            let frac = 0.6 + ((idx + 1) as f32 / total_cands as f32) * 0.4;
            progress_cb(ScanEvent::Progress {
                stage: "NTFS Tree".to_string(),
                fraction: frac,
                found_count: entries.len(),
                detail: format!("{}/{}", idx + 1, candidate_segs.len()),
            });
        }
    }

    if entries.is_empty() {
        warnings.push("No deleted $MFT records with intact data streams were found.".to_string());
    }

    Ok((entries, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MemDevice(Vec<u8>);
    impl BlockDevice for MemDevice {
        fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
            let off = offset as usize;
            if off >= self.0.len() {
                return Ok(Vec::new());
            }
            let end = (off + len).min(self.0.len());
            Ok(self.0[off..end].to_vec())
        }
        fn total_size(&self) -> u64 {
            self.0.len() as u64
        }
        fn sector_size(&self) -> u32 {
            512
        }
    }

    fn set_usa(rec: &mut [u8], seq_val: u16) {
        let usa_off = 48usize;
        rec[4..6].copy_from_slice(&(usa_off as u16).to_le_bytes());
        rec[6..8].copy_from_slice(&3u16.to_le_bytes());
        rec[usa_off..usa_off + 2].copy_from_slice(&seq_val.to_le_bytes());
        let end1 = [rec[510], rec[511]];
        let end2 = [rec[1022], rec[1023]];
        rec[usa_off + 2..usa_off + 4].copy_from_slice(&end1);
        rec[usa_off + 4..usa_off + 6].copy_from_slice(&end2);
        rec[510..512].copy_from_slice(&seq_val.to_le_bytes());
        rec[1022..1024].copy_from_slice(&seq_val.to_le_bytes());
    }

    fn make_mft_record(seq: u16, flags: u16, attrs: &[u8]) -> Vec<u8> {
        let mut rec = vec![0u8; 1024];
        rec[0..4].copy_from_slice(b"FILE");
        rec[16..18].copy_from_slice(&seq.to_le_bytes());
        let first_attr = 56u16;
        rec[20..22].copy_from_slice(&first_attr.to_le_bytes());
        rec[22..24].copy_from_slice(&flags.to_le_bytes());
        let end_off = first_attr as usize + attrs.len();
        rec[first_attr as usize..end_off].copy_from_slice(attrs);
        rec[end_off..end_off + 4].copy_from_slice(&ATTR_END.to_le_bytes());
        set_usa(&mut rec, 1);
        rec
    }

    fn make_res_attr(attr_type: u32, payload: &[u8]) -> Vec<u8> {
        let header_len = 24usize;
        let total = (header_len + payload.len() + 7) & !7;
        let mut out = vec![0u8; total];
        out[0..4].copy_from_slice(&attr_type.to_le_bytes());
        out[4..8].copy_from_slice(&(total as u32).to_le_bytes());
        out[8] = 0;
        out[16..20].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        out[20..22].copy_from_slice(&(header_len as u16).to_le_bytes());
        out[header_len..header_len + payload.len()].copy_from_slice(payload);
        out
    }

    fn make_fn_attr(name: &str, parent_seg: u64, parent_seq: u16) -> Vec<u8> {
        let u16s: Vec<u16> = name.encode_utf16().collect();
        let mut body = vec![0u8; 66 + u16s.len() * 2];
        let p_ref = (parent_seg & 0x0000_FFFF_FFFF_FFFF) | ((parent_seq as u64) << 48);
        body[0..8].copy_from_slice(&p_ref.to_le_bytes());
        body[64] = u16s.len() as u8;
        body[65] = 1; // Win32
        for (i, c) in u16s.iter().enumerate() {
            body[66 + i * 2..66 + i * 2 + 2].copy_from_slice(&c.to_le_bytes());
        }
        make_res_attr(ATTR_FILE_NAME, &body)
    }

    fn make_nonres_data(runs_bytes: &[u8], file_size: u64) -> Vec<u8> {
        let mp_off = 64usize;
        let total = (mp_off + runs_bytes.len() + 1 + 7) & !7;
        let mut out = vec![0u8; total];
        out[0..4].copy_from_slice(&ATTR_DATA.to_le_bytes());
        out[4..8].copy_from_slice(&(total as u32).to_le_bytes());
        out[8] = 1;
        out[32..34].copy_from_slice(&(mp_off as u16).to_le_bytes());
        out[48..56].copy_from_slice(&file_size.to_le_bytes());
        out[mp_off..mp_off + runs_bytes.len()].copy_from_slice(runs_bytes);
        out
    }

    #[test]
    fn test_dataruns_signed_deltas_and_sparse() {
        // Run 1: count=2 (1B), offset=+10 (1B) -> header 0x11
        // Run 2: sparse count=3 -> header 0x01
        // Run 3: count=1, offset=-4 (0xFC) -> header 0x11
        let data = [0x11, 2, 10, 0x01, 3, 0x11, 1, 0xFC, 0x00];
        let runs = parse_dataruns(&data);
        assert_eq!(runs, vec![(Some(10), 2), (None, 3), (Some(6), 1)]);
    }

    #[test]
    fn test_synthetic_ntfs_recovers_deleted_tree_with_seq_increment() {
        let cluster_size = 4096usize;
        let total_clusters = 64usize;
        let mut img = vec![0u8; total_clusters * cluster_size];

        // Boot sector
        img[3..11].copy_from_slice(b"NTFS    ");
        img[11..13].copy_from_slice(&512u16.to_le_bytes());
        img[13] = 8; // 8 sectors * 512 = 4096 cluster
        img[40..48].copy_from_slice(&((total_clusters * 8) as u64).to_le_bytes());
        img[48..56].copy_from_slice(&2u64.to_le_bytes()); // MFT at cluster 2
        img[64] = (-10i8) as u8; // 2^10 = 1024 byte records

        // MFT starts at cluster 2 (8 records = 8192 bytes = 2 clusters)
        let mft_base = 2 * cluster_size;
        let mft0 = make_mft_record(1, FLAG_IN_USE, &make_nonres_data(&[0x11, 2, 2], 8 * 1024));
        img[mft_base..mft_base + 1024].copy_from_slice(&mft0);

        // Record 5: Root dir (in use)
        let rec5 = make_mft_record(5, FLAG_IN_USE | FLAG_IS_DIR, &make_fn_attr(".", 5, 5));
        img[mft_base + 5 * 1024..mft_base + 6 * 1024].copy_from_slice(&rec5);

        // Record 6: Deleted folder "Telegram" (deleted -> seq incremented from 3 to 4!)
        let rec6 = make_mft_record(4, FLAG_IS_DIR, &make_fn_attr("Telegram", 5, 5));
        img[mft_base + 6 * 1024..mft_base + 7 * 1024].copy_from_slice(&rec6);

        // Record 7: Deleted non-resident file "notes.txt" under deleted folder #6 (parent_seq=3)
        let payload = b"Hello OpenRecover NTFS!";
        img[10 * cluster_size..10 * cluster_size + payload.len()].copy_from_slice(payload);
        let mut attrs7 = make_fn_attr("notes.txt", 6, 3);
        attrs7.extend_from_slice(&make_nonres_data(&[0x11, 1, 10], payload.len() as u64));
        let rec7 = make_mft_record(2, 0, &attrs7);
        img[mft_base + 7 * 1024..mft_base + 8 * 1024].copy_from_slice(&rec7);

        let dev: Arc<dyn BlockDevice> = Arc::new(MemDevice(img));
        let cancel = AtomicBool::new(false);
        let (entries, _) = scan_ntfs(&dev, &cancel, &mut |_| {}).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].rel_path, "Telegram/notes.txt");
        assert_eq!(entries[0].health, FileHealth::Intact);
        assert_eq!(entries[0].size, payload.len() as u64);
        let recovered = crate::core::recover::read_entry_bytes(dev.as_ref(), &entries[0], 1024).unwrap();
        assert_eq!(recovered, payload);
    }
}
