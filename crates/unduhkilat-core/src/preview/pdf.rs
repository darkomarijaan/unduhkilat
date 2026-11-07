//! PDF pages rendered to pictures with `hayro` (pure Rust, no DLL; see docs/superpowers/spikes/2026-11-07-pdf.md).
//! Encrypted PDFs are not supported and give a clear message.

use super::PageImage;
use crate::error::{AppError, ErrorCode, Result};
use hayro::hayro_syntax::{LoadPdfError, Pdf};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::{PixmapSettings, RenderCache, RenderSettings};

/// Render at most this many pixels per side, whatever the scale asks for.
const MAX_SIDE: f32 = 4000.0;

fn open(bytes: &[u8]) -> Result<Pdf> {
    Pdf::new(bytes.to_vec()).map_err(|e| match e {
        LoadPdfError::Decryption(_) => AppError::new(ErrorCode::ErrPreviewUnsupported, "PDF ini terkunci (berkata sandi) dan belum bisa dipratinjau. Gunakan Unduh atau Buka di aplikasi Windows."),
        LoadPdfError::Invalid => AppError::new(ErrorCode::ErrPreviewUnsupported, "PDF rusak atau tidak bisa dibaca. Gunakan Unduh."),
    })
}

pub fn page_count(bytes: &[u8]) -> Result<usize> {
    Ok(open(bytes)?.pages().len())
}

/// Render page `index` (0-based) at `scale` (1.0 = 72 dpi).
pub fn render_page(bytes: &[u8], index: usize, scale: f32) -> Result<PageImage> {
    let pdf = open(bytes)?;
    let pages = pdf.pages();
    let page = pages.get(index).ok_or_else(|| AppError::new(ErrorCode::ErrNotFound, format!("PDF hanya memiliki {} halaman.", pages.len())))?;
    let (w, h) = page.render_dimensions();
    let scale = scale.clamp(0.1, 8.0).min(MAX_SIDE / w.max(h).max(1.0));
    let settings = PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE };
    let pixmap = hayro::render(page, &RenderCache::new(), &InterpreterSettings::default(), &RenderSettings::default(), &settings);
    let png = pixmap.into_png().map_err(|_| AppError::internal("gagal menyimpan halaman PDF"))?;
    super::image_view::from_png(&png)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A tiny, valid, one-page PDF with the text "Halo".
    pub fn tiny_pdf() -> Vec<u8> {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
            {
                let content = "BT /F1 24 Tf 20 40 Td (Halo) Tj ET";
                format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len())
            },
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    #[test]
    fn pdf_first_page_renders() {
        let pdf = tiny_pdf();
        assert_eq!(page_count(&pdf).unwrap(), 1);
        let img = render_page(&pdf, 0, 2.0).unwrap();
        assert_eq!((img.width, img.height), (400, 200));
        assert!(img.rgba.chunks(4).any(|p| p[0] < 100), "some dark pixels: the text was drawn");
        assert!(img.rgba.chunks(4).any(|p| p == [255, 255, 255, 255]), "white background");
        assert_eq!(render_page(&pdf, 3, 1.0).unwrap_err().code, ErrorCode::ErrNotFound);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert_eq!(page_count(b"%PDF-1.4 nothing useful").unwrap_err().code, ErrorCode::ErrPreviewUnsupported);
        assert!(render_page(b"", 0, 1.0).is_err());
    }
}
