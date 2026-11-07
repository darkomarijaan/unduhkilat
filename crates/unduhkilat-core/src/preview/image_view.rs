//! Decoding pictures for display, with a cap on how large a decoded picture may get.

use super::PageImage;
use crate::error::{AppError, ErrorCode, Result};
use std::io::Cursor;

/// Decoded pixels at most (about 200 MB of RGBA).
const MAX_PIXELS: u64 = 50_000_000;
/// Longest side shown; larger pictures are scaled down.
const MAX_SIDE: u32 = 4096;

pub fn decode(bytes: &[u8]) -> Result<PageImage> {
    let bad = || AppError::new(ErrorCode::ErrPreviewUnsupported, "Gambar rusak atau formatnya tidak didukung. Gunakan Unduh.");
    let reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|_| bad())?;
    let (w, h) = reader.into_dimensions().map_err(|_| bad())?;
    if (w as u64) * (h as u64) > MAX_PIXELS {
        return Err(AppError::new(ErrorCode::ErrPreviewTooLarge, format!("Gambar {w}x{h} terlalu besar untuk dipratinjau. Gunakan Unduh.")));
    }
    let mut img = image::load_from_memory(bytes).map_err(|_| bad())?.to_rgba8();
    // A texture side above the GPU limit crashes the renderer: keep both sides within MAX_SIDE.
    let longest = img.width().max(img.height());
    if longest > MAX_SIDE {
        let f = MAX_SIDE as f32 / longest as f32;
        let (w, h) = (((img.width() as f32 * f) as u32).max(1), ((img.height() as f32 * f) as u32).max(1));
        img = image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle);
    }
    Ok(PageImage { width: img.width(), height: img.height(), rgba: img.into_raw() })
}

/// Decode a PNG made by the PDF renderer.
pub(super) fn from_png(png: &[u8]) -> Result<PageImage> {
    decode(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn image_preview_decodes_png_and_jpeg_and_rejects_corrupt() {
        let p = decode(&png(3, 2)).unwrap();
        assert_eq!((p.width, p.height, p.rgba.len()), (3, 2, 24));
        assert_eq!(&p.rgba[..4], &[10, 20, 30, 255]);
        let rgb = image::RgbImage::from_pixel(4, 4, image::Rgb([200, 100, 50]));
        let mut jpg = Vec::new();
        rgb.write_to(&mut Cursor::new(&mut jpg), image::ImageFormat::Jpeg).unwrap();
        assert_eq!(decode(&jpg).unwrap().width, 4);
        assert_eq!(decode(b"definitely not an image").unwrap_err().code, ErrorCode::ErrPreviewUnsupported);
        let mut cut = png(50, 50);
        cut.truncate(40);
        assert!(decode(&cut).is_err(), "a truncated file is an error, not a panic");
    }

    #[test]
    fn a_very_wide_picture_is_scaled_to_fit_a_texture() {
        let wide = decode(&png(9000, 100)).unwrap();
        assert!(wide.width <= 4096 && wide.height <= 4096, "{}x{}", wide.width, wide.height);
        assert_eq!(wide.rgba.len(), (wide.width * wide.height * 4) as usize);
        assert!(wide.height >= 1, "never collapses to nothing");
        let ratio = wide.width as f32 / wide.height as f32;
        assert!((ratio - 90.0).abs() < 8.0, "aspect ratio kept: {ratio}");
        let small = decode(&png(300, 200)).unwrap();
        assert_eq!((small.width, small.height), (300, 200), "small pictures are untouched");
    }
}
