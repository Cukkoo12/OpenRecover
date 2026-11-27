# Privacy Policy — OpenRecover

**Effective Date:** 2026-11-27  
**Repository:** https://github.com/Cukkoo12/OpenRecover  

**[English](#english) · [Türkçe](#türkçe)**

---

## English

OpenRecover is an open-source, 100% offline data recovery utility for Windows, Linux, and macOS licensed under the MIT License.

### 1. Data Collection & Telemetry
- **Zero data collection:** OpenRecover does **not** collect, store, transmit, track, or sell any personal data, usage analytics, telemetry, crash reports, file lists, or recovered file contents.
- **No network connections:** OpenRecover operates completely offline. It makes no outbound network requests and contains no ads, tracking SDKs, accounts, or license servers.

### 2. Local Disk & File Access
- **Read-only scanning:** When you select a local storage volume, physical disk, or disk image file (`.img`, `.dd`, `.raw`, `.bin`), OpenRecover reads storage sectors strictly on your local machine in read-only mode to locate recoverable files and generate local previews.
- **User-controlled recovery output:** Recovered files are written solely to the destination folder you explicitly choose on your computer. A built-in same-volume write protection prevents accidentally writing recovered files back onto the scanned source partition.
- **Administrative privileges (Windows):** Reading raw local volumes (`C:`, `D:`, USB/SD block devices) on Windows requires local Administrator privileges granted through standard Windows UAC only when requested by the user. Disk image files can be scanned without Administrator privileges.

### 3. Third-Party Services
OpenRecover does not integrate with any third-party analytics, cloud storage, or advertising providers. External links (such as GitHub `github.com/Cukkoo12/OpenRecover` or optional support link `buymeacoffee.com/Cukkoo` in standalone builds) are opened in your web browser only if you explicitly choose to click them.

### 4. Contact
For privacy questions or security reports, please open an issue at:  
https://github.com/Cukkoo12/OpenRecover/issues

---

## Türkçe

OpenRecover; Windows, Linux ve macOS için MIT Lisansı ile sunulan, açık kaynaklı ve %100 çevrimdışı çalışan bir veri kurtarma aracıdır.

### 1. Veri Toplama ve Telemetri
- **Sıfır veri toplama:** OpenRecover hiçbir kişisel veriyi, kullanım istatistiğini, telemetriyi, hata raporunu, dosya listesini veya kurtarılan dosya içeriğini **toplamaz, saklamaz, iletmez ve paylaşmaz**.
- **Ağ bağlantısı yoktur:** Uygulama tamamen çevrimdışı çalışır. Dış sunuculara bağlantı kurmaz; reklam, izleme SDK’sı, hesap girişi veya lisans sunucusu içermez.

### 2. Yerel Disk ve Dosya Erişimi
- **Salt-okunur tarama:** Seçtiğiniz yerel disk bölümü, fiziksel sürücü veya disk imajı (`.img`, `.dd`, `.raw`, `.bin`) yalnızca bilgisayarınızda yerel olarak ve salt-okunur biçimde taranır; dosya önizlemeleri cihazınız içinde oluşturulur.
- **Kullanıcı kontrollü kayıt:** Kurtarılan dosyalar yalnızca kullanıcının açıkça seçtiği hedef klasöre yazılır. Taranan kaynak sürücünün üzerine yazılmasını engelleyen yerleşik koruma kilidi bulunur.
- **Yönetici izinleri (Windows):** Canlı yerel sürücülerin (`C:`, `D:`, USB/SD blok aygıtları) ham sektörlerini okumak için yalnızca kullanıcının talebi üzerine standart Windows UAC üzerinden yerel Yönetici yetkisi kullanılır. Disk imajları yönetici izni olmadan taranabilir.

### 3. Üçüncü Taraf Hizmetler
OpenRecover hiçbir üçüncü taraf analitik, bulut veya reklam hizmeti kullanmaz.

### 4. İletişim
Gizlilik veya güvenlik sorularınız için GitHub üzerinden bildirim açabilirsiniz:  
https://github.com/Cukkoo12/OpenRecover/issues
