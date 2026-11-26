# OpenRecover

**Free, fast, open-source data recovery for SSDs, HDDs, USB flash drives, SD/microSD cards, and disk images.**  
Zero paywalls. Zero 500 MB / 4,000-file limits. Full folder-tree reconstruction + deep signature carving.

---

## Features / Özellikler

- **Multi-Device Support:** Internal SSD/HDD (`NVMe`, `SATA`), removable `USB` flash drives, `SD/microSD` memory cards, raw unmounted `PhysicalDrive` partitions (MBR & GPT), and `.img` / `.dd` / `.raw` disk images.
- **Multi-Filesystem Engine (Pure Rust):**
  - **NTFS:** High-speed buffered `$MFT` parser with USA fixup, `$ATTRIBUTE_LIST`, resident inline data, variable-length signed dataruns, sparse & LZNT1 compressed streams, and deleted directory `SequenceNumber + 1` folder-tree reconstruction.
  - **exFAT:** Deleted `0x05 / 0x40 / 0x41` directory entries with UTF-16LE filenames and recursive deleted-subfolder traversal.
  - **FAT32:** Deleted `0xE5` entries with Long File Name (`LFN`) UCS-2 reconstruction and FAT-chain / contiguous recovery.
  - **Deep File Carving (RAW Signatures):** Exact boundary/header carving for `JPG` (skipping embedded EXIF thumbnails), `PNG` (chunk-accurate `IEND`), `GIF`, `BMP`, `WEBP`, `MP4/MOV`, `WAV`, `AVI`, `PDF`, `ZIP/DOCX/XLSX`, and `SQLite`.
- **SSD TRIM Detection:** Automatically checks file headers and separates all-zero TRIMmed files into `_TRIM_Zeroed` during recovery so clean folders stay reliable.
- **Live Preview Before Recovery:** Renders recovered photos (`PNG`, `JPG`, `GIF`, `BMP`, `WEBP`), UTF-8 text/JSON/code files, and Hex/ASCII headers inside the UI.
- **Same-Drive Safety Guard:** Blocks recovering files onto the same source volume you are scanning so you never overwrite deleted data.
- **Bilingual UI:** Switch live between **Türkçe** and **English**.
- **Bonus CLI (`openrecover-cli`):** Scriptable command-line tool for headless/automation workflows.

---

## Building from Source

```bash
cargo test
cargo build --release
```

Binaries are output to `target/release/openrecover.exe` and `target/release/openrecover-cli.exe`.

> **Windows Note:** Reading live `C:`, `D:`, USB, or SD card block devices requires Administrator privileges (click **"Run as Admin / Yönetici Olarak Aç"** in the top bar). Opening `.img` / `.dd` disk image files works without Admin privileges — you can also launch `openrecover.exe path\to\disk.img --scan` directly.

---

## CLI Usage

```bash
# List detected volumes & physical drives
openrecover-cli list

# Scan C: for deleted AppData/Roaming files and extract safely to D:\Restored
openrecover-cli scan C: --mode quick --filter AppData/Roaming --out D:\Restored

# Combined tree + deep carving on an SD card disk image
openrecover-cli scan sdcard.img --mode combined --out D:\SD_Recovered
```

---

## License

[MIT](LICENSE)
