use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::core::device::BlockDevice;
use crate::core::types::{
    FileCategory, FileHealth, FilePayload, FileSystemKind, RecoveredEntry, ScanEvent,
};

const SCAN_BLOCK: usize = 2 * 1024 * 1024; // 2 MB aligned read chunks
const MAX_CARVE_IMAGE: u64 = 50 * 1024 * 1024;
const MAX_CARVE_DOC: u64 = 128 * 1024 * 1024;
const MAX_CARVE_VIDEO: u64 = 512 * 1024 * 1024;

fn is_sector_zero(sec: &[u8]) -> bool {
    let (prefix, aligned, suffix) = unsafe { sec.align_to::<u64>() };
    prefix.iter().all(|&b| b == 0)
        && aligned.iter().all(|&w| w == 0)
        && suffix.iter().all(|&b| b == 0)
}

/// Carve PNG exact size by walking chunks up to `IEND`.
fn carve_png_size(dev: &dyn BlockDevice, start: u64) -> Option<u64> {
    let hdr = dev.read_at(start, 33).ok()?;
    if hdr.len() < 33 || &hdr[0..8] != b"\x89PNG\r\n\x1a\n" || &hdr[12..16] != b"IHDR" {
        return None;
    }
    let mut pos = 8u64;
    for _ in 0..8192 {
        let ch = dev.read_at(start + pos, 8).ok()?;
        if ch.len() < 8 {
            return None;
        }
        let len = u32::from_be_bytes(ch[0..4].try_into().unwrap()) as u64;
        let tag = &ch[4..8];
        if !tag.iter().all(|&b| b.is_ascii_alphabetic()) || len > MAX_CARVE_IMAGE {
            return None;
        }
        pos += 12 + len; // 4 len + 4 tag + payload + 4 crc
        if pos > MAX_CARVE_IMAGE {
            return None;
        }
        if tag == b"IEND" && len == 0 {
            return Some(pos);
        }
    }
    None
}

/// Carve JPEG size while skipping EXIF embedded thumbnails (`APPn`) and walking until SOS -> EOI.
fn carve_jpeg_size(dev: &dyn BlockDevice, start: u64) -> Option<u64> {
    let head = dev.read_at(start, 1024).ok()?;
    if head.len() < 16 || head[0] != 0xFF || head[1] != 0xD8 || head[2] != 0xFF {
        return None;
    }
    let mut pos = 2usize;
    let mut buf = head;
    let mut loaded_off = start;
    let mut in_entropy = false;

    loop {
        while pos + 8 > buf.len() {
            let abs = loaded_off + pos as u64;
            if abs - start > MAX_CARVE_IMAGE {
                return None;
            }
            let next = dev.read_at(abs, 65536).ok()?;
            if next.is_empty() {
                return None;
            }
            loaded_off = abs;
            buf = next;
            pos = 0;
        }

        if !in_entropy {
            if buf[pos] != 0xFF {
                return None;
            }
            while pos < buf.len() && buf[pos] == 0xFF {
                pos += 1;
            }
            if pos >= buf.len() {
                continue;
            }
            let marker = buf[pos];
            pos += 1;
            if marker == 0xD9 {
                return Some((loaded_off + pos as u64) - start);
            }
            if marker == 0x00 || (0xD0..=0xD8).contains(&marker) {
                continue;
            }
            if pos + 2 > buf.len() {
                continue;
            }
            let seg_len = u16::from_be_bytes([buf[pos], buf[pos + 1]]) as usize;
            if seg_len < 2 {
                return None;
            }
            pos += seg_len;
            if marker == 0xDA {
                // Start of Scan -> entropy-coded data follows until 0xFF 0xD9 (or RST/marker)
                in_entropy = true;
            }
        } else {
            // Fast search for 0xFF byte inside entropy stream
            if let Some(rel) = buf[pos..].iter().position(|&b| b == 0xFF) {
                pos += rel + 1;
                if pos >= buf.len() {
                    // Back up 1 byte so 0xFF stays at start of next chunk
                    pos -= 1;
                    let abs = loaded_off + pos as u64;
                    if abs - start > MAX_CARVE_IMAGE {
                        return None;
                    }
                    let next = dev.read_at(abs, 65536).ok()?;
                    if next.len() < 2 {
                        return None;
                    }
                    loaded_off = abs;
                    buf = next;
                    pos = 1;
                }
                let next_b = buf[pos];
                pos += 1;
                if next_b == 0xD9 {
                    return Some((loaded_off + pos as u64) - start);
                }
                // 0x00 (byte stuffing), 0xFF (padding), 0xD0..0xD7 (RSTn) remain in entropy
                if next_b != 0x00 && next_b != 0xFF && !(0xD0..=0xD7).contains(&next_b) {
                    // Back up onto marker if another scan header appears (progressive JPEG)
                    pos -= 2;
                    in_entropy = false;
                }
            } else {
                let abs = loaded_off + buf.len() as u64;
                if abs - start > MAX_CARVE_IMAGE {
                    return None;
                }
                let next = dev.read_at(abs, 65536).ok()?;
                if next.is_empty() {
                    return None;
                }
                loaded_off = abs;
                buf = next;
                pos = 0;
            }
        }
    }
}

