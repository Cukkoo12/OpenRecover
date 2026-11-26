use std::collections::HashSet;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::core::device::BlockDevice;
use crate::core::types::{
    FileCategory, FileHealth, FilePayload, FileSystemKind, RecoveredEntry, ScanEvent,
};

fn read_fat32_entry(dev: &dyn BlockDevice, fat_offset: u64, cluster: u32) -> Option<u32> {
    let off = fat_offset + (cluster as u64) * 4;
    let buf = dev.read_at(off, 4).ok()?;
    if buf.len() < 4 {
        return None;
    }
    Some(u32::from_le_bytes(buf[..4].try_into().unwrap()) & 0x0FFF_FFFF)
}

fn follow_fat32_chain(
    dev: &dyn BlockDevice,
    fat_offset: u64,
    start_cluster: u32,
    max_clusters: u64,
    limit_clusters: usize,
) -> Vec<u32> {
    let mut chain = Vec::new();
    if start_cluster < 2 || (start_cluster as u64) >= max_clusters + 2 {
        return chain;
    }
    let mut seen = HashSet::new();
    let mut cur = start_cluster;
    while chain.len() < limit_clusters && seen.insert(cur) {
        chain.push(cur);
        match read_fat32_entry(dev, fat_offset, cur) {
            Some(next) if next >= 2 && (next as u64) < max_clusters + 2 => cur = next,
            _ => break,
        }
    }
    chain
}

fn clusters_to_byte_runs(
    clusters: &[u32],
    data_offset: u64,
    cluster_size: u64,
) -> Vec<(Option<u64>, u64)> {
    let mut runs: Vec<(Option<u64>, u64)> = Vec::new();
    for &c in clusters {
        if c < 2 {
            continue;
        }
        let byte_off = data_offset + ((c - 2) as u64) * cluster_size;
        if let Some((Some(last_off), byte_len)) = runs.last_mut() {
            if *last_off + *byte_len == byte_off {
                *byte_len += cluster_size;
                continue;
            }
        }
        runs.push((Some(byte_off), cluster_size));
    }
    runs
}

