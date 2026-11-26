use std::fmt;

/// Detected or scanned filesystem type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileSystemKind {
    Ntfs,
    ExFat,
    Fat32,
    RawCarved,
    Unknown,
}

impl FileSystemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ntfs => "NTFS",
            Self::ExFat => "exFAT",
            Self::Fat32 => "FAT32",
            Self::RawCarved => "RAW",
            Self::Unknown => "Unknown",
        }
    }

    pub fn label(self, tr: bool) -> &'static str {
        match self {
            Self::Unknown if tr => "Bilinmiyor",
            _ => self.as_str(),
        }
    }
}

/// Hardware bus type for storage devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BusKind {
    Nvme,
    Sata,
    Usb,
    SdMmc,
    Virtual,
    ImageFile,
    Other,
}

impl BusKind {
    pub fn label(self, tr: bool) -> &'static str {
        match self {
            Self::Nvme => "NVMe SSD",
            Self::Sata => "SATA",
            Self::Usb => "USB",
            Self::SdMmc => {
                if tr {
                    "SD / Hafıza Kartı"
                } else {
                    "SD / Memory Card"
                }
            }
            Self::Virtual => {
                if tr {
                    "Sanal Disk"
                } else {
                    "Virtual"
                }
            }
            Self::ImageFile => {
                if tr {
                    "Disk İmajı"
                } else {
                    "Disk Image"
                }
            }
            Self::Other => "Disk",
        }
    }
}

/// Information about a scannable volume, physical disk partition, or disk image.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub id: String,
    pub display_name: String,
    pub mount_point: Option<String>,
    pub volume_label: String,
    pub fs_kind: FileSystemKind,
    pub bus_kind: BusKind,
    pub is_removable: bool,
    pub is_physical_partition: bool,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub sector_size: u32,
    pub base_offset: u64,
    pub part_length: u64,
    pub raw_device_path: String,
    pub physical_disk_number: Option<u32>,
}

/// Scan strategy requested by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanMode {
    QuickTree,
    DeepCarve,
    Combined,
}

/// Estimated recoverability health of a deleted entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileHealth {
    Intact,
    Resident,
    ZeroByte,
    LikelyTrimmedZero,
    PartialOrCorrupted,
}

impl FileHealth {
    pub fn label(self, tr: bool) -> &'static str {
        match self {
            Self::Intact => {
                if tr {
                    "Sağlam"
                } else {
                    "Intact"
                }
            }
            Self::Resident => {
                if tr {
                    "Yerleşik (MFT)"
                } else {
                    "Resident (MFT)"
                }
            }
            Self::ZeroByte => {
                if tr {
                    "0 Bayt"
                } else {
                    "0 Bytes"
                }
            }
            Self::LikelyTrimmedZero => {
                if tr {
                    "TRIM (Boş)"
                } else {
                    "TRIM (Zeroed)"
                }
            }
            Self::PartialOrCorrupted => {
                if tr {
                    "Kısmi / Riskli"
                } else {
                    "Partial / Risky"
                }
            }
        }
    }
}

/// High-level file category for fast filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileCategory {
    All,
    Images,
    Documents,
    VideosAudio,
    Archives,
    CodeConfig,
    Other,
}

impl FileCategory {
    pub fn from_ext(ext: &str) -> Self {
        match ext {
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "ico" | "svg" | "tif" | "tiff"
            | "raw" | "cr2" | "nef" | "arw" | "heic" => Self::Images,
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "rtf" | "md"
            | "csv" | "odt" | "ods" => Self::Documents,
            "mp4" | "mov" | "mkv" | "avi" | "wmv" | "webm" | "flv" | "mp3" | "wav" | "flac"
            | "ogg" | "m4a" | "aac" => Self::VideosAudio,
            "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "iso" => Self::Archives,
            "json" | "yaml" | "yml" | "toml" | "xml" | "ini" | "cfg" | "conf" | "rs" | "py"
            | "js" | "ts" | "tsx" | "jsx" | "cs" | "cpp" | "c" | "h" | "java" | "gd"
            | "html" | "css" | "sql" | "db" | "sqlite" => Self::CodeConfig,
            _ => Self::Other,
        }
    }

    pub fn label(self, tr: bool) -> &'static str {
        match self {
            Self::All => {
                if tr {
                    "Tümü"
                } else {
                    "All"
                }
            }
            Self::Images => {
                if tr {
                    "Fotoğraf"
                } else {
                    "Photos"
                }
            }
            Self::Documents => {
                if tr {
                    "Belgeler"
                } else {
                    "Docs"
                }
            }
            Self::VideosAudio => {
                if tr {
                    "Video / Ses"
                } else {
                    "Video / Audio"
                }
            }
            Self::Archives => {
                if tr {
                    "Arşiv"
                } else {
                    "Archives"
                }
            }
            Self::CodeConfig => {
                if tr {
                    "Kod / Ayar"
                } else {
                    "Code / Cfg"
                }
            }
            Self::Other => {
                if tr {
                    "Diğer"
                } else {
                    "Other"
                }
            }
        }
    }
}

/// How the bytes of a recoverable entry are stored on the underlying block device.
#[derive(Debug, Clone)]
pub enum FilePayload {
    Empty,
    Inline(Vec<u8>),
    /// Non-resident extents: `(optional byte offset relative to volume, length in units)`.
    /// `None` offset represents a sparse zeroed run.
    Extents {
        runs: Vec<(Option<u64>, u64)>,
        unit_bytes: u64,
        file_size: u64,
        compressed: bool,
        compression_unit_bytes: u64,
    },
}

/// A single recoverable file discovered during scanning.
#[derive(Debug, Clone)]
pub struct RecoveredEntry {
    pub id: u64,
    pub name: String,
    pub rel_path: String,
    pub rel_path_lower: String,
    pub folder_path: String,
    pub ext: String,
    pub size: u64,
    pub modified_unix: i64,
    pub fs_kind: FileSystemKind,
    pub health: FileHealth,
    pub category: FileCategory,
    pub source_desc: String,
    pub payload: FilePayload,
}

/// Live progress update emitted by background scan or recovery threads.
#[derive(Debug, Clone)]
pub enum ScanEvent {
    Progress {
        stage: String,
        fraction: f32,
        found_count: usize,
        detail: String,
    },
    Finished {
        entries: Vec<RecoveredEntry>,
        elapsed_secs: f64,
        warnings: Vec<String>,
    },
    Failed(String),
}

/// Recovery extraction progress / completion events.
#[derive(Debug, Clone)]
pub enum RecoverEvent {
    Progress {
        done_files: usize,
        total_files: usize,
        bytes_written: u64,
        current_file: String,
    },
    Finished(RecoverSummary),
    Failed(String),
}

#[derive(Debug, Clone, Default)]
pub struct RecoverSummary {
    pub recovered_intact: usize,
    pub recovered_zero_size: usize,
    pub separated_trimmed: usize,
    pub skipped_errors: usize,
    pub bytes_written: u64,
    pub dest_dir: String,
    pub elapsed_secs: f64,
}

/// Format byte sizes cleanly for display.
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    const TB: f64 = GB * 1024.0;
    let b = bytes as f64;
    if b >= TB {
        format!("{:.2} TB", b / TB)
    } else if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

impl fmt::Display for FileSystemKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