fn carve_riff_size(sec: &[u8]) -> Option<(&'static str, &'static str, u64)> {
    if sec.len() < 12 || &sec[0..4] != b"RIFF" {
        return None;
    }
    let body_sz = u32::from_le_bytes(sec[4..8].try_into().unwrap()) as u64;
    if body_sz < 16 || body_sz + 8 > MAX_CARVE_VIDEO {
        return None;
    }
    let kind = &sec[8..12];
    match kind {
        b"WEBP" => Some(("webp", "WEBP", body_sz + 8)),
        b"WAVE" => Some(("wav", "WAV", body_sz + 8)),
        b"AVI " => Some(("avi", "AVI", body_sz + 8)),
        _ => None,
    }
}

fn carve_mp4_size(dev: &dyn BlockDevice, start: u64) -> Option<u64> {
    let hdr = dev.read_at(start, 32).ok()?;
    if hdr.len() < 16 || &hdr[4..8] != b"ftyp" {
        return None;
    }
    let mut pos = 0u64;
    let mut saw_moov_or_mdat = false;
    for _ in 0..256 {
        let b = dev.read_at(start + pos, 16).ok()?;
        if b.len() < 8 {
            break;
        }
        let sz32 = u32::from_be_bytes(b[0..4].try_into().unwrap()) as u64;
        let box_type = &b[4..8];
        if !box_type.iter().all(|&c| (0x20..=0x7E).contains(&c)) {
            break;
        }
        let atom_sz = if sz32 == 1 && b.len() == 16 {
            u64::from_be_bytes(b[8..16].try_into().unwrap())
        } else {
            sz32
        };
        if atom_sz < 8 || pos + atom_sz > MAX_CARVE_VIDEO {
            break;
        }
        if box_type == b"moov" || box_type == b"mdat" {
            saw_moov_or_mdat = true;
        }
        pos += atom_sz;
    }
    if saw_moov_or_mdat && pos >= 64 {
        Some(pos)
    } else {
        None
    }
}

