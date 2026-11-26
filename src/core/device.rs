use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::core::types::{format_bytes, BusKind, DeviceInfo, FileSystemKind};

/// Abstract read-only random-access block device (raw volume, physical disk, partition slice, or `.img` file).
pub trait BlockDevice: Send + Sync {
    fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>>;
    fn total_size(&self) -> u64;
    fn sector_size(&self) -> u32;
}

/// Partition / byte-range view over an underlying `BlockDevice`.
pub struct SliceBlockDevice {
    inner: Arc<dyn BlockDevice>,
    base_offset: u64,
    length: u64,
}

impl SliceBlockDevice {
    pub fn new(inner: Arc<dyn BlockDevice>, base_offset: u64, length: u64) -> Self {
        let clamped_len = length.min(inner.total_size().saturating_sub(base_offset));
        Self {
            inner,
            base_offset,
            length: clamped_len,
        }
    }
}

impl BlockDevice for SliceBlockDevice {
    fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        if offset >= self.length || len == 0 {
            return Ok(Vec::new());
        }
        let available = (self.length - offset) as usize;
        let to_read = len.min(available);
        self.inner.read_at(self.base_offset + offset, to_read)
    }

    fn total_size(&self) -> u64 {
        self.length
    }

    fn sector_size(&self) -> u32 {
        self.inner.sector_size()
    }
}

/// Cross-platform disk image reader (`.img`, `.dd`, `.raw`).
pub struct ImageFileDevice {
    file: Mutex<File>,
    size: u64,
    sector_size: u32,
}

impl ImageFileDevice {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            file: Mutex::new(file),
            size,
            sector_size: 512,
        })
    }
}

impl BlockDevice for ImageFileDevice {
    fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        if offset >= self.size || len == 0 {
            return Ok(Vec::new());
        }
        let to_read = len.min((self.size - offset) as usize);
        let mut buf = vec![0u8; to_read];
        let mut guard = self
            .file
            .lock()
            .map_err(|_| io::Error::other("image lock poisoned"))?;
        guard.seek(SeekFrom::Start(offset))?;
        let mut total = 0;
        while total < to_read {
            let n = guard.read(&mut buf[total..])?;
            if n == 0 {
                break;
            }
            total += n;
        }
        buf.truncate(total);
        Ok(buf)
    }

    fn total_size(&self) -> u64 {
        self.size
    }

    fn sector_size(&self) -> u32 {
        self.sector_size
    }
}

/// Sector-aligned caching wrapper around any `BlockDevice` (64 KB aligned blocks).
pub struct CachedBlockDevice {
    inner: Arc<dyn BlockDevice>,
    chunk_size: usize,
    max_chunks: usize,
    cache: Mutex<ChunkLru>,
}

struct ChunkLru {
    order: VecDeque<(u64, Arc<Vec<u8>>)>,
}

impl ChunkLru {
    fn get(&mut self, key: u64) -> Option<Arc<Vec<u8>>> {
        let pos = self.order.iter().position(|(k, _)| *k == key)?;
        let item = self.order.remove(pos)?;
        let val = Arc::clone(&item.1);
        self.order.push_back(item);
        Some(val)
    }

    fn insert(&mut self, key: u64, val: Arc<Vec<u8>>, max_chunks: usize) {
        if let Some(pos) = self.order.iter().position(|(k, _)| *k == key) {
            self.order.remove(pos);
        }
        self.order.push_back((key, val));
        while self.order.len() > max_chunks {
            self.order.pop_front();
        }
    }
}

impl CachedBlockDevice {
    pub fn new(inner: Arc<dyn BlockDevice>) -> Self {
        Self {
            inner,
            chunk_size: 65536,
            max_chunks: 256, // 16 MB cache
            cache: Mutex::new(ChunkLru {
                order: VecDeque::with_capacity(256),
            }),
        }
    }
}

impl BlockDevice for CachedBlockDevice {
    fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let total = self.inner.total_size();
        if offset >= total || len == 0 {
            return Ok(Vec::new());
        }
        let target_len = len.min((total - offset) as usize);