fn dos_time_to_unix(dos_date: u16, dos_time: u16) -> i64 {
    if dos_date == 0 {
        return 0;
    }
    let year = ((dos_date >> 9) & 0x7F) as i64 + 1980;
    let month = ((dos_date >> 5) & 0x0F) as i64;
    let day = (dos_date & 0x1F) as i64;
    let hour = ((dos_time >> 11) & 0x1F) as i64;
    let min = ((dos_time >> 5) & 0x3F) as i64;
    let sec = ((dos_time & 0x1F) * 2) as i64;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return 0;
    }
    // Simple days-since-epoch calculation (accurate for 1980..2099)
    let mut days = 0i64;
    for y in 1970..year {
        let leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
        days += if leap { 366 } else { 365 };
    }
    let leap_cur = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let mdays = [
        31,
        if leap_cur { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    for m in 1..month {
        days += mdays[(m - 1) as usize];
    }
    days += day - 1;
    days * 86400 + hour * 3600 + min * 60 + sec
}

// ==================================== exFAT ====================================

pub fn scan_exfat(
    dev: &Arc<dyn BlockDevice>,
    cancel: &AtomicBool,
    progress_cb: &mut dyn FnMut(ScanEvent),
) -> io::Result<(Vec<RecoveredEntry>, Vec<String>)> {
    let boot = dev.read_at(0, 512)?;
    if boot.len() < 512 || &boot[3..11] != b"EXFAT   " {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid exFAT boot sector",
        ));
    }
    let sec_shift = boot[108].clamp(9, 12);
    let spc_shift = boot[109].min(25);
    let sector_size = 1u64 << sec_shift;
    let cluster_size = sector_size << spc_shift;
    let fat_offset = u32::from_le_bytes(boot[80..84].try_into().unwrap()) as u64 * sector_size;
    let cluster_heap_off =
        u32::from_le_bytes(boot[88..92].try_into().unwrap()) as u64 * sector_size;
    let cluster_count = u32::from_le_bytes(boot[92..96].try_into().unwrap()) as u64;
    let root_cluster = u32::from_le_bytes(boot[96..100].try_into().unwrap());

    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let mut visited_dirs: HashSet<u32> = HashSet::new();
    // Queue: (cluster, is_parent_deleted, no_fat_chain, dir_size_opt, dir_rel_path)
    let mut queue: Vec<(u32, bool, bool, Option<u64>, String)> =
        vec![(root_cluster, false, false, None, String::new())];
    let mut next_id = 1u64;

    while let Some((dir_clus, parent_deleted, dir_nofat, dir_len_opt, prefix)) = queue.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if dir_clus < 2 || (dir_clus as u64) >= cluster_count + 2 || !visited_dirs.insert(dir_clus) {
            continue;
        }

        let dir_clusters = if dir_nofat {
            let cnt = dir_len_opt
                .map(|l| ((l + cluster_size - 1) / cluster_size).max(1) as usize)
                .unwrap_or(1)
                .min(512);
            (0..cnt).map(|i| dir_clus + i as u32).collect()
        } else {
            let c = follow_fat32_chain(dev.as_ref(), fat_offset, dir_clus, cluster_count, 512);
            if c.is_empty() {
                vec![dir_clus]
            } else {
                c
            }
        };

        let mut dir_bytes = Vec::new();
        for c in dir_clusters {
            let off = cluster_heap_off + ((c - 2) as u64) * cluster_size;
            if let Ok(mut b) = dev.read_at(off, cluster_size as usize) {
                dir_bytes.append(&mut b);
            }
        }

        let num_entries = dir_bytes.len() / 32;
        let mut idx = 0usize;
        while idx < num_entries {
            let e = &dir_bytes[idx * 32..(idx + 1) * 32];
            let etype = e[0];
            if etype == 0x00 {
                break;
            }
            if etype == 0x85 || etype == 0x05 {
                let entry_deleted = etype == 0x05 || parent_deleted;
                let sec_count = e[1] as usize;
                let file_attrs = u16::from_le_bytes([e[4], e[5]]);
                let is_dir = (file_attrs & 0x0010) != 0;
                let mod_ts = u32::from_le_bytes(e[12..16].try_into().unwrap());
                let modified_unix = dos_time_to_unix((mod_ts >> 16) as u16, (mod_ts & 0xFFFF) as u16);

                if sec_count >= 2 && idx + sec_count < num_entries {
                    let s_ext = &dir_bytes[(idx + 1) * 32..(idx + 2) * 32];
                    if s_ext[0] == 0xC0 || s_ext[0] == 0x40 {
                        let no_fat_chain = (s_ext[1] & 0x02) != 0;
                        let name_len = s_ext[3] as usize;
                        let first_cluster = u32::from_le_bytes(s_ext[20..24].try_into().unwrap());
                        let data_len = u64::from_le_bytes(s_ext[24..32].try_into().unwrap());

                        let mut utf16_chars = Vec::with_capacity(name_len);
                        for k in 2..=sec_count {
                            let ne = &dir_bytes[(idx + k) * 32..(idx + k + 1) * 32];
                            if ne[0] != 0xC1 && ne[0] != 0x41 {
                                break;
                            }
                            for pair in ne[2..32].chunks_exact(2) {
                                if utf16_chars.len() >= name_len {
                                    break;
                                }
                                utf16_chars.push(u16::from_le_bytes([pair[0], pair[1]]));
                            }
                        }
                        let name = String::from_utf16_lossy(&utf16_chars);
                        if !name.is_empty() && name != "." && name != ".." {
                            let rel_path = if prefix.is_empty() {
                                name.clone()
                            } else {
                                format!("{}/{}", prefix, name)
                            };
                            if is_dir {
                                if first_cluster >= 2 {
                                    queue.push((
                                        first_cluster,
                                        entry_deleted,
                                        no_fat_chain,
                                        Some(data_len),
                                        rel_path,
                                    ));
                                }
                            } else if entry_deleted {
                                let ext = name
                                    .rsplit('.')
                                    .next()
                                    .filter(|x| *x != name)
                                    .unwrap_or("")
                                    .to_ascii_lowercase();
                                let folder_path = prefix.clone();
                                let (health, payload) = build_fat_payload(
                                    dev.as_ref(),
                                    fat_offset,
                                    cluster_heap_off,
                                    cluster_size,
                                    cluster_count,
                                    first_cluster,
                                    data_len,
                                    no_fat_chain || entry_deleted,
                                );
                                entries.push(RecoveredEntry {
                                    id: next_id,
                                    name,
                                    rel_path_lower: rel_path.to_ascii_lowercase(),
                                    rel_path,
                                    folder_path,
                                    category: FileCategory::from_ext(&ext),
                                    ext,
                                    size: data_len,
                                    modified_unix,
                                    fs_kind: FileSystemKind::ExFat,
                                    health,
                                    source_desc: format!("exFAT clus #{}", first_cluster),
                                    payload,
                                });
                                next_id += 1;
                            }
                        }
                    }
                }
                idx += 1 + sec_count;
                continue;
            }
            idx += 1;
        }

        progress_cb(ScanEvent::Progress {
            stage: "exFAT".to_string(),
            fraction: 0.5,
            found_count: entries.len(),
            detail: format!("{} dirs", visited_dirs.len()),
        });
    }

    if entries.is_empty() {
        warnings.push("No deleted exFAT directory entries found; try Deep Carving.".to_string());
    }
    Ok((entries, warnings))
}