fn carve_sqlite_size(sec: &[u8], total_dev: u64, start: u64) -> Option<u64> {
    if sec.len() < 64 || &sec[0..16] != b"SQLite format 3\0" {
        return None;
    }
    let mut page_sz = u16::from_be_bytes([sec[16], sec[17]]) as u64;
    if page_sz == 1 {
        page_sz = 65536;
    }
    if !page_sz.is_power_of_two() || page_sz < 512 {
        return None;
    }
    let page_cnt = u32::from_be_bytes(sec[28..32].try_into().unwrap()) as u64;
    let sz = page_sz.saturating_mul(page_cnt);
    if sz >= 512 && start + sz <= total_dev && sz <= MAX_CARVE_DOC {
        Some(sz)
    } else {
        None
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn carve_marker_end(dev: &dyn BlockDevice, start: u64, end_marker: &[u8], extra_after: usize, max_sz: u64) -> Option<u64> {
    let mut pos = 0u64;
    let chunk_sz = 256 * 1024usize;
    let overlap = end_marker.len() + 16;
    while pos < max_sz {
        let read_len = chunk_sz.min((max_sz - pos) as usize);
        let buf = dev.read_at(start + pos, read_len).ok()?;
        if buf.len() < end_marker.len() {
            break;
        }
        if let Some(hit) = find_bytes(&buf, end_marker) {
            return Some(pos + hit as u64 + end_marker.len() as u64 + extra_after as u64);
        }
        if buf.len() < read_len {
            break;
        }
        pos += (buf.len().saturating_sub(overlap)) as u64;
    }
    None
}

fn carve_zip_size(dev: &dyn BlockDevice, start: u64) -> Option<u64> {
    // Locate End of Central Directory PK\x05\x06 and add comment length at +20..22
    let mut pos = 0u64;
    let chunk_sz = 256 * 1024usize;
    while pos < MAX_CARVE_DOC {
        let buf = dev.read_at(start + pos, chunk_sz).ok()?;
        if buf.len() < 22 {
            break;
        }
        if let Some(hit) = find_bytes(&buf, b"PK\x05\x06") {
            if hit + 22 <= buf.len() {
                let comment_len = u16::from_le_bytes([buf[hit + 20], buf[hit + 21]]) as u64;
                return Some(pos + hit as u64 + 22 + comment_len);
            }
        }
        if buf.len() < chunk_sz {
            break;
        }
        pos += (buf.len() - 24) as u64;
    }
    None
}

/// Deep sector-aligned signature scanner across any disk, partition, or raw `.img` file.
pub fn scan_carve(
    dev: &Arc<dyn BlockDevice>,
    cancel: &AtomicBool,
    progress_cb: &mut dyn FnMut(ScanEvent),
) -> io::Result<(Vec<RecoveredEntry>, Vec<String>)> {
    let total = dev.total_size();
    let step = dev.sector_size().max(512) as usize;
    let mut entries = Vec::new();
    let warnings = Vec::new();
    let mut offset = 0u64;
    let mut id_counter = 1_000_000_000u64;
    let mut file_seq = 1usize;

    while offset < total {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let read_len = SCAN_BLOCK.min((total - offset) as usize);
        let buf = dev.read_at(offset, read_len)?;
        if buf.is_empty() {
            break;
        }

        let mut in_buf = 0usize;
        while in_buf + 64 <= buf.len() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let sec = &buf[in_buf..];
            let sec_slice = &sec[..sec.len().min(step)];
            if is_sector_zero(sec_slice) {
                in_buf += step;
                continue;
            }

            let abs_off = offset + in_buf as u64;
            let detected: Option<(&'static str, &'static str, u64)> =
                if sec.starts_with(b"\xFF\xD8\xFF") {
                    carve_jpeg_size(dev.as_ref(), abs_off).map(|s| ("jpg", "JPG", s))
                } else if sec.starts_with(b"\x89PNG\r\n\x1a\n") {
                    carve_png_size(dev.as_ref(), abs_off).map(|s| ("png", "PNG", s))
                } else if sec.starts_with(b"GIF87a") || sec.starts_with(b"GIF89a") {
                    carve_marker_end(dev.as_ref(), abs_off, b"\x00;", 0, MAX_CARVE_IMAGE)
                        .map(|s| ("gif", "GIF", s))
                } else if sec.starts_with(b"BM") && sec.len() >= 18 {
                    let bmp_sz = u32::from_le_bytes(sec[2..6].try_into().unwrap()) as u64;
                    let dib_sz = u32::from_le_bytes(sec[14..18].try_into().unwrap());
                    if matches!(dib_sz, 12 | 40 | 52 | 56 | 108 | 124)
                        && bmp_sz > 64
                        && bmp_sz <= MAX_CARVE_IMAGE
                        && abs_off + bmp_sz <= total
                    {
                        Some(("bmp", "BMP", bmp_sz))
                    } else {
                        None
                    }
                } else if sec.starts_with(b"RIFF") {
                    carve_riff_size(sec)
                } else if sec.len() >= 12 && &sec[4..8] == b"ftyp" {
                    carve_mp4_size(dev.as_ref(), abs_off).map(|s| ("mp4", "MP4", s))
                } else if sec.starts_with(b"%PDF-") {
                    carve_marker_end(dev.as_ref(), abs_off, b"%%EOF", 2, MAX_CARVE_DOC)
                        .map(|s| ("pdf", "PDF", s))
                } else if sec.starts_with(b"PK\x03\x04") {
                    carve_zip_size(dev.as_ref(), abs_off).map(|s| ("zip", "ZIP", s))
                } else if sec.starts_with(b"SQLite format 3\0") {
                    carve_sqlite_size(sec, total, abs_off).map(|s| ("sqlite", "SQLite", s))
                } else {
                    None
                };

            if let Some((ext, folder, size)) = detected {
                let name = format!("carved_{:06}.{}", file_seq, ext);
                let rel_path = format!("_DeepCarve/{}/{}", folder, name);
                let folder_path = format!("_DeepCarve/{}", folder);
                entries.push(RecoveredEntry {
                    id: id_counter,
                    name,
                    rel_path_lower: rel_path.to_ascii_lowercase(),
                    rel_path,
                    folder_path,
                    ext: ext.to_string(),
                    size,
                    modified_unix: 0,
                    fs_kind: FileSystemKind::RawCarved,
                    health: FileHealth::Intact,
                    category: FileCategory::from_ext(ext),
                    source_desc: format!("Carved @ 0x{:X}", abs_off),
                    payload: FilePayload::Extents {
                        runs: vec![(Some(abs_off), size)],
                        unit_bytes: 1,
                        file_size: size,
                        compressed: false,
                        compression_unit_bytes: 0,
                    },
                });
                id_counter += 1;
                file_seq += 1;

                // Advance past the carved file aligned to sector boundary
                let advance = (((size as usize) + step - 1) / step) * step;
                if in_buf + advance < buf.len() {
                    in_buf += advance.max(step);
                } else {
                    offset = abs_off + advance as u64;
                    in_buf = buf.len();
                    break;
                }
            } else {
                in_buf += step;
            }
        }

        if in_buf >= buf.len() && offset + buf.len() as u64 > offset {
            let next_off = offset + buf.len() as u64;
            if next_off > offset {
                offset = next_off;
            }
        }

        progress_cb(ScanEvent::Progress {
            stage: "Deep Carving".to_string(),
            fraction: (offset.min(total) as f32 / total.max(1) as f32),
            found_count: entries.len(),
            detail: format!("{}/{} MB", offset / (1024 * 1024), total / (1024 * 1024)),
        });
    }

    Ok((entries, warnings))
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
    fn test_carver_extracts_png_jpeg_and_pdf_with_exact_sizes() {
        let mut disk = vec![0u8; 64 * 512];

        // 1) PNG at sector 4 (offset 2048)
        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&[0u8; 13]);
        png.extend_from_slice(&[0u8; 4]); // CRC
        png.extend_from_slice(&0u32.to_be_bytes());
        png.extend_from_slice(b"IEND");
        png.extend_from_slice(&[0xAE, 0x42, 0x60, 0x82]);
        disk[2048..2048 + png.len()].copy_from_slice(&png);

        // 2) JPEG with embedded APP1 thumbnail containing FF D8..FF D9 at sector 10 (5120)
        let mut jpg = Vec::new();
        jpg.extend_from_slice(&[0xFF, 0xD8]);
        // APP1 segment of length 10 containing fake embedded thumbnail FF D8 FF D9
        jpg.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x0A, b'E', b'x', 0xFF, 0xD8, 0xFF, 0xD9, 0x00, 0x00]);
        // SOS segment length 4 + entropy payload + stuffed FF 00 + real EOI FF D9
        jpg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x04, 0x01, 0x02, 0x11, 0xFF, 0x00, 0x22, 0xFF, 0xD9]);
        disk[5120..5120 + jpg.len()].copy_from_slice(&jpg);

        // 3) PDF at sector 20 (10240)
        let pdf = b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\n%%EOF\r\n";
        disk[10240..10240 + pdf.len()].copy_from_slice(pdf);

        let dev: Arc<dyn BlockDevice> = Arc::new(MemDev(disk));
        let cancel = AtomicBool::new(false);
        let (entries, _) = scan_carve(&dev, &cancel, &mut |_| {}).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].ext, "png");
        assert_eq!(entries[0].size, png.len() as u64);
        assert_eq!(crate::core::recover::read_entry_bytes(dev.as_ref(), &entries[0], 1024).unwrap(), png);
        assert_eq!(entries[1].ext, "jpg");
        assert_eq!(entries[1].size, jpg.len() as u64);
        assert_eq!(crate::core::recover::read_entry_bytes(dev.as_ref(), &entries[1], 1024).unwrap(), jpg);
        assert_eq!(entries[2].ext, "pdf");
        assert_eq!(entries[2].size, pdf.len() as u64);
        assert_eq!(crate::core::recover::read_entry_bytes(dev.as_ref(), &entries[2], 1024).unwrap(), pdf);
    }
}