        // Large contiguous reads bypass the small-block cache to avoid cache churn.
        if target_len >= self.chunk_size * 4 {
            let align = (self.inner.sector_size().max(512)) as u64;
            let aligned_start = (offset / align) * align;
            let end = offset + target_len as u64;
            let aligned_end = ((end + align - 1) / align) * align;
            let raw_len = (aligned_end - aligned_start) as usize;
            let raw = self.inner.read_at(aligned_start, raw_len)?;
            let head = (offset - aligned_start) as usize;
            let available = raw.len().saturating_sub(head);
            let copy_len = target_len.min(available);
            return Ok(raw[head..head + copy_len].to_vec());
        }

        let mut out = Vec::with_capacity(target_len);
        let mut cur = offset;
        let mut remaining = target_len;
        let cs = self.chunk_size as u64;

        while remaining > 0 {
            let chunk_start = (cur / cs) * cs;
            let in_chunk = (cur - chunk_start) as usize;
            let cached = {
                let mut guard = self
                    .cache
                    .lock()
                    .map_err(|_| io::Error::other("cache lock poisoned"))?;
                guard.get(chunk_start)
            };

            let chunk = match cached {
                Some(c) => c,
                None => {
                    let data = Arc::new(self.inner.read_at(chunk_start, self.chunk_size)?);
                    let mut guard = self
                        .cache
                        .lock()
                        .map_err(|_| io::Error::other("cache lock poisoned"))?;
                    guard.insert(chunk_start, Arc::clone(&data), self.max_chunks);
                    data
                }
            };

            if in_chunk >= chunk.len() {
                break;
            }
            let take = remaining.min(chunk.len() - in_chunk);
            out.extend_from_slice(&chunk[in_chunk..in_chunk + take]);
            cur += take as u64;
            remaining -= take;
        }
        Ok(out)
    }

    fn total_size(&self) -> u64 {
        self.inner.total_size()
    }

    fn sector_size(&self) -> u32 {
        self.inner.sector_size()
    }
}

/// Detect filesystem magic from the first 4 KB of a volume or partition.
pub fn detect_filesystem(dev: &dyn BlockDevice) -> FileSystemKind {
    let Ok(boot) = dev.read_at(0, 4096) else {
        return FileSystemKind::Unknown;
    };
    if boot.len() < 512 {
        return FileSystemKind::Unknown;
    }
    if &boot[3..11] == b"NTFS    " {
        return FileSystemKind::Ntfs;
    }
    if &boot[3..11] == b"EXFAT   " {
        return FileSystemKind::ExFat;
    }
    if &boot[82..90] == b"FAT32   " {
        return FileSystemKind::Fat32;
    }
    // Backup FAT32 detection by BPB signature
    let bytes_per_sec = u16::from_le_bytes([boot[11], boot[12]]);
    let fat_sz16 = u16::from_le_bytes([boot[22], boot[23]]);
    let root_ent = u16::from_le_bytes([boot[17], boot[18]]);
    let fat_sz32 = u32::from_le_bytes([boot[36], boot[37], boot[38], boot[39]]);
    if boot[510] == 0x55
        && boot[511] == 0xAA
        && bytes_per_sec.is_power_of_two()
        && bytes_per_sec >= 512
        && fat_sz16 == 0
        && root_ent == 0
        && fat_sz32 > 0
    {
        return FileSystemKind::Fat32;
    }
    FileSystemKind::Unknown
}

/// Discovered MBR/GPT partition entry.
#[derive(Debug, Clone)]
pub struct PartitionEntry {
    pub index: usize,
    pub offset_bytes: u64,
    pub size_bytes: u64,
    pub label: String,
    pub fs_kind: FileSystemKind,
}