// ==================================== FAT32 ====================================

fn decode_sfn(raw: &[u8]) -> String {
    let mut name_raw = raw[0..8].to_vec();
    if name_raw[0] == 0xE5 {
        name_raw[0] = b'_';
    }
    let stem = String::from_utf8_lossy(&name_raw).trim_end().to_string();
    let ext = String::from_utf8_lossy(&raw[8..11]).trim_end().to_string();
    if ext.is_empty() {
        stem
    } else {
        format!("{}.{}", stem, ext)
    }
}

fn lfn_chunk_chars(entry: &[u8]) -> Vec<u16> {
    let mut chars = Vec::with_capacity(13);
    for off in [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30] {
        let code = u16::from_le_bytes([entry[off], entry[off + 1]]);
        if code == 0x0000 || code == 0xFFFF {
            break;
        }
        chars.push(code);
    }
    chars
}

pub fn scan_fat32(
    dev: &Arc<dyn BlockDevice>,
    cancel: &AtomicBool,
    progress_cb: &mut dyn FnMut(ScanEvent),
) -> io::Result<(Vec<RecoveredEntry>, Vec<String>)> {
    let boot = dev.read_at(0, 512)?;
    if boot.len() < 512 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "FAT32 boot too short"));
    }
    let bytes_per_sec = u16::from_le_bytes([boot[11], boot[12]]) as u64;
    let spc = (boot[13] as u64).max(1);
    let reserved = u16::from_le_bytes([boot[14], boot[15]]) as u64;
    let num_fats = (boot[16] as u64).max(1);
    let fat_sz32 = u32::from_le_bytes(boot[36..40].try_into().unwrap()) as u64;
    let root_clus = u32::from_le_bytes(boot[44..48].try_into().unwrap());
    let total_secs = {
        let s16 = u16::from_le_bytes([boot[19], boot[20]]) as u64;
        if s16 != 0 {
            s16
        } else {
            u32::from_le_bytes(boot[32..36].try_into().unwrap()) as u64
        }
    };
    if !bytes_per_sec.is_power_of_two() || bytes_per_sec < 512 || fat_sz32 == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid FAT32 BPB"));
    }

    let cluster_size = bytes_per_sec * spc;
    let fat_offset = reserved * bytes_per_sec;
    let data_offset = (reserved + num_fats * fat_sz32) * bytes_per_sec;
    let data_secs = total_secs.saturating_sub(reserved + num_fats * fat_sz32);
    let cluster_count = data_secs / spc;

    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let mut visited_dirs: HashSet<u32> = HashSet::new();
    let mut queue: Vec<(u32, bool, String)> = vec![(root_clus, false, String::new())];
    let mut next_id = 1u64;

    while let Some((dir_clus, parent_deleted, prefix)) = queue.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if dir_clus < 2 || (dir_clus as u64) >= cluster_count + 2 || !visited_dirs.insert(dir_clus) {
            continue;
        }
        let chain = follow_fat32_chain(dev.as_ref(), fat_offset, dir_clus, cluster_count, 512);
        let dir_clusters = if chain.is_empty() {
            vec![dir_clus]
        } else {
            chain
        };

        let mut dir_bytes = Vec::new();
        for c in dir_clusters {
            let off = data_offset + ((c - 2) as u64) * cluster_size;
            if let Ok(mut b) = dev.read_at(off, cluster_size as usize) {
                dir_bytes.append(&mut b);
            }
        }

        let mut lfn_parts: Vec<Vec<u16>> = Vec::new();
        for e in dir_bytes.chunks_exact(32) {
            let first = e[0];
            if first == 0x00 {
                break;
            }
            let attr = e[11];
            // LFN entry (either active 0x01..0x54 or deleted 0xE5 with attr 0x0F)
            if attr == 0x0F && e[12] == 0x00 && e[26] == 0x00 && e[27] == 0x00 {
                lfn_parts.push(lfn_chunk_chars(e));
                continue;
            }
            // Skip volume labels
            if (attr & 0x08) != 0 {
                lfn_parts.clear();
                continue;
            }

            let is_deleted = first == 0xE5 || parent_deleted;
            let name = if !lfn_parts.is_empty() {
                let mut all_u16 = Vec::new();
                for part in lfn_parts.iter().rev() {
                    all_u16.extend_from_slice(part);
                }
                lfn_parts.clear();
                let s = String::from_utf16_lossy(&all_u16);
                if s.is_empty() {
                    decode_sfn(e)
                } else {
                    s
                }
            } else {
                decode_sfn(e)
            };

            if name == "." || name == ".." || name.is_empty() {
                continue;
            }

            let clus_hi = u16::from_le_bytes([e[20], e[21]]) as u32;
            let clus_lo = u16::from_le_bytes([e[26], e[27]]) as u32;
            let first_cluster = (clus_hi << 16) | clus_lo;
            let file_size = u32::from_le_bytes(e[28..32].try_into().unwrap()) as u64;
            let mod_time = u16::from_le_bytes([e[22], e[23]]);
            let mod_date = u16::from_le_bytes([e[24], e[25]]);
            let modified_unix = dos_time_to_unix(mod_date, mod_time);
            let is_dir = (attr & 0x10) != 0;

            let rel_path = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", prefix, name)
            };

            if is_dir {
                if first_cluster >= 2 {
                    queue.push((first_cluster, is_deleted, rel_path));
                }
            } else if is_deleted {
                let ext = name
                    .rsplit('.')
                    .next()
                    .filter(|x| *x != name)
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let (health, payload) = build_fat_payload(
                    dev.as_ref(),
                    fat_offset,
                    data_offset,
                    cluster_size,
                    cluster_count,
                    first_cluster,
                    file_size,
                    false,
                );
                entries.push(RecoveredEntry {
                    id: next_id,
                    name,
                    rel_path_lower: rel_path.to_ascii_lowercase(),
                    rel_path,
                    folder_path: prefix.clone(),
                    category: FileCategory::from_ext(&ext),
                    ext,
                    size: file_size,
                    modified_unix,
                    fs_kind: FileSystemKind::Fat32,
                    health,
                    source_desc: format!("FAT32 clus #{}", first_cluster),
                    payload,
                });
                next_id += 1;
            }
        }

        progress_cb(ScanEvent::Progress {
            stage: "FAT32".to_string(),
            fraction: 0.5,
            found_count: entries.len(),
            detail: format!("{} dirs", visited_dirs.len()),
        });
    }

    if entries.is_empty() {
        warnings.push("No deleted FAT32 directory entries found; try Deep Carving.".to_string());
    }
    Ok((entries, warnings))
}

