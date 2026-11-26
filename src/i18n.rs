#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Tr,
    En,
}

impl Lang {
    pub fn is_tr(self) -> bool {
        matches!(self, Self::Tr)
    }

    pub fn title_admin_ok(self) -> &'static str {
        if self.is_tr() { "Yönetici" } else { "Admin" }
    }

    pub fn btn_elevate(self) -> &'static str {
        if self.is_tr() { "Yönetici Olarak Aç" } else { "Run as Admin" }
    }

    pub fn btn_refresh(self) -> &'static str {
        if self.is_tr() { "Yenile" } else { "Refresh" }
    }

    pub fn btn_open_img(self) -> &'static str {
        if self.is_tr() { "İmaj Aç (.img)" } else { "Open Image" }
    }

    pub fn open_img_card(self) -> &'static str {
        if self.is_tr() {
            "+ Disk İmajı Seç (.img / .dd / .raw)"
        } else {
            "+ Select Disk Image (.img / .dd / .raw)"
        }
    }

    pub fn home_title(self) -> &'static str {
        if self.is_tr() {
            "Hangi diskten veri kurtarmak istersiniz?"
        } else {
            "Choose a source to recover"
        }
    }

    pub fn home_subtitle(self) -> &'static str {
        if self.is_tr() {
            "SSD, HDD, USB bellek, SD kart ve disk imajlarında verilerin üzerine yazmadan güvenli tarama."
        } else {
            "Safe read-only recovery across SSDs, HDDs, USB drives, SD cards & disk images."
        }
    }

    pub fn sec_removable(self) -> &'static str {
        if self.is_tr() { "USB, SD Kart ve İmajlar" } else { "USB, SD Card & Images" }
    }

    pub fn sec_fixed(self) -> &'static str {
        if self.is_tr() { "Dahili Diskler" } else { "Internal Drives" }
    }

    pub fn sec_physical(self) -> &'static str {
        if self.is_tr() {
            "Ham fiziksel bölümleri de göster"
        } else {
            "Show raw physical partitions"
        }
    }

    pub fn sec_scan_mode(self) -> &'static str {
        if self.is_tr() { "Tarama Modu" } else { "Scan Method" }
    }

    pub fn mode_quick(self) -> &'static str {
        if self.is_tr() { "Hızlı Tarama" } else { "Quick Scan" }
    }

    pub fn mode_quick_hint(self) -> &'static str {
        if self.is_tr() {
            "Silinmiş dosya ve klasörleri dosya sisteminden (NTFS / exFAT / FAT32) saniyeler içinde bulur."
        } else {
            "Finds deleted files & folder trees from NTFS, exFAT & FAT32 metadata in seconds."
        }
    }

    pub fn mode_carve(self) -> &'static str {
        if self.is_tr() { "Derin Tarama" } else { "Deep Scan" }
    }

    pub fn mode_carve_hint(self) -> &'static str {
        if self.is_tr() {
            "Sektör sektör dosya imzalarını tarar; biçimlendirilmiş veya hasarlı disklerde önerilir."
        } else {
            "Sector-level signature scan; best for formatted or damaged volumes."
        }
    }

    pub fn mode_combined(self) -> &'static str {
        if self.is_tr() { "Hızlı + Derin" } else { "Quick + Deep" }
    }

    pub fn mode_combined_hint(self) -> &'static str {
        if self.is_tr() {
            "Önce klasör yapısını çıkarır, ardından derin sektör taramasıyla kalan dosyaları bulur."
        } else {
            "Runs fast folder-tree recovery followed by sector-level deep carving in one pass."
        }
    }

    pub fn btn_start_scan(self) -> &'static str {
        if self.is_tr() { "Taramayı Başlat  →" } else { "Start Scan  →" }
    }

    pub fn btn_stop_scan(self) -> &'static str {
        if self.is_tr() { "Durdur" } else { "Stop" }
    }

    pub fn btn_back_sources(self) -> &'static str {
        if self.is_tr() { "← Sürücüler" } else { "← Sources" }
    }

    pub fn btn_back_results(self) -> &'static str {
        if self.is_tr() { "Sonuçlara Dön →" } else { "Back to Results →" }
    }

    pub fn btn_folders(self) -> &'static str {
        if self.is_tr() { "Klasörler" } else { "Folders" }
    }

    pub fn root_folder_name(self) -> &'static str {
        if self.is_tr() { "Tüm Klasörler" } else { "All Folders" }
    }

    pub fn search_hint(self) -> &'static str {
        if self.is_tr() {
            "Dosya veya klasör ara..."
        } else {
            "Search files or paths..."
        }
    }

    pub fn hide_trimmed(self) -> &'static str {
        if self.is_tr() { "TRIM Gizle" } else { "Hide TRIM" }
    }

    pub fn btn_select_visible(self) -> &'static str {
        if self.is_tr() { "Görünenleri Seç" } else { "Select Shown" }
    }

    pub fn btn_clear_sel(self) -> &'static str {
        if self.is_tr() { "Temizle" } else { "Clear" }
    }

    pub fn folders_title(self) -> &'static str {
        if self.is_tr() { "Klasör Ağacı" } else { "Folders" }
    }

    pub fn col_name(self) -> &'static str {
        if self.is_tr() { "Dosya Adı" } else { "Name" }
    }

    pub fn col_size(self) -> &'static str {
        if self.is_tr() { "Boyut" } else { "Size" }
    }

    pub fn col_health(self) -> &'static str {
        if self.is_tr() { "Durum" } else { "Health" }
    }

    pub fn col_source(self) -> &'static str {
        if self.is_tr() { "Kaynak" } else { "Source" }
    }

    pub fn col_path(self) -> &'static str {
        if self.is_tr() { "Klasör Yolu" } else { "Path" }
    }

    pub fn preview_title(self) -> &'static str {
        if self.is_tr() { "Önizleme" } else { "Preview" }
    }

    pub fn dest_label(self) -> &'static str {
        if self.is_tr() { "Hedef:" } else { "Save to:" }
    }

    pub fn btn_browse(self) -> &'static str {
        if self.is_tr() { "Klasör Seç" } else { "Browse..." }
    }

    pub fn separate_trim_opt(self) -> &'static str {
        if self.is_tr() { "Boş (TRIM) dosyaları ayır" } else { "Separate TRIM" }
    }

    pub fn btn_recover(self) -> &'static str {
        if self.is_tr() { "Kurtar" } else { "Recover" }
    }

    pub fn same_drive_warning(self) -> &'static str {
        if self.is_tr() {
            "Uyarı: Hedef klasör taranan sürücü ile aynı diskte olamaz!"
        } else {
            "Warning: Destination cannot be on the scanned source drive!"
        }
    }

    pub fn admin_hint(self) -> &'static str {
        if self.is_tr() {
            "Canlı C:/D:/USB/SD taraması için Yönetici yetkisi gerekir; .img dosyaları doğrudan açılır."
        } else {
            "Live C:/D:/USB/SD volumes require Admin privileges; disk images (.img) open without Admin."
        }
    }

    pub fn btn_open_dest(self) -> &'static str {
        if self.is_tr() { "Klasörü Aç" } else { "Open Folder" }
    }

    pub fn empty_title(self) -> &'static str {
        if self.is_tr() {
            "Silinmiş dosya bulunamadı"
        } else {
            "No deleted files found"
        }
    }

    pub fn empty_hint(self) -> &'static str {
        if self.is_tr() {
            "Dosya sistemi kaydı silinmişse 'Derin Tarama' modunu deneyebilir veya başka bir kaynak seçebilirsiniz."
        } else {
            "Try 'Deep Scan' if filesystem records were overwritten, or pick another source drive."
        }
    }

    pub fn starting_stage(self) -> &'static str {
        if self.is_tr() { "Tarama başlatılıyor..." } else { "Starting scan..." }
    }

    pub fn err_open_reader(self) -> &'static str {
        if self.is_tr() {
            "Kaynak disk okuyucusu açılamadı"
        } else {
            "Cannot open source device reader"
        }
    }

    pub fn err_no_reader(self) -> &'static str {
        if self.is_tr() {
            "Aktif disk okuyucu bulunamadı"
        } else {
            "No active device reader"
        }
    }

    pub fn localize_stage(self, stage: &str) -> String {
        if !self.is_tr() {
            return match stage {
                "MFT" => "Reading $MFT".to_string(),
                "NTFS Tree" => "Building NTFS Tree".to_string(),
                "Deep Carving" => "Deep Scan".to_string(),
                "exFAT" => "Scanning exFAT".to_string(),
                "FAT32" => "Scanning FAT32".to_string(),
                _ => stage.to_string(),
            };
        }
        match stage {
            "MFT" => "$MFT Kayıtları Okunuyor".to_string(),
            "NTFS Tree" => "NTFS Klasör Ağacı Kuruluyor".to_string(),
            "Deep Carving" => "Derin Tarama Yapılıyor".to_string(),
            "exFAT" => "exFAT Dizinleri Taranıyor".to_string(),
            "FAT32" => "FAT32 Dizinleri Taranıyor".to_string(),
            _ => stage.to_string(),
        }
    }

    pub fn localize_detail(self, detail: &str) -> String {
        if !self.is_tr() {
            return detail.to_string();
        }
        if let Some(num) = detail.strip_suffix(" dirs") {
            return format!("{} klasör", num);
        }
        detail.to_string()
    }

    pub fn localize_warning(self, warning: &str) -> String {
        if !self.is_tr() {
            return warning.to_string();
        }
        match warning {
            "No deleted $MFT records with intact data streams were found." => {
                "Sağlam veri akışına sahip silinmiş $MFT kaydı bulunamadı.".to_string()
            }
            "No deleted exFAT directory entries found; try Deep Carving." => {
                "Silinmiş exFAT dizin kaydı bulunamadı; Derin Tarama modunu deneyebilirsiniz.".to_string()
            }
            "No deleted FAT32 directory entries found; try Deep Carving." => {
                "Silinmiş FAT32 dizin kaydı bulunamadı; Derin Tarama modunu deneyebilirsiniz.".to_string()
            }
            "Unknown/RAW filesystem: run Deep Carving to recover by file signatures." => {
                "Bilinmeyen / RAW dosya sistemi: dosya imzalarıyla kurtarmak için Derin Tarama çalıştırın.".to_string()
            }
            _ => {
                if let Some(rest) = warning.strip_prefix("NTFS scan note: ") {
                    format!("NTFS tarama notu: {}", rest)
                } else if let Some(rest) = warning.strip_prefix("exFAT scan note: ") {
                    format!("exFAT tarama notu: {}", rest)
                } else if let Some(rest) = warning.strip_prefix("FAT32 scan note: ") {
                    format!("FAT32 tarama notu: {}", rest)
                } else if let Some(rest) = warning.strip_prefix("Carving note: ") {
                    format!("Derin tarama notu: {}", rest)
                } else {
                    warning.to_string()
                }
            }
        }
    }

    pub fn localize_source_desc(self, src: &str) -> String {
        if !self.is_tr() {
            return src.to_string();
        }
        if let Some(rest) = src.strip_prefix("Carved @ ") {
            format!("İmza @ {}", rest)
        } else if let Some(rest) = src.strip_prefix("exFAT clus #") {
            format!("exFAT küme #{}", rest)
        } else if let Some(rest) = src.strip_prefix("FAT32 clus #") {
            format!("FAT32 küme #{}", rest)
        } else {
            src.to_string()
        }
    }
}