/// Parse GPT or MBR partition tables from a raw disk/image.
pub fn parse_partitions(dev: &dyn BlockDevice) -> Vec<PartitionEntry> {
    let sector = dev.sector_size().max(512) as u64;
    let Ok(hdr) = dev.read_at(sector, 512) else {
        return Vec::new();
    };
    let mut parts = Vec::new();

    if hdr.len() >= 92 && &hdr[0..8] == b"EFI PART" {
        let part_lba = u64::from_le_bytes(hdr[72..80].try_into().unwrap());
        let num_entries = u32::from_le_bytes(hdr[80..84].try_into().unwrap()).min(128) as usize;
        let entry_size = u32::from_le_bytes(hdr[84..88].try_into().unwrap()) as usize;
        if entry_size >= 128 && num_entries > 0 {
            if let Ok(table) = dev.read_at(part_lba * sector, num_entries * entry_size) {
                for i in 0..num_entries {
                    let base = i * entry_size;
                    if base + 128 > table.len() {
                        break;
                    }
                    let e = &table[base..base + 128];
                    if e[0..16].iter().all(|&b| b == 0) {
                        continue;
                    }
                    let first_lba = u64::from_le_bytes(e[32..40].try_into().unwrap());
                    let last_lba = u64::from_le_bytes(e[40..48].try_into().unwrap());
                    if last_lba < first_lba {
                        continue;
                    }
                    let name_u16: Vec<u16> = e[56..128]
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .take_while(|&v| v != 0)
                        .collect();
                    let label = String::from_utf16_lossy(&name_u16);
                    let offset_bytes = first_lba * sector;
                    let size_bytes = (last_lba - first_lba + 1) * sector;
                    let slice = SliceDeviceProbe {
                        dev,
                        base: offset_bytes,
                        len: size_bytes,
                    };
                    let fs_kind = detect_filesystem(&slice);
                    parts.push(PartitionEntry {
                        index: parts.len() + 1,
                        offset_bytes,
                        size_bytes,
                        label,
                        fs_kind,
                    });
                }
            }
        }
        return parts;
    }

    // Fallback to classic MBR
    let Ok(mbr) = dev.read_at(0, 512) else {
        return Vec::new();
    };
    if mbr.len() == 512 && mbr[510] == 0x55 && mbr[511] == 0xAA {
        for i in 0..4 {
            let off = 446 + i * 16;
            let ptype = mbr[off + 4];
            if ptype == 0 || ptype == 0xEE {
                continue;
            }
            let start_lba = u32::from_le_bytes(mbr[off + 8..off + 12].try_into().unwrap()) as u64;
            let num_sectors =
                u32::from_le_bytes(mbr[off + 12..off + 16].try_into().unwrap()) as u64;
            if num_sectors == 0 {
                continue;
            }
            let offset_bytes = start_lba * sector;
            let size_bytes = num_sectors * sector;
            let slice = SliceDeviceProbe {
                dev,
                base: offset_bytes,
                len: size_bytes,
            };
            let fs_kind = detect_filesystem(&slice);
            parts.push(PartitionEntry {
                index: parts.len() + 1,
                offset_bytes,
                size_bytes,
                label: format!("Partition {}", i + 1),
                fs_kind,
            });
        }
    }
    parts
}

struct SliceDeviceProbe<'a> {
    dev: &'a dyn BlockDevice,
    base: u64,
    len: u64,
}

impl<'a> BlockDevice for SliceDeviceProbe<'a> {
    fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        if offset >= self.len {
            return Ok(Vec::new());
        }
        self.dev
            .read_at(self.base + offset, len.min((self.len - offset) as usize))
    }

    fn total_size(&self) -> u64 {
        self.len
    }

    fn sector_size(&self) -> u32 {
        self.dev.sector_size()
    }
}

/// Open a `DeviceInfo` as a ready-to-scan cached `BlockDevice`.
pub fn open_device_reader(info: &DeviceInfo) -> io::Result<Arc<dyn BlockDevice>> {
    let raw: Arc<dyn BlockDevice> = if info.bus_kind == BusKind::ImageFile {
        Arc::new(ImageFileDevice::open(Path::new(&info.raw_device_path))?)
    } else {
        #[cfg(windows)]
        {
            Arc::new(WindowsRawDevice::open(&info.raw_device_path, info.sector_size)?)
        }
        #[cfg(not(windows))]
        {
            Arc::new(ImageFileDevice::open(Path::new(&info.raw_device_path))?)
        }
    };

    let ranged: Arc<dyn BlockDevice> = if info.base_offset > 0 || info.part_length > 0 {
        let len = if info.part_length > 0 {
            info.part_length
        } else {
            raw.total_size().saturating_sub(info.base_offset)
        };
        Arc::new(SliceBlockDevice::new(raw, info.base_offset, len))
    } else {
        raw
    };

    Ok(Arc::new(CachedBlockDevice::new(ranged)))
}

