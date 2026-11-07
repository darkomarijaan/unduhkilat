//! Previews of files on a server without downloading them in full (design spec section 6).
//!
//! Text and Markdown are read from the start only (the first megabyte, more on request). Images,
//! PDF and Office files are fetched whole, but never above a size limit. Nothing in a document is
//! ever executed (no macros, no scripts). Parsing is bounded so a hostile file cannot exhaust memory.

pub mod image_view;
pub mod markdown;
pub mod office;
pub mod pdf;
pub mod text;

use crate::error::{AppError, ErrorCode, Result};
use crate::server::fs::RemoteFs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Markdown,
    Image,
    Pdf,
    Docx,
    Odt,
    Xlsx,
    Ods,
    /// Known office formats we cannot read (doc, xls, ppt, pptx, ...): offer "open in Windows".
    External,
    Unknown,
}

pub fn kind_for(name: &str) -> Kind {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "md" | "markdown" => Kind::Markdown,
        "txt" | "log" | "json" | "xml" | "yaml" | "yml" | "ini" | "conf" | "cfg" | "toml" | "csv" | "tsv" | "sh" | "bat" | "ps1" | "py" | "js" | "ts" | "rs" | "go" | "java" | "c" | "h" | "cpp" | "cs" | "php" | "rb" | "sql" | "html" | "htm" | "css" | "env" | "service" | "properties" => Kind::Text,
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" => Kind::Image,
        "pdf" => Kind::Pdf,
        "docx" => Kind::Docx,
        "odt" => Kind::Odt,
        "xlsx" => Kind::Xlsx,
        "ods" => Kind::Ods,
        "doc" | "xls" | "ppt" | "pptx" | "odp" | "rtf" | "pages" | "numbers" | "key" => Kind::External,
        _ => Kind::Unknown,
    }
}

/// One block of a rendered document (Markdown, docx, odt).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading { level: u8, text: String },
    Paragraph(String),
    ListItem { ordered: bool, depth: u8, text: String },
    Code(String),
    Quote(String),
    Table(Vec<Vec<String>>),
    Rule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sheet {
    pub name: String,
    pub rows: Vec<Vec<String>>,
    /// More rows or columns existed than were kept.
    pub truncated: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub struct PageImage {
    pub width: u32,
    pub height: u32,
    /// RGBA, 8 bits per channel, `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for PageImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PageImage({}x{})", self.width, self.height)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    Text { text: String, truncated: bool },
    Markdown { blocks: Vec<Block>, truncated: bool },
    Image(PageImage),
    /// The first page is rendered; later pages come from `pdf::render_page` on `bytes`.
    Pdf { pages: usize, first: PageImage, bytes: Vec<u8> },
    Doc(Vec<Block>),
    Sheets(Vec<Sheet>),
}

#[derive(Debug, Clone)]
pub struct Limits {
    /// Images, PDF and Office files larger than this are not fetched.
    pub max_bytes: u64,
    /// Text and Markdown: how much of the start to read.
    pub text_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_bytes: 25 * 1024 * 1024, text_bytes: 1024 * 1024 }
    }
}

/// Fetch and parse a preview of `path` (`size` bytes, from the listing). Never reads more than the
/// limits allow.
pub async fn load(fs: &mut dyn RemoteFs, path: &str, size: u64, limits: &Limits) -> Result<Preview> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let kind = kind_for(name);
    match kind {
        Kind::Text | Kind::Markdown => {
            let want = limits.text_bytes.min(size as usize).max(1);
            let bytes = fs.read_at(path, 0, want).await?;
            let truncated = (bytes.len() as u64) < size;
            let text = text::decode_text(&bytes, truncated).ok_or_else(|| AppError::new(ErrorCode::ErrPreviewUnsupported, "Berkas ini berisi data biner, bukan teks. Gunakan Unduh."))?;
            if kind == Kind::Markdown {
                Ok(Preview::Markdown { blocks: markdown::parse(&text), truncated })
            } else {
                Ok(Preview::Text { text, truncated })
            }
        }
        Kind::External => Err(AppError::new(ErrorCode::ErrPreviewUnsupported, "Format ini belum bisa dipratinjau. Gunakan \"Buka di aplikasi Windows\" atau Unduh.")),
        Kind::Unknown => Err(AppError::new(ErrorCode::ErrPreviewUnsupported, "Jenis berkas ini tidak dikenali, jadi tidak ditampilkan. Gunakan Unduh.")),
        _ => {
            if size > limits.max_bytes {
                return Err(AppError::new(ErrorCode::ErrPreviewTooLarge, format!("Berkas {} MB melebihi batas pratinjau {} MB. Gunakan Unduh atau Buka di aplikasi Windows.", size / 1_048_576, limits.max_bytes / 1_048_576)));
            }
            let bytes = read_all(fs, path, size).await?;
            tokio::task::spawn_blocking(move || parse_whole(kind, bytes)).await.map_err(|_| AppError::internal("pratinjau berhenti tiba-tiba"))?
        }
    }
}

async fn read_all(fs: &mut dyn RemoteFs, path: &str, size: u64) -> Result<Vec<u8>> {
    const STEP: usize = 1024 * 1024;
    let mut out = Vec::with_capacity(size.min(26 * 1024 * 1024) as usize);
    while (out.len() as u64) < size {
        let part = fs.read_at(path, out.len() as u64, STEP.min((size - out.len() as u64) as usize)).await?;
        if part.is_empty() {
            break;
        }
        out.extend_from_slice(&part);
    }
    Ok(out)
}