#[allow(clippy::too_many_arguments)]
fn build_fat_payload(
    dev: &dyn BlockDevice,
    fat_offset: u64,
    heap_offset: u64,
    cluster_size: u64,
    cluster_count: u64,
    first_cluster: u32,
    file_size: u64,
    assume_contiguous: bool,
) -> (FileHealth, FilePayload) {
    if file_size == 0 {
        return (FileHealth::ZeroByte, FilePayload::Empty);
    }
    if first_cluster < 2 || (first_cluster as u64) >= cluster_count + 2 {
        return (FileHealth::PartialOrCorrupted, FilePayload::Empty);
    }
    let needed_clusters = ((file_size + cluster_size - 1) / cluster_size).max(1) as usize;
    let chain = if assume_contiguous {
        (0..needed_clusters)
            .map(|i| first_cluster + i as u32)
            .filter(|&c| (c as u64) < cluster_count + 2)
            .collect()
    } else {
        let fat_chain = follow_fat32_chain(
            dev,
            fat_offset,
            first_cluster,
            cluster_count,
            needed_clusters,
        );
        if fat_chain.len() >= needed_clusters {
            fat_chain
        } else {
            // Deleted FAT32 entries normally have FAT links cleared to 0; assume contiguous from first_cluster.
            (0..needed_clusters)
                .map(|i| first_cluster + i as u32)
                .filter(|&c| (c as u64) < cluster_count + 2)
                .collect()
        }
    };

    let runs = clusters_to_byte_runs(&chain, heap_offset, cluster_size);
    let peek = if let Some((Some(off), _)) = runs.first() {
        dev.read_at(*off, (file_size as usize).min(4096))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let health = if runs.is_empty() {
        FileHealth::PartialOrCorrupted
    } else if !peek.is_empty() && peek.iter().all(|&b| b == 0) {
        FileHealth::LikelyTrimmedZero
    } else if chain.len() < needed_clusters {
        FileHealth::PartialOrCorrupted
    } else {
        FileHealth::Intact
    };

    (
        health,
        FilePayload::Extents {
            runs,
            unit_bytes: 1,
            file_size,
            compressed: false,
            compression_unit_bytes: 0,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MemDev(Vec<u8>);
    impl BlockDevice for MemDev {
        fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
            let o = offset as usize;
            if o >= self.0.len() {
                return Ok(Vec::new());
            }
            Ok(self.0[o..(o + len).min(self.0.len())].to_vec())
        }
        fn total_size(&self) -> u64 {
            self.0.len() as u64
        }
        fn sector_size(&self) -> u32 {
            512
        }
    }

    #[test]
    fn test_exfat_deleted_file_recovery() {
        let sector = 512usize;
        let cluster = 4096usize;
        let mut img = vec![0u8; 32 * cluster];
        img[3..11].copy_from_slice(b"EXFAT   ");
        img[80..84].copy_from_slice(&4u32.to_le_bytes()); // fat_offset=4 sectors
        img[84..88].copy_from_slice(&4u32.to_le_bytes());
        img[88..92].copy_from_slice(&16u32.to_le_bytes()); // heap at 16 sectors = 8192
        img[92..96].copy_from_slice(&28u32.to_le_bytes()); // cluster_count
        img[96..100].copy_from_slice(&2u32.to_le_bytes()); // root at cluster 2
        img[108] = 9; // 512B
        img[109] = 3; // 4096B

        let heap = 16 * sector;
        // Root dir at cluster 2 (heap + 0): deleted file "photo.jpg" at cluster 5
        let payload = b"\xFF\xD8\xFF\xE0exfat-photo-data";
        let clus5_off = heap + (5 - 2) * cluster;
        img[clus5_off..clus5_off + payload.len()].copy_from_slice(payload);

        // Entry 0: 0x05 deleted File
        img[heap] = 0x05;
        img[heap + 1] = 2;
        // Entry 1: 0x40 deleted StreamExt
        let s = heap + 32;
        img[s] = 0x40;
        img[s + 1] = 0x02; // NoFatChain
        img[s + 3] = 9; // "photo.jpg".len()
        img[s + 20..s + 24].copy_from_slice(&5u32.to_le_bytes());
        img[s + 24..s + 32].copy_from_slice(&(payload.len() as u64).to_le_bytes());
        // Entry 2: 0x41 deleted Name
        let n = heap + 64;
        img[n] = 0x41;
        for (i, u) in "photo.jpg".encode_utf16().enumerate() {
            img[n + 2 + i * 2..n + 2 + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }

        let dev: Arc<dyn BlockDevice> = Arc::new(MemDev(img));
        let cancel = AtomicBool::new(false);
        let (entries, _) = scan_exfat(&dev, &cancel, &mut |_| {}).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].rel_path, "photo.jpg");
        assert_eq!(entries[0].health, FileHealth::Intact);
        assert_eq!(entries[0].size, payload.len() as u64);
        let recovered = crate::core::recover::read_entry_bytes(dev.as_ref(), &entries[0], 1024).unwrap();
        assert_eq!(recovered, payload);
    }

    #[test]
    fn test_fat32_deleted_lfn_file_recovery() {
        let sector = 512usize;
        let cluster = 512usize; // 1 sector/cluster for compact test
        let mut img = vec![0u8; 64 * cluster];
        img[11..13].copy_from_slice(&512u16.to_le_bytes());
        img[13] = 1;
        img[14..16].copy_from_slice(&2u16.to_le_bytes()); // reserved = 2
        img[16] = 1; // 1 FAT
        img[32..36].copy_from_slice(&64u32.to_le_bytes()); // total sectors
        img[36..40].copy_from_slice(&2u32.to_le_bytes()); // fat_sz32 = 2
        img[44..48].copy_from_slice(&2u32.to_le_bytes()); // root_cluster = 2
        img[82..90].copy_from_slice(b"FAT32   ");

        let data_off = (2 + 2) * sector;
        // Root at cluster 2 (data_off): deleted LFN "TatildeCekim.txt" -> SFN at cluster 4
        let msg = b"FAT32 LFN OK";
        let clus4 = data_off + (4 - 2) * cluster;
        img[clus4..clus4 + msg.len()].copy_from_slice(msg);

        // LFN part 2 ("im.txt") at entry 0, deleted 0xE5
        let mut e0 = [0xFFu8; 32];
        e0[0] = 0xE5;
        e0[11] = 0x0F;
        e0[12] = 0;
        e0[26] = 0;
        e0[27] = 0;
        let p2: Vec<u16> = "im.txt\0".encode_utf16().collect();
        let offs = [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
        for (i, &u) in p2.iter().enumerate() {
            e0[offs[i]..offs[i] + 2].copy_from_slice(&u.to_le_bytes());
        }
        img[data_off..data_off + 32].copy_from_slice(&e0);

        // LFN part 1 ("TatildeCek") at entry 1, deleted 0xE5
        let mut e1 = [0xFFu8; 32];
        e1[0] = 0xE5;
        e1[11] = 0x0F;
        e1[12] = 0;
        e1[26] = 0;
        e1[27] = 0;
        let p1: Vec<u16> = "TatildeCek\0".encode_utf16().collect();
        for (i, &u) in p1.iter().enumerate() {
            e1[offs[i]..offs[i] + 2].copy_from_slice(&u.to_le_bytes());
        }
        img[data_off + 32..data_off + 64].copy_from_slice(&e1);

        // SFN deleted entry at entry 2
        let mut sfn = [0u8; 32];
        sfn[0..11].copy_from_slice(b"\xE5ATILDE~TXT");
        sfn[11] = 0x20;
        sfn[26..28].copy_from_slice(&4u16.to_le_bytes()); // cluster 4
        sfn[28..32].copy_from_slice(&(msg.len() as u32).to_le_bytes());
        img[data_off + 64..data_off + 96].copy_from_slice(&sfn);

        let dev: Arc<dyn BlockDevice> = Arc::new(MemDev(img));
        let cancel = AtomicBool::new(false);
        let (entries, _) = scan_fat32(&dev, &cancel, &mut |_| {}).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].rel_path, "TatildeCekim.txt");
        assert_eq!(entries[0].size, msg.len() as u64);
        let recovered = crate::core::recover::read_entry_bytes(dev.as_ref(), &entries[0], 1024).unwrap();
        assert_eq!(recovered, msg);
    }
}