/// Build `DeviceInfo` entries for a user-selected disk image file (`.img` / `.dd` / `.raw`).
pub fn inspect_image_file(path: &Path) -> io::Result<Vec<DeviceInfo>> {
    let raw = Arc::new(ImageFileDevice::open(path)?);
    let total = raw.total_size();
    let filename = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    let path_str = path.to_string_lossy().to_string();

    let mut out = Vec::new();
    let fs_root = detect_filesystem(raw.as_ref());
    if fs_root != FileSystemKind::Unknown {
        out.push(DeviceInfo {
            id: format!("img:{}:0", path_str),
            display_name: format!("{} ({})", filename, format_bytes(total)),
            mount_point: None,
            volume_label: filename.clone(),
            fs_kind: fs_root,
            bus_kind: BusKind::ImageFile,
            is_removable: true,
            is_physical_partition: false,
            total_bytes: total,
            free_bytes: 0,
            sector_size: 512,
            base_offset: 0,
            part_length: total,
            raw_device_path: path_str.clone(),
            physical_disk_number: None,
        });
    } else {
        let parts = parse_partitions(raw.as_ref());
        if parts.is_empty() {
            out.push(DeviceInfo {
                id: format!("img:{}:raw", path_str),
                display_name: format!("{} ({})", filename, format_bytes(total)),
                mount_point: None,
                volume_label: filename,
                fs_kind: FileSystemKind::Unknown,
                bus_kind: BusKind::ImageFile,
                is_removable: true,
                is_physical_partition: false,
                total_bytes: total,
                free_bytes: 0,
                sector_size: 512,
                base_offset: 0,
                part_length: total,
                raw_device_path: path_str,
                physical_disk_number: None,
            });
        } else {
            for p in parts {
                out.push(DeviceInfo {
                    id: format!("img:{}:{}", path_str, p.index),
                    display_name: format!(
                        "{} [P{}: {}] ({})",
                        filename,
                        p.index,
                        p.fs_kind,
                        format_bytes(p.size_bytes)
                    ),
                    mount_point: None,
                    volume_label: p.label,
                    fs_kind: p.fs_kind,
                    bus_kind: BusKind::ImageFile,
                    is_removable: true,
                    is_physical_partition: true,
                    total_bytes: p.size_bytes,
                    free_bytes: 0,
                    sector_size: 512,
                    base_offset: p.offset_bytes,
                    part_length: p.size_bytes,
                    raw_device_path: path_str.clone(),
                    physical_disk_number: None,
                });
            }
        }
    }
    Ok(out)
}

/// Prevent overwriting deleted files by checking whether destination sits on the scanned drive.
pub fn is_same_volume(source: &DeviceInfo, dest_dir: &Path) -> bool {
    let Some(ref mount) = source.mount_point else {
        return false;
    };
    let src_prefix = mount.trim_end_matches(['\\', '/']).to_ascii_uppercase();
    if src_prefix.len() < 2 {
        return false;
    }
    let abs = std::fs::canonicalize(dest_dir).unwrap_or_else(|_| dest_dir.to_path_buf());
    let mut dst_str = abs.to_string_lossy().to_ascii_uppercase();
    if let Some(stripped) = dst_str.strip_prefix("\\\\?\\") {
        dst_str = stripped.to_string();
    }
    dst_str.starts_with(&src_prefix)
}

// ======================= WINDOWS RAW DEVICE & ENUMERATION =======================