fn parse_whole(kind: Kind, bytes: Vec<u8>) -> Result<Preview> {
    match kind {
        Kind::Image => Ok(Preview::Image(image_view::decode(&bytes)?)),
        Kind::Pdf => {
            let pages = pdf::page_count(&bytes)?;
            let first = pdf::render_page(&bytes, 0, 1.5)?;
            Ok(Preview::Pdf { pages, first, bytes })
        }
        Kind::Docx => Ok(Preview::Doc(office::docx_blocks(&bytes)?)),
        Kind::Odt => Ok(Preview::Doc(office::odt_blocks(&bytes)?)),
        Kind::Xlsx => Ok(Preview::Sheets(office::xlsx_sheets(&bytes)?)),
        Kind::Ods => Ok(Preview::Sheets(office::ods_sheets(&bytes)?)),
        _ => Err(AppError::new(ErrorCode::ErrPreviewUnsupported, "Jenis berkas ini tidak bisa dipratinjau.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::fs::{Entry, RemoteReader, RemoteWriter};
    use async_trait::async_trait;

    #[test]
    fn kind_for_maps_extensions() {
        assert_eq!(kind_for("README.MD"), Kind::Markdown);
        assert_eq!(kind_for("a.log"), Kind::Text);
        assert_eq!(kind_for("photo.JPEG"), Kind::Image);
        assert_eq!(kind_for("x.pdf"), Kind::Pdf);
        assert_eq!(kind_for("a.docx"), Kind::Docx);
        assert_eq!(kind_for("a.xlsx"), Kind::Xlsx);
        assert_eq!(kind_for("a.ods"), Kind::Ods);
        assert_eq!(kind_for("a.xls"), Kind::External);
        assert_eq!(kind_for("noext"), Kind::Unknown);
        assert_eq!(kind_for("archive.tar.gz"), Kind::Unknown);
    }

    /// A file system that only knows how to read one file, and counts the bytes it hands out.
    struct Fake {
        data: Vec<u8>,
        served: usize,
    }

    #[async_trait]
    impl RemoteFs for Fake {
        async fn list(&mut self, _d: &str) -> Result<Vec<Entry>> {
            unimplemented!()
        }
        async fn stat(&mut self, _p: &str) -> Result<Entry> {
            unimplemented!()
        }
        async fn mkdir(&mut self, _p: &str) -> Result<()> {
            unimplemented!()
        }
        async fn rename(&mut self, _a: &str, _b: &str) -> Result<()> {
            unimplemented!()
        }
        async fn remove(&mut self, _p: &str, _d: bool) -> Result<()> {
            unimplemented!()
        }
        async fn read_at(&mut self, _p: &str, offset: u64, len: usize) -> Result<Vec<u8>> {
            let start = (offset as usize).min(self.data.len());
            let end = (start + len).min(self.data.len());
            self.served += end - start;
            Ok(self.data[start..end].to_vec())
        }
        async fn open_read(&mut self, _p: &str, _o: u64) -> Result<RemoteReader> {
            unimplemented!()
        }
        async fn open_write(&mut self, _p: &str, _o: u64, _a: bool) -> Result<RemoteWriter> {
            unimplemented!()
        }
        async fn home(&mut self) -> Result<String> {
            Ok("/".into())
        }
        async fn close(self: Box<Self>) {}
    }

    #[tokio::test]
    async fn text_preview_truncates_at_the_limit_and_loads_more() {
        let body = "line\n".repeat(400_000); // 2 MB
        let mut fs = Fake { data: body.clone().into_bytes(), served: 0 };
        let size = body.len() as u64;
        let first = load(&mut fs, "/big.txt", size, &Limits::default()).await.unwrap();
        let Preview::Text { text, truncated } = first else { panic!("text") };
        assert!(truncated && text.len() <= 1024 * 1024 && text.ends_with('\n'), "cut at a line end: {}", text.len());
        assert!(fs.served <= 1024 * 1024 + 8, "only the first megabyte was fetched: {}", fs.served);
        let more = load(&mut fs, "/big.txt", size, &Limits { text_bytes: 4 * 1024 * 1024, ..Limits::default() }).await.unwrap();
        let Preview::Text { text, truncated } = more else { panic!("text") };
        assert!(!truncated && text.len() == body.len());
    }

    #[tokio::test]
    async fn file_over_the_limit_is_not_fetched() {
        let mut fs = Fake { data: vec![0u8; 3_000_000], served: 0 };
        let e = load(&mut fs, "/huge.pdf", 30_000_000, &Limits::default()).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::ErrPreviewTooLarge);
        assert_eq!(fs.served, 0, "not one byte was transferred");
        let e = load(&mut fs, "/a.xls", 10, &Limits::default()).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::ErrPreviewUnsupported);
        let e = load(&mut fs, "/mystery.bin", 10, &Limits::default()).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::ErrPreviewUnsupported);
        assert_eq!(fs.served, 0);
    }

    #[tokio::test]
    async fn binary_data_named_txt_is_not_shown_as_text() {
        let mut fs = Fake { data: vec![0, 159, 146, 150, 0, 1, 2, 3], served: 0 };
        let e = load(&mut fs, "/x.txt", 8, &Limits::default()).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::ErrPreviewUnsupported);
    }
}
