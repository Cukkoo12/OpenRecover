# Microsoft Store Submission Guide — OpenRecover

This guide summarizes everything prepared in this repository for publishing **OpenRecover** to the Microsoft Store via **Partner Center** (`https://storedeveloper.microsoft.com`).

---

## 1. Register & Reserve App Name (Partner Center)

1. Sign in at https://storedeveloper.microsoft.com with your personal Microsoft Account (free individual developer registration; requires government photo ID + selfie verification).
2. Go to **Apps and games** → **New app** → **MSIX or PWA app**.
3. Reserve the name **`OpenRecover`**.
4. Open **Product management → Product identity** and copy:
   - **`Package/Identity/Name`**
   - **`Package/Identity/Publisher`** (`CN=...`)
   - **`Package/Properties/PublisherDisplayName`**

---

## 2. Build Unsigned Store `.msix`

Run from the repository root (`D:\Ai\OpenRecover`) substituting your Partner Center values:

```powershell
.\packaging\msix\build_msix.ps1 `
    -IdentityName "<Package/Identity/Name>" `
    -Publisher "<CN=...>" `
    -PublisherDisplayName "<PublisherDisplayName>" `
    -Version "0.1.0.0"
```

Output: `dist\OpenRecover-0.1.0.0-x64.msix` (**unsigned**, as required — Microsoft signs Store packages with Microsoft's Store certificate automatically after certification).

> Note: Microsoft Store requires the 4th version segment (revision) to stay `0` (e.g., `0.1.0.0`, `0.2.0.0`).

---

## 3. Store Listing Fields & Assets (Partner Center)

- **Category:** `Utilities & tools` → `File managers` (or `System utilities`)
- **Pricing & Availability:** Free (`$0`), All Markets
- **Privacy Policy URL:**  
  `https://github.com/Cukkoo12/OpenRecover/blob/main/PRIVACY.md`
- **Website:**  
  `https://github.com/Cukkoo12/OpenRecover`
- **Support Contact / URL:**  
  `https://github.com/Cukkoo12/OpenRecover/issues`
- **License Terms:**  
  `https://github.com/Cukkoo12/OpenRecover/blob/main/LICENSE`
- **Store Logo (300x300 PNG):**  
  Upload `packaging/store/StoreLogo_300x300.png`
- **Desktop Screenshots (1493x972 PNG, satisfies >= 1366x768 minimum):**
  - English listing (`en-US`): `docs/screenshots/home_en.png`, `docs/screenshots/results_en.png`
  - Turkish listing (`tr-TR`): `docs/screenshots/home_tr.png`, `docs/screenshots/results_tr.png`

### Listing Descriptions (Copy-Paste Ready)

#### English (`en-US`)
**Short description:**  
Fast, free, open-source file recovery for SSDs, HDDs, USB drives, SD cards, and disk images — with zero paywalls or file-size limits.

**Description:**  
OpenRecover is a free, open-source, 100% offline data recovery application built in Pure Rust. Unlike freemium recovery tools capped at 500 MB or 4,000 files, OpenRecover restores files with full folder hierarchy and zero limits.

Key capabilities:
- NTFS ($MFT buffered scanner, deleted directory reconstruction, resident files, sparse & LZNT1 compressed streams)
- exFAT & FAT32 (including UCS-2 Long File Names)
- Deep Sector Carving for JPG, PNG, GIF, BMP, WEBP, MP4/MOV, WAV, AVI, PDF, ZIP/DOCX/XLSX, and SQLite
- Live pre-recovery preview (images, UTF-8 text, hex dump)
- Built-in same-drive write lock preventing overwriting the scanned source drive
- Full CLI included (`openrecover-cli.exe` via Windows App Execution Alias)
- Bilingual interface: English & Turkish (`EN | TR`)

#### Türkçe (`tr-TR`)
**Kısa açıklama:**  
SSD, HDD, USB bellek, SD kart ve disk imajları için ücretsiz, kotasız ve açık kaynaklı dosya kurtarma uygulaması.

**Açıklama:**  
OpenRecover; tamamen Pure Rust ile geliştirilmiş, %100 çevrimdışı çalışan ücretsiz ve açık kaynaklı bir veri kurtarma uygulamasıdır. 500 MB veya dosya sayısı sınırı olmadan klasör yapısını koruyarak kurtarma yapar.

Özellikler:
- NTFS ($MFT tamponlu hızlı tarama, silinen klasör ağacı, resident & LZNT1 sıkıştırılmış dosyalar)
- exFAT ve FAT32 (uzun dosya adları dahil)
- Derin Tarama: JPG, PNG, GIF, BMP, WEBP, MP4/MOV, WAV, AVI, PDF, ZIP/DOCX/XLSX, SQLite
- Kurtarma öncesi canlı önizleme (görsel, metin, hex dökümü)
- Taranan diskin üzerine yazmayı engelleyen koruma kilidi
- Dahili komut satırı aracı (`openrecover-cli`)
- Çift dil desteği: Türkçe ve İngilizce (`TR | EN`)

---

## 4. `runFullTrust` Justification & Notes for Certification

When Partner Center displays the **Restricted capability (`runFullTrust`)** prompt on **Submission options**, paste this explanation:

> OpenRecover is a classic Win32 desktop data recovery utility packaged via MSIX (`openrecover.exe` GUI and `openrecover-cli.exe` console alias). It declares `runFullTrust` solely to perform local, read-only filesystem and disk-image recovery operations and save restored files to user-chosen destination folders outside the app container. The application launches as a standard user with zero network activity (`internetClient` is not declared) and collects no user data. Disk image files (`.img`, `.dd`, `.raw`, `.bin`) can be scanned immediately without elevation; only reading live system/USB block devices on Windows prompts standard on-demand UAC elevation when explicitly initiated by the user.

### Notes for Certification (Testers)
> OpenRecover runs 100% offline with no sign-in required. To test immediately without Administrator privileges, click **"Open Disk Image..."** in the top bar and select any `.img`, `.dd`, `.raw`, or `.bin` disk image file (or pass an image path to `openrecover-cli scan <image.img> --mode combined --out <dest>`). Standard-user live physical drive recovery requires Windows Administrator privileges for raw volume read access (`\\.\X:`), which the user triggers manually via the **"Run as Administrator"** button.