#[cfg(windows)]
mod win {
    use super::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        BusTypeAta, BusTypeAtapi, BusTypeFileBackedVirtual, BusTypeMmc, BusTypeNvme, BusTypeSas,
        BusTypeSata, BusTypeScsi, BusTypeSd, BusTypeSpaces, BusTypeUsb, CreateFileW,
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW, GetVolumeInformationW,
        ReadFile, SetFilePointerEx, FILE_ATTRIBUTE_NORMAL, FILE_BEGIN, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Ioctl::{
        PropertyStandardQuery, StorageDeviceProperty, DISK_GEOMETRY, GET_LENGTH_INFORMATION,
        IOCTL_DISK_GET_DRIVE_GEOMETRY, IOCTL_DISK_GET_LENGTH_INFO, IOCTL_STORAGE_GET_DEVICE_NUMBER,
        IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_DEVICE_DESCRIPTOR, STORAGE_DEVICE_NUMBER,
        STORAGE_PROPERTY_QUERY,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::UI::Shell::{IsUserAnAdmin, ShellExecuteW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    fn to_wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub struct WindowsRawDevice {
        handle: HANDLE,
        size: u64,
        sector_size: u32,
        io_lock: Mutex<()>,
    }

    // SAFETY: HANDLE is only accessed under `io_lock` for SetFilePointerEx + ReadFile.
    unsafe impl Send for WindowsRawDevice {}
    unsafe impl Sync for WindowsRawDevice {}

    impl WindowsRawDevice {
        pub fn open(device_path: &str, hint_sector_size: u32) -> io::Result<Self> {
            let wide = to_wide(device_path);
            let share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
            let handle = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    GENERIC_READ,
                    share,
                    null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE || handle.is_null() {
                let err = unsafe { GetLastError() };
                if err == 5 {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!(
                            "Erisim engellendi / Access denied ({device_path}). Yonetici olarak calistirin / Run as Administrator."
                        ),
                    ));
                }
                return Err(io::Error::from_raw_os_error(err as i32));
            }

            let size = query_length(handle).unwrap_or(0);
            let sector_size = query_sector_size(handle).unwrap_or(hint_sector_size.max(512));
            Ok(Self {
                handle,
                size,
                sector_size,
                io_lock: Mutex::new(()),
            })
        }
    }

    impl Drop for WindowsRawDevice {
        fn drop(&mut self) {
            if !self.handle.is_null() && self.handle != INVALID_HANDLE_VALUE {
                unsafe {
                    CloseHandle(self.handle);
                }
            }
        }
    }

    impl BlockDevice for WindowsRawDevice {
        fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
            if len == 0 || (self.size > 0 && offset >= self.size) {
                return Ok(Vec::new());
            }
            let align = self.sector_size.max(512) as u64;
            let aligned_start = (offset / align) * align;
            let req_len = if self.size > 0 {
                len.min((self.size - offset) as usize)
            } else {
                len
            };
            let aligned_end = ((offset + req_len as u64 + align - 1) / align) * align;
            let read_len = (aligned_end - aligned_start) as usize;

            let mut raw = vec![0u8; read_len];
            let mut got = 0usize;

            let _guard = self
                .io_lock
                .lock()
                .map_err(|_| io::Error::other("io lock poisoned"))?;
            unsafe {
                let mut new_pos: i64 = 0;
                if SetFilePointerEx(self.handle, aligned_start as i64, &mut new_pos, FILE_BEGIN) == 0
                {
                    return Err(io::Error::from_raw_os_error(GetLastError() as i32));
                }
                while got < read_len {
                    let chunk = (read_len - got).min(16 * 1024 * 1024);
                    let mut n: u32 = 0;
                    let ok = ReadFile(
                        self.handle,
                        raw[got..].as_mut_ptr(),
                        chunk as u32,
                        &mut n,
                        null_mut(),
                    );
                    if ok == 0 {
                        return Err(io::Error::from_raw_os_error(GetLastError() as i32));
                    }
                    if n == 0 {
                        break;
                    }
                    got += n as usize;
                }
            }
            let head = (offset - aligned_start) as usize;
            let avail = got.saturating_sub(head);
            let take = req_len.min(avail);
            Ok(raw[head..head + take].to_vec())
        }

        fn total_size(&self) -> u64 {
            self.size
        }

