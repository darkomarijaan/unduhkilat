//! Turning the bytes of a text file into a string, or refusing when they are not text.

/// Decode UTF-8 (with or without BOM) or UTF-16 (with BOM). `None` for binary data or other
/// encodings, so unknown bytes are never shown as garbled text. When `cut` the data ends mid-file:
/// a partial character at the end is dropped and the text is trimmed back to a line end.
pub fn decode_text(bytes: &[u8], cut: bool) -> Option<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return decode_utf16(rest, false, cut);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return decode_utf16(rest, true, cut);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    if bytes.contains(&0) {
        return None;
    }
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => t.to_string(),
        // A cut can land in the middle of a character: keep what is valid.
        Err(e) if cut && e.error_len().is_none() => std::str::from_utf8(&bytes[..e.valid_up_to()]).ok()?.to_string(),
        Err(_) => return None,
    };
    finish(text, cut)
}

fn decode_utf16(bytes: &[u8], big_endian: bool, cut: bool) -> Option<String> {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|p| if big_endian { u16::from_be_bytes([p[0], p[1]]) } else { u16::from_le_bytes([p[0], p[1]]) }).collect();
    let mut text = String::from_utf16(&units).ok().or_else(|| if cut { String::from_utf16(&units[..units.len().saturating_sub(1)]).ok() } else { None })?;
    text.retain(|c| c != '\0');
    finish(text, cut)
}

/// Reject control-heavy data; trim a cut text back to the last complete line.
fn finish(mut text: String, cut: bool) -> Option<String> {
    let controls = text.chars().filter(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t' | '\u{c}' | '\u{1b}')).count();
    if text.chars().count() > 0 && controls * 100 > text.chars().count() {
        return None;
    }
    if cut {
        if let Some(i) = text.rfind('\n') {
            text.truncate(i + 1);
        }
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_with_and_without_bom() {
        assert_eq!(decode_text("halo \u{4F60}\u{597D}".as_bytes(), false).as_deref(), Some("halo \u{4F60}\u{597D}"));
        let mut b = vec![0xEF, 0xBB, 0xBF];
        b.extend_from_slice(b"abc");
        assert_eq!(decode_text(&b, false).as_deref(), Some("abc"));
    }

    #[test]
    fn utf16_with_bom() {
        let mut b = vec![0xFF, 0xFE];
        for u in "a\u{4F60}".encode_utf16() {
            b.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_text(&b, false).as_deref(), Some("a\u{4F60}"));
    }

    #[test]
    fn a_cut_in_the_middle_of_a_character_is_dropped_not_an_error() {
        let s = "satu\ndua \u{4F60}".as_bytes().to_vec();
        let cut = &s[..s.len() - 1]; // half of the last character
        assert_eq!(decode_text(cut, true).as_deref(), Some("satu\n"), "trimmed back to the last full line");
        assert!(decode_text(cut, false).is_none(), "an uncut file with broken UTF-8 is not text");
    }

    #[test]
    fn binary_and_other_encodings_are_refused() {
        assert!(decode_text(&[0, 1, 2, 3], false).is_none());
        assert!(decode_text(&[0xE9, 0x74, 0xE9], false).is_none(), "latin-1 is not guessed");
        assert!(decode_text(&[0x01, 0x02, 0x03, 0x04, b'a'], false).is_none(), "control-heavy");
    }
}
