use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::core::device::BlockDevice;
use crate::core::ntfs::lznt1_decompress;
use crate::core::types::{FilePayload, RecoverEvent, RecoverSummary, RecoveredEntry};

const RESERVED_WIN_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

pub fn sanitize_component(name: &str) -> String {
    let mut cleaned: String = name
        .chars()
        .map(|c| {
            if (c as u32) < 32 || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    while cleaned.ends_with([' ', '.']) {
        cleaned.pop();
    }
    if cleaned.is_empty() {
        cleaned = "_empty".to_string();
    }
    let stem = cleaned.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED_WIN_NAMES.contains(&stem.as_str()) {
        cleaned = format!("_{}", cleaned);
    }
    cleaned
}

#[cfg(windows)]
fn to_long_path(path: &Path) -> PathBuf {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let s = abs.to_string_lossy();
    if s.starts_with(r"\\?\") {
        abs
    } else if let Some(unc) = s.strip_prefix(r"\\") {
        PathBuf::from(format!(r"\\?\UNC\{}", unc))
    } else {
        PathBuf::from(format!(r"\\?\{}", s))
    }
}

#[cfg(not(windows))]
fn to_long_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

/// Read up to `max_bytes` from an entry for instant UI preview (image / text / hex).
pub fn read_entry_bytes(
    dev: &dyn BlockDevice,
    entry: &RecoveredEntry,
    max_bytes: usize,
) -> io::Result<Vec<u8>> {
    match &entry.payload {
        FilePayload::Empty => Ok(Vec::new()),
        FilePayload::Inline(data) => {
            let take = data.len().min(max_bytes);
            Ok(data[..take].to_vec())
        }
        FilePayload::Extents {
            runs,
            unit_bytes,
            file_size,
            compressed,
            compression_unit_bytes,
        } => {
            let target = (*file_size as usize).min(max_bytes);
            let mut out = Vec::with_capacity(target);
            stream_extents(
                dev,
                runs,
                *unit_bytes,
                *file_size,
                *compressed,
                *compression_unit_bytes,
                target,
                &mut |chunk| {
                    out.extend_from_slice(chunk);
                    Ok(())
                },
            )?;
            Ok(out)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn stream_extents(
    dev: &dyn BlockDevice,
    runs: &[(Option<u64>, u64)],
    unit_bytes: u64,
    file_size: u64,
    compressed: bool,
    compression_unit_bytes: u64,
    limit_bytes: usize,
    sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<()> {
    let want_total = (file_size as usize).min(limit_bytes);
    if want_total == 0 {
        return Ok(());
    }

    if compressed && compression_unit_bytes > 0 && unit_bytes > 0 {
        let cu_clusters = (compression_unit_bytes / unit_bytes).max(1);
        let mut cur_cu_runs: Vec<(Option<u64>, u64)> = Vec::new();
        let mut cur_cu_rem = cu_clusters;
        let mut written = 0usize;

        let mut flush_cu = |cu_runs: &[(Option<u64>, u64)], written_so_far: &mut usize| -> io::Result<bool> {
            let need = (want_total - *written_so_far).min(compression_unit_bytes as usize);
            let all_sparse = cu_runs.iter().all(|(o, _)| o.is_none());
            let has_sparse = cu_runs.iter().any(|(o, _)| o.is_none());
            if all_sparse {
                sink(&vec![0u8; need])?;
                *written_so_far += need;
            } else if has_sparse {
                let mut raw = Vec::new();
                for &(off_opt, cnt) in cu_runs {
                    if let Some(unit_idx) = off_opt {
                        let byte_len = (cnt * unit_bytes) as usize;
                        let part = dev.read_at(unit_idx * unit_bytes, byte_len)?;
                        raw.extend_from_slice(&part);
                    }
                }
                let decompressed = lznt1_decompress(&raw, compression_unit_bytes as usize)
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "LZNT1 error"))?;
                let take = need.min(decompressed.len());
                sink(&decompressed[..take])?;
                *written_so_far += take;
            } else {
                for &(off_opt, cnt) in cu_runs {
                    if *written_so_far >= want_total {
                        break;
                    }
                    if let Some(unit_idx) = off_opt {
                        let avail = (cnt * unit_bytes) as usize;
                        let take = avail.min(want_total - *written_so_far);
                        let part = dev.read_at(unit_idx * unit_bytes, take)?;
                        sink(&part)?;
                        *written_so_far += part.len();
                    }
                }
            }
            Ok(*written_so_far < want_total)
        };

        for &(off_opt, mut cnt) in runs {
            while cnt > 0 && written < want_total {
                let take = cnt.min(cur_cu_rem);
                cur_cu_runs.push((off_opt, take));
                cur_cu_rem -= take;
                cnt -= take;
                if cur_cu_rem == 0 {
                    let cont = flush_cu(&cur_cu_runs, &mut written)?;
                    cur_cu_runs.clear();
                    cur_cu_rem = cu_clusters;
                    if !cont {
                        return Ok(());
                    }
                }
            }
        }
        if !cur_cu_runs.is_empty() && written < want_total {
            let _ = flush_cu(&cur_cu_runs, &mut written)?;
        }
        return Ok(());
    }

    let mut written = 0usize;
    const STREAM_CHUNK: usize = 1024 * 1024;
    for &(off_opt, cnt) in runs {
        if written >= want_total {
            break;
        }
        let run_bytes = cnt.saturating_mul(unit_bytes);
        let mut in_run = 0u64;
        while in_run < run_bytes && written < want_total {
            let rem_file = want_total - written;
            let rem_run = (run_bytes - in_run) as usize;
            let step = STREAM_CHUNK.min(rem_file).min(rem_run);
            match off_opt {
                None => {
                    sink(&vec![0u8; step])?;
                    written += step;
                }
                Some(unit_idx) => {
                    let disk_off = unit_idx.saturating_mul(unit_bytes).saturating_add(in_run);
                    let chunk = dev.read_at(disk_off, step)?;
                    if chunk.is_empty() {
                        return Ok(());
                    }
                    sink(&chunk)?;
                    written += chunk.len();
                    if chunk.len() < step {
                        return Ok(());
                    }
                }
            }
            in_run += step as u64;
        }
    }
    Ok(())
}

fn build_dest_path(base: &Path, rel_path: &str, used: &mut HashSet<String>, id: u64) -> PathBuf {
    let mut parts: Vec<String> = rel_path
        .split(['/', '\\'])
        .filter(|s| !s.is_empty())
        .map(sanitize_component)
        .collect();
    if parts.is_empty() {
        parts.push(format!("file_{}", id));
    }
    let key = parts
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("/");

    if !used.insert(key) {
        if let Some(last) = parts.last_mut() {
            if let Some((stem, ext)) = last.rsplit_once('.') {
                *last = format!("{}_{}.{}", stem, id, ext);
            } else {
                *last = format!("{}_{}", last, id);
            }
        }
    }

    let mut p = base.to_path_buf();
    for part in parts {
        p.push(part);
    }
    p
}

/// Recover a selected list of entries into `dest_dir` safely.
pub fn recover_entries(
    dev: &Arc<dyn BlockDevice>,
    entries: &[RecoveredEntry],
    dest_dir: &Path,
    separate_trimmed: bool,
    cancel: &AtomicBool,
    progress_cb: &mut dyn FnMut(RecoverEvent),
) -> RecoverSummary {
    let started = Instant::now();
    let mut summary = RecoverSummary {
        dest_dir: dest_dir.to_string_lossy().to_string(),
        ..Default::default()
    };

    let trim_dir = dest_dir.join("_TRIM_Zeroed");
    let mut used_paths: HashSet<String> = HashSet::new();
    let total = entries.len();

    for (idx, entry) in entries.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let dest = build_dest_path(dest_dir, &entry.rel_path, &mut used_paths, entry.id);

        if entry.size == 0 || matches!(entry.payload, FilePayload::Empty) {
            let lp = to_long_path(&dest);
            if let Some(parent) = lp.parent() {
                let _ = fs::create_dir_all(parent);
            }
            match File::create(&lp) {
                Ok(_) => summary.recovered_zero_size += 1,
                Err(_) => summary.skipped_errors += 1,
            }
        } else {
            let mut wrote_bytes = 0u64;
            let mut has_nonzero = false;

            let res: io::Result<()> = (|| -> io::Result<()> {
                match &entry.payload {
                    FilePayload::Empty => Ok(()),
                    FilePayload::Inline(data) => {
                        has_nonzero = data.iter().any(|&b| b != 0);
                        let target = if separate_trimmed && !has_nonzero {
                            trim_dir.join(dest.strip_prefix(dest_dir).unwrap_or(&dest))
                        } else {
                            dest.clone()
                        };
                        let lp = to_long_path(&target);
                        if let Some(parent) = lp.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        let mut f = File::create(&lp)?;
                        f.write_all(data)?;
                        wrote_bytes = data.len() as u64;
                        Ok(())
                    }
                    FilePayload::Extents {
                        runs,
                        unit_bytes,
                        file_size,
                        compressed,
                        compression_unit_bytes,
                    } => {
                        let lp = to_long_path(&dest);
                        if let Some(parent) = lp.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        let mut f = File::create(&lp)?;
                        stream_extents(
                            dev.as_ref(),
                            runs,
                            *unit_bytes,
                            *file_size,
                            *compressed,
                            *compression_unit_bytes,
                            *file_size as usize,
                            &mut |chunk| {
                                if !has_nonzero && chunk.iter().any(|&b| b != 0) {
                                    has_nonzero = true;
                                }
                                f.write_all(chunk)?;
                                wrote_bytes += chunk.len() as u64;
                                Ok(())
                            },
                        )?;
                        drop(f);
                        if separate_trimmed && !has_nonzero && wrote_bytes > 0 {
                            let trim_target =
                                trim_dir.join(dest.strip_prefix(dest_dir).unwrap_or(&dest));
                            let trim_lp = to_long_path(&trim_target);
                            if let Some(tp) = trim_lp.parent() {
                                let _ = fs::create_dir_all(tp);
                            }
                            let _ = fs::rename(&lp, &trim_lp);
                        }
                        Ok(())
                    }
                }
            })();

            match res {
                Ok(()) => {
                    if separate_trimmed && !has_nonzero && wrote_bytes > 0 {
                        summary.separated_trimmed += 1;
                    } else {
                        summary.recovered_intact += 1;
                        summary.bytes_written += wrote_bytes;
                    }
                }
                Err(_) => {
                    summary.skipped_errors += 1;
                }
            }
        }

        if (idx + 1) % 50 == 0 || idx + 1 == total {
            progress_cb(RecoverEvent::Progress {
                done_files: idx + 1,
                total_files: total,
                bytes_written: summary.bytes_written,
                current_file: entry.name.clone(),
            });
        }
    }

    summary.elapsed_secs = started.elapsed().as_secs_f64();
    summary
}
