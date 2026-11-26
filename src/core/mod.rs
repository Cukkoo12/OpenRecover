pub mod carver;
pub mod device;
pub mod fat;
pub mod ntfs;
pub mod recover;
pub mod types;

use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use device::{detect_filesystem, open_device_reader};
use types::{DeviceInfo, FileSystemKind, RecoveredEntry, ScanEvent, ScanMode};

/// Tree node representing a folder in the scanned results for fast UI folder browsing.
#[derive(Debug, Clone)]
pub struct FolderNode {
    pub name: String,
    pub path: String,
    pub file_indices: Vec<usize>,
    pub children: BTreeMap<String, FolderNode>,
    pub total_files: usize,
    pub total_bytes: u64,
}

impl FolderNode {
    pub fn new(name: String, path: String) -> Self {
        Self {
            name,
            path,
            file_indices: Vec::new(),
            children: BTreeMap::new(),
            total_files: 0,
            total_bytes: 0,
        }
    }

    pub fn build(entries: &[RecoveredEntry]) -> Self {
        let mut root = Self::new("Root".to_string(), String::new());
        for (idx, e) in entries.iter().enumerate() {
            if e.folder_path.is_empty() {
                root.file_indices.push(idx);
                root.total_files += 1;
                root.total_bytes += e.size;
                continue;
            }
            let mut cur = &mut root;
            cur.total_files += 1;
            cur.total_bytes += e.size;

            let mut acc = String::new();
            for part in e.folder_path.split('/').filter(|p| !p.is_empty()) {
                if !acc.is_empty() {
                    acc.push('/');
                }
                acc.push_str(part);
                let next_path = acc.clone();
                cur = cur
                    .children
                    .entry(part.to_string())
                    .or_insert_with(|| FolderNode::new(part.to_string(), next_path));
                cur.total_files += 1;
                cur.total_bytes += e.size;
            }
            cur.file_indices.push(idx);
        }
        root
    }

    /// Collect all file indices under this folder and all its subfolders.
    pub fn collect_all_indices(&self, out: &mut Vec<usize>) {
        out.extend_from_slice(&self.file_indices);
        for child in self.children.values() {
            child.collect_all_indices(out);
        }
    }
}

/// Run the selected scan mode (`QuickTree`, `DeepCarve`, or `Combined`) against a device/image.
pub fn run_scan(
    device: &DeviceInfo,
    mode: ScanMode,
    cancel: &AtomicBool,
    progress_cb: &mut dyn FnMut(ScanEvent),
) -> ScanEvent {
    let started = Instant::now();
    let dev = match open_device_reader(device) {
        Ok(d) => d,
        Err(e) => return ScanEvent::Failed(e.to_string()),
    };

    let fs_kind = if device.fs_kind != FileSystemKind::Unknown {
        device.fs_kind
    } else {
        detect_filesystem(dev.as_ref())
    };

    let mut all_entries: Vec<RecoveredEntry> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    if matches!(mode, ScanMode::QuickTree | ScanMode::Combined) {
        match fs_kind {
            FileSystemKind::Ntfs => match ntfs::scan_ntfs(&dev, cancel, progress_cb) {
                Ok((mut e, mut w)) => {
                    all_entries.append(&mut e);
                    warnings.append(&mut w);
                }
                Err(e) => warnings.push(format!("NTFS scan note: {}", e)),
            },
            FileSystemKind::ExFat => match fat::scan_exfat(&dev, cancel, progress_cb) {
                Ok((mut e, mut w)) => {
                    all_entries.append(&mut e);
                    warnings.append(&mut w);
                }
                Err(e) => warnings.push(format!("exFAT scan note: {}", e)),
            },
            FileSystemKind::Fat32 => match fat::scan_fat32(&dev, cancel, progress_cb) {
                Ok((mut e, mut w)) => {
                    all_entries.append(&mut e);
                    warnings.append(&mut w);
                }
                Err(e) => warnings.push(format!("FAT32 scan note: {}", e)),
            },
            _ => {
                if mode == ScanMode::QuickTree {
                    // Check if the device is a raw disk with partitions (PhysicalDrive / raw image)
                    let parts = device::parse_partitions(dev.as_ref());
                    if parts.is_empty() {
                        warnings.push(
                            "Unknown/RAW filesystem: run Deep Carving to recover by file signatures."
                                .to_string(),
                        );
                    } else {
                        for part in parts {
                            let slice: Arc<dyn device::BlockDevice> =
                                Arc::new(device::SliceBlockDevice::new(
                                    Arc::clone(&dev),
                                    part.offset_bytes,
                                    part.size_bytes,
                                ));
                            let res = match part.fs_kind {
                                FileSystemKind::Ntfs => ntfs::scan_ntfs(&slice, cancel, progress_cb),
                                FileSystemKind::ExFat => fat::scan_exfat(&slice, cancel, progress_cb),
                                FileSystemKind::Fat32 => fat::scan_fat32(&slice, cancel, progress_cb),
                                _ => continue,
                            };
                            if let Ok((mut e, mut w)) = res {
                                // Shift partition-relative extent offsets by partition base offset!
                                shift_extents_by(&mut e, part.offset_bytes);
                                all_entries.append(&mut e);
                                warnings.append(&mut w);
                            }
                        }
                    }
                }
            }
        }
    }

    if matches!(mode, ScanMode::DeepCarve | ScanMode::Combined) {
        match carver::scan_carve(&dev, cancel, progress_cb) {
            Ok((mut e, mut w)) => {
                all_entries.append(&mut e);
                warnings.append(&mut w);
            }
            Err(e) => warnings.push(format!("Carving note: {}", e)),
        }
    }

    ScanEvent::Finished {
        entries: all_entries,
        elapsed_secs: started.elapsed().as_secs_f64(),
        warnings,
    }
}

fn shift_extents_by(entries: &mut [RecoveredEntry], base_offset: u64) {
    if base_offset == 0 {
        return;
    }
    for e in entries {
        if let types::FilePayload::Extents {
            ref mut runs,
            unit_bytes,
            file_size,
            compressed,
            compression_unit_bytes,
        } = e.payload
        {
            if unit_bytes > 1 && base_offset % unit_bytes == 0 {
                let delta_units = base_offset / unit_bytes;
                for (lcn_opt, _) in runs.iter_mut() {
                    if let Some(lcn) = lcn_opt {
                        *lcn = lcn.saturating_add(delta_units);
                    }
                }
            } else if !compressed {
                // Convert cluster runs to exact byte offsets when partition isn't cluster-aligned
                let mut byte_runs = Vec::with_capacity(runs.len());
                for &(lcn_opt, cnt) in runs.iter() {
                    let b_off = lcn_opt.map(|l| base_offset + l * unit_bytes);
                    byte_runs.push((b_off, cnt * unit_bytes));
                }
                e.payload = types::FilePayload::Extents {
                    runs: byte_runs,
                    unit_bytes: 1,
                    file_size,
                    compressed,
                    compression_unit_bytes,
                };
            }
        }
    }
}