        fn sector_size(&self) -> u32 {
            self.sector_size
        }
    }

    fn query_length(handle: HANDLE) -> Option<u64> {
        let mut info = GET_LENGTH_INFORMATION { Length: 0 };
        let mut returned: u32 = 0;
        let ok = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_DISK_GET_LENGTH_INFO,
                null(),
                0,
                (&mut info as *mut GET_LENGTH_INFORMATION).cast(),
                std::mem::size_of::<GET_LENGTH_INFORMATION>() as u32,
                &mut returned,
                null_mut(),
            )
        };
        if ok != 0 && info.Length > 0 {
            Some(info.Length as u64)
        } else {
            None
        }
    }

    fn query_sector_size(handle: HANDLE) -> Option<u32> {
        let mut geom: DISK_GEOMETRY = unsafe { std::mem::zeroed() };
        let mut returned: u32 = 0;
        let ok = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_DISK_GET_DRIVE_GEOMETRY,
                null(),
                0,
                (&mut geom as *mut DISK_GEOMETRY).cast(),
                std::mem::size_of::<DISK_GEOMETRY>() as u32,
                &mut returned,
                null_mut(),
            )
        };
        if ok != 0 && geom.BytesPerSector >= 512 {
            Some(geom.BytesPerSector)
        } else {
            None
        }
    }

    fn query_device_number(handle: HANDLE) -> Option<u32> {
        let mut dev_num: STORAGE_DEVICE_NUMBER = unsafe { std::mem::zeroed() };
        let mut returned: u32 = 0;
        let ok = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_STORAGE_GET_DEVICE_NUMBER,
                null(),
                0,
                (&mut dev_num as *mut STORAGE_DEVICE_NUMBER).cast(),
                std::mem::size_of::<STORAGE_DEVICE_NUMBER>() as u32,
                &mut returned,
                null_mut(),
            )
        };
        if ok != 0 {
            Some(dev_num.DeviceNumber)
        } else {
            None
        }
    }

    fn query_bus_type(handle: HANDLE) -> (BusKind, bool, String) {
        let query = STORAGE_PROPERTY_QUERY {
            PropertyId: StorageDeviceProperty,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0],
        };
        let mut buf = vec![0u8; 1024];
        let mut returned: u32 = 0;
        let ok = unsafe {
            DeviceIoControl(
                handle,
                IOCTL_STORAGE_QUERY_PROPERTY,
                (&query as *const STORAGE_PROPERTY_QUERY).cast(),
                std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut returned,
                null_mut(),
            )
        };
        if ok == 0 || (returned as usize) < std::mem::size_of::<STORAGE_DEVICE_DESCRIPTOR>() {
            return (BusKind::Other, false, String::new());
        }
        let desc = unsafe { &*(buf.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR) };
        let bus = map_bus_type(desc.BusType);
        let removable = desc.RemovableMedia || matches!(bus, BusKind::Usb | BusKind::SdMmc);

        let mut model = String::new();
        if desc.ProductIdOffset != 0 && (desc.ProductIdOffset as usize) < returned as usize {
            let slice = &buf[desc.ProductIdOffset as usize..returned as usize];
            let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
            model = String::from_utf8_lossy(&slice[..end]).trim().to_string();
        }
        (bus, removable, model)
    }

    #[allow(non_upper_case_globals)]
    fn map_bus_type(bt: i32) -> BusKind {
        match bt {
            BusTypeNvme => BusKind::Nvme,
            BusTypeSata | BusTypeAta | BusTypeAtapi | BusTypeSas | BusTypeScsi => BusKind::Sata,
            BusTypeUsb => BusKind::Usb,
            BusTypeSd | BusTypeMmc => BusKind::SdMmc,
            BusTypeFileBackedVirtual | BusTypeSpaces => BusKind::Virtual,
            _ => BusKind::Other,
        }
    }

    pub fn is_elevated() -> bool {
        unsafe { IsUserAnAdmin() != 0 }
    }

    pub fn relaunch_as_admin() -> io::Result<()> {
        let exe = std::env::current_exe()?;
        let exe_w = to_wide(&exe.to_string_lossy());
        let verb_w = to_wide("runas");
        let res = unsafe {
            ShellExecuteW(
                null_mut(),
                verb_w.as_ptr(),
                exe_w.as_ptr(),
                null(),
                null(),
                SW_SHOWNORMAL,
            )
        };
        if (res as isize) <= 32 {
            return Err(io::Error::other("UAC elevation cancelled or failed"));
        }
        Ok(())
    }

    pub fn enumerate_devices() -> Vec<DeviceInfo> {
        let mut devices = Vec::new();
        let mut buf = vec![0u16; 512];
        let len = unsafe { GetLogicalDriveStringsW(buf.len() as u32, buf.as_mut_ptr()) };

        if len > 0 {
            for slice in buf[..len as usize].split(|&c| c == 0) {
                if slice.is_empty() {
                    continue;
                }
                let root = String::from_utf16_lossy(slice);
                let drive_type = unsafe { GetDriveTypeW(to_wide(&root).as_ptr()) };
                // Skip network/CDROM drives (2 = Removable, 3 = Fixed, 6 = RAMDisk)
                if drive_type != 2 && drive_type != 3 {
                    continue;
                }

                let mut vol_name = [0u16; 128];
                let mut fs_name = [0u16; 64];
                let mut serial = 0u32;
                let mut max_comp = 0u32;
                let mut flags = 0u32;
                let ok_vol = unsafe {
                    GetVolumeInformationW(
                        to_wide(&root).as_ptr(),
                        vol_name.as_mut_ptr(),
                        vol_name.len() as u32,
                        &mut serial,
                        &mut max_comp,
                        &mut flags,
                        fs_name.as_mut_ptr(),
                        fs_name.len() as u32,
                    )
                };

                let label = if ok_vol != 0 {
                    let end = vol_name.iter().position(|&c| c == 0).unwrap_or(0);
                    String::from_utf16_lossy(&vol_name[..end])
                } else {
                    String::new()
                };
                let fs_str = if ok_vol != 0 {
                    let end = fs_name.iter().position(|&c| c == 0).unwrap_or(0);
                    String::from_utf16_lossy(&fs_name[..end])
                } else {
                    String::new()
                };

                let fs_kind = match fs_str.to_ascii_uppercase().as_str() {
                    "NTFS" => FileSystemKind::Ntfs,
                    "EXFAT" => FileSystemKind::ExFat,
                    "FAT32" | "FAT" => FileSystemKind::Fat32,
                    _ => FileSystemKind::Unknown,
                };

                let mut avail: u64 = 0;
                let mut total: u64 = 0;
                let mut free: u64 = 0;
                unsafe {
                    GetDiskFreeSpaceExW(
                        to_wide(&root).as_ptr(),
                        &mut avail,
                        &mut total,
                        &mut free,
                    );
                }

                let letter = root.trim_end_matches(['\\', '/']);
                let raw_path = format!(r"\\.\{}", letter);

                let (mut bus_kind, mut is_removable, mut model, mut phys_num, mut sector_size) =
                    (BusKind::Other, drive_type == 2, String::new(), None, 512u32);

                // Probe hardware bus + device number using volume handle (no admin needed for query)
                let h = unsafe {
                    CreateFileW(
                        to_wide(&raw_path).as_ptr(),
                        0, // query-only metadata access succeeds even without admin
                        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                        null(),
                        OPEN_EXISTING,
                        FILE_ATTRIBUTE_NORMAL,
                        null_mut(),
                    )
                };
                if h != INVALID_HANDLE_VALUE && !h.is_null() {
                    let (b, rem, m) = query_bus_type(h);
                    bus_kind = b;
                    is_removable = is_removable || rem;
                    model = m;
                    phys_num = query_device_number(h);
                    if let Some(sec) = query_sector_size(h) {
                        sector_size = sec;
                    }
                    if total == 0 {
                        if let Some(l) = query_length(h) {
                            total = l;
                        }
                    }
                    unsafe {
                        CloseHandle(h);
                    }
                }

                let title_label = if label.is_empty() {
                    if model.is_empty() {
                        letter.to_string()
                    } else {
                        format!("{} ({})", letter, model)
                    }
                } else {
                    format!("{} [{}]", letter, label)
                };

                devices.push(DeviceInfo {
                    id: format!("vol:{}", letter),
                    display_name: format!("{} - {} ({})", title_label, fs_kind, format_bytes(total)),
                    mount_point: Some(root),
                    volume_label: label,
                    fs_kind,
                    bus_kind,
                    is_removable,
                    is_physical_partition: false,
                    total_bytes: total,
                    free_bytes: free,
                    sector_size,
                    base_offset: 0,
                    part_length: total,
                    raw_device_path: raw_path,
                    physical_disk_number: phys_num,
                });
            }
        }

        // Also probe PhysicalDrive0..15 (useful for unmounted / formatted / raw SD cards & USBs)
        for disk_idx in 0..16u32 {
            let phys_path = format!(r"\\.\PhysicalDrive{}", disk_idx);
            let h = unsafe {
                CreateFileW(
                    to_wide(&phys_path).as_ptr(),
                    0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    null(),
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    null_mut(),
                )
            };
            if h == INVALID_HANDLE_VALUE || h.is_null() {
                continue;
            }
            let (bus_kind, is_removable, model) = query_bus_type(h);
            let total = query_length(h).unwrap_or(0);
            let sector_size = query_sector_size(h).unwrap_or(512);
            unsafe {
                CloseHandle(h);
            }
            if total == 0 {
                continue;
            }

            let label = if model.is_empty() {
                format!("PhysicalDrive{}", disk_idx)
            } else {
                format!("Disk {} - {}", disk_idx, model)
            };

            devices.push(DeviceInfo {
                id: format!("phys:{}", disk_idx),
                display_name: format!("{} ({})", label, format_bytes(total)),
                mount_point: None,
                volume_label: model,
                fs_kind: FileSystemKind::Unknown,
                bus_kind,
                is_removable,
                is_physical_partition: true,
                total_bytes: total,
                free_bytes: 0,
                sector_size,
                base_offset: 0,
                part_length: total,
                raw_device_path: phys_path,
                physical_disk_number: Some(disk_idx),
            });
        }

        devices
    }
}

