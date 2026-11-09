//! Jenis berkas yang TIDAK ditangkap dari browser (seperti daftar tipe berkas di IDM).
//!
//! Pengguna menulis daftar ekstensi (mis. `pdf, jpg`) di Pengaturan → Integrasi Browser.
//! Unduhan browser yang cocok dibiarkan diunduh browser sendiri: aplikasi menolak
//! `browser.intercept` dengan `ERR_CAPTURE_SKIPPED`, dan ekstensi (yang menerima daftar
//! yang sama lewat `ping`) bahkan tidak meneruskannya. Logika ini kembar dengan
//! `shouldSkipCapture` di `extension/shared.js`; ubah keduanya bersamaan.
//!
//! Pencocokan memakai ekstensi nama berkas, ekstensi jalur URL, DAN tipe MIME, karena
//! banyak tautan unduhan tidak berekstensi (mis. `.../invoice?id=123` yang berisi PDF).

/// MIME → ekstensi yang dianggap setara. Hanya tipe umum; MIME lain dicocokkan lewat nama.
const MIME_EXTS: &[(&str, &[&str])] = &[
    ("application/pdf", &["pdf"]),
    ("application/x-pdf", &["pdf"]),
    ("application/zip", &["zip"]),
    ("application/x-zip-compressed", &["zip"]),
    ("application/vnd.rar", &["rar"]),
    ("application/x-rar-compressed", &["rar"]),
    ("application/x-7z-compressed", &["7z"]),
    ("application/gzip", &["gz"]),
    ("application/x-gzip", &["gz"]),
    ("application/x-tar", &["tar"]),
    ("application/x-msdownload", &["exe"]),
    ("application/x-msdos-program", &["exe"]),
    ("application/vnd.microsoft.portable-executable", &["exe"]),
    ("application/x-msi", &["msi"]),
    ("application/x-ms-installer", &["msi"]),
    ("application/x-iso9660-image", &["iso"]),
    ("application/vnd.android.package-archive", &["apk"]),
    ("application/x-bittorrent", &["torrent"]),
    ("application/epub+zip", &["epub"]),
    ("application/msword", &["doc"]),
    ("application/vnd.openxmlformats-officedocument.wordprocessingml.document", &["docx"]),
    ("application/vnd.ms-excel", &["xls"]),
    ("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", &["xlsx"]),
    ("application/vnd.ms-powerpoint", &["ppt"]),
    ("application/vnd.openxmlformats-officedocument.presentationml.presentation", &["pptx"]),
    ("text/csv", &["csv"]),
    ("text/plain", &["txt"]),
    ("image/jpeg", &["jpg", "jpeg"]),
    ("image/png", &["png"]),
    ("image/gif", &["gif"]),
    ("image/webp", &["webp"]),
    ("image/svg+xml", &["svg"]),
    ("video/mp4", &["mp4"]),
    ("video/webm", &["webm"]),
    ("video/x-matroska", &["mkv"]),
    ("audio/mpeg", &["mp3"]),
];

/// Ubah teks pengaturan menjadi daftar ekstensi bersih: huruf kecil, tanpa `*.`/`.`,
/// dipisah koma, titik koma, atau spasi. Duplikat dibuang.
pub fn parse_list(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split(|c: char| c == ',' || c == ';' || c.is_whitespace()) {
        let e = raw.trim().trim_start_matches('*').trim_start_matches('.').to_ascii_lowercase();
        if !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric()) && !out.contains(&e) {
            out.push(e);
        }
    }
    out
}

/// Bentuk rapi untuk disimpan/ditampilkan: `pdf, jpg`.
pub fn normalize(text: &str) -> String {
    parse_list(text).join(", ")
}

fn ext_of(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let (stem, ext) = base.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() || ext.len() > 10 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// Semua ekstensi yang bisa diturunkan dari satu unduhan browser.
fn candidate_exts(file_name: Option<&str>, url: &str, mime: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(e) = file_name.and_then(ext_of) {
        out.push(e);
    }
    if let Ok(u) = url::Url::parse(url) {
        if let Some(e) = ext_of(u.path()) {
            out.push(e);
        }
    }
    let m = mime.unwrap_or("").split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if let Some((_, exts)) = MIME_EXTS.iter().find(|(k, _)| *k == m) {
        out.extend(exts.iter().map(|e| e.to_string()));
    }
    out
}

/// True bila unduhan ini termasuk jenis yang dibiarkan diunduh browser.
pub fn should_skip(list: &[String], file_name: Option<&str>, url: &str, mime: Option<&str>) -> bool {
    if list.is_empty() {
        return false;
    }
    candidate_exts(file_name, url, mime).iter().any(|e| list.contains(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(s: &str) -> Vec<String> {
        parse_list(s)
    }

    #[test]
    fn parse_accepts_common_spellings() {
        assert_eq!(l(" PDF, *.jpg; .zip  mp4,,pdf "), vec!["pdf", "jpg", "zip", "mp4"]);
        assert!(l("").is_empty());
        assert!(l("../x, a/b").is_empty());
        assert_eq!(normalize("pdf;JPG"), "pdf, jpg");
    }

    #[test]
    fn matches_by_file_name_url_or_mime() {
        let list = l("pdf");
        assert!(should_skip(&list, Some("Laporan.PDF"), "https://a.test/x", None));
        assert!(should_skip(&list, None, "https://a.test/files/report.pdf?dl=1", None));
        // Tautan tanpa ekstensi: cukup dari MIME (kasus invoice Tokopedia).
        assert!(should_skip(&list, None, "https://www.tokopedia.com/digital/invoice?id=3240614950", Some("application/pdf; charset=binary")));
        assert!(should_skip(&list, Some("C:\\Users\\a\\Downloads\\x.pdf"), "https://a.test/get", None));
    }

    #[test]
    fn other_types_and_empty_list_are_captured() {
        assert!(!should_skip(&l("pdf"), Some("setup.exe"), "https://a.test/setup.exe", Some("application/x-msdownload")));
        assert!(!should_skip(&[], Some("a.pdf"), "https://a.test/a.pdf", Some("application/pdf")));
        assert!(!should_skip(&l("pdf"), None, "https://a.test/pdf", None)); // "pdf" sebagai nama, bukan ekstensi
        assert!(!should_skip(&l("pdf"), Some(".pdf"), "https://a.test/x", None));
    }

    #[test]
    fn mime_aliases_cover_both_spellings() {
        assert!(should_skip(&l("jpeg"), None, "https://a.test/img", Some("image/jpeg")));
        assert!(should_skip(&l("jpg"), None, "https://a.test/img", Some("image/jpeg")));
        assert!(should_skip(&l("pdf"), None, "https://a.test/b%20c.pdf", None));
    }
}