#[cfg(windows)]
pub use win::{enumerate_devices, is_elevated, relaunch_as_admin, WindowsRawDevice};

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    unsafe { libc_geteuid() == 0 }
}

#[cfg(not(windows))]
fn libc_geteuid() -> u32 {
    // Avoid requiring libc crate: read /proc/self/status Uid on Linux
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("Uid:") {
                if let Some(euid) = rest.split_whitespace().nth(1) {
                    return euid.parse::<u32>().unwrap_or(1000);
                }
            }
        }
    }
    1000
}

#[cfg(not(windows))]
pub fn relaunch_as_admin() -> io::Result<()> {
    Err(io::Error::other("Run with sudo on Linux/macOS"))
}

#[cfg(not(windows))]
pub fn enumerate_devices() -> Vec<DeviceInfo> {
    let mut devices = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/sys/block") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("loop") || name.starts_with("ram") {
                continue;
            }
            let dev_path = format!("/dev/{}", name);
            let sectors = std::fs::read_to_string(entry.path().join("size"))
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok())
                .unwrap_or(0);
            let total = sectors * 512;
            if total == 0 {
                continue;
            }
            let removable = std::fs::read_to_string(entry.path().join("removable"))
                .ok()
                .map(|s| s.trim() == "1")
                .unwrap_or(false);
            let bus_kind = if name.starts_with("nvme") {
                BusKind::Nvme
            } else if name.starts_with("mmc") {
                BusKind::SdMmc
            } else if removable {
                BusKind::Usb
            } else {
                BusKind::Sata
            };
            devices.push(DeviceInfo {
                id: dev_path.clone(),
                display_name: format!("{} ({})", dev_path, format_bytes(total)),
                mount_point: None,
                volume_label: name,
                fs_kind: FileSystemKind::Unknown,
                bus_kind,
                is_removable: removable,
                is_physical_partition: true,
                total_bytes: total,
                free_bytes: 0,
                sector_size: 512,
                base_offset: 0,
                part_length: total,
                raw_device_path: dev_path,
                physical_disk_number: None,
            });
        }
    }
    devices
}
