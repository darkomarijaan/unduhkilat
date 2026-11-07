//! Safe pasting: normalized line endings, no injected escape sequences, chunked so the UI never stalls.

pub const CHUNK_BYTES: usize = 4096;

const BRACKET_START: &[u8] = b"\x1b[200~";
const BRACKET_END: &[u8] = b"\x1b[201~";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PastePlan {
    /// Bytes to send, in order. Send them one at a time, yielding in between, so a large paste never
    /// blocks the interface or floods the program's input.
    pub chunks: Vec<Vec<u8>>,
    /// The text has several lines and the program did not ask for bracketed paste: each line would
    /// run as its own command, so the user should confirm first.
    pub needs_confirm: bool,
}

/// Prepare `text` for pasting into a program.
///
/// Line endings become CR (what Enter sends). Escape characters and the 8-bit C1 controls are removed,
/// so pasted text can never carry the bracketed-paste end marker or any other terminal sequence.
pub fn plan_paste(text: &str, bracketed: bool) -> PastePlan {
    let mut clean = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                clean.push('\r');
            }
            '\n' => clean.push('\r'),
            // Escape, C1, and the other C0 controls: ^O or ^C inside pasted text would act on the
            // program (accept a line, interrupt) even without a line break. Tab stays.
            '\u{0}'..='\u{8}' | '\u{b}' | '\u{c}' | '\u{e}'..='\u{1f}' | '\u{7f}'..='\u{9f}' => {}
            other => clean.push(other),
        }
    }
    let body = clean.strip_suffix('\r').unwrap_or(&clean);
    let needs_confirm = !bracketed && body.contains('\r');

    let mut chunks: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::with_capacity(CHUNK_BYTES);
    for c in clean.chars() {
        let mut buf = [0u8; 4];
        let bytes = c.encode_utf8(&mut buf).as_bytes();
        if cur.len() + bytes.len() > CHUNK_BYTES {
            chunks.push(std::mem::take(&mut cur));
        }
        cur.extend_from_slice(bytes);
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    if bracketed && !chunks.is_empty() {
        let first = chunks.first_mut().unwrap();
        first.splice(0..0, BRACKET_START.iter().copied());
        chunks.last_mut().unwrap().extend_from_slice(BRACKET_END);
    }
    PastePlan { chunks, needs_confirm }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(plan: &PastePlan) -> Vec<u8> {
        plan.chunks.concat()
    }

    #[test]
    fn crlf_becomes_cr() {
        let p = plan_paste("a\r\nb\nc\r", false);
        assert_eq!(joined(&p), b"a\rb\rc\r");
    }

    #[test]
    fn escape_bytes_are_stripped_so_the_end_marker_cannot_be_injected() {
        let p = plan_paste("a\x1b[201~rm -rf /\x1b[200~b", true);
        let all = joined(&p);
        let body = &all[b"\x1b[200~".len()..all.len() - b"\x1b[201~".len()];
        assert!(!body.contains(&0x1b), "no ESC may survive inside the pasted text: {body:?}");
        assert_eq!(body, b"a[201~rm -rf /[200~b");
        assert!(all.starts_with(b"\x1b[200~") && all.ends_with(b"\x1b[201~"));
        assert_eq!(all.windows(6).filter(|w| *w == b"\x1b[201~").count(), 1, "exactly one end marker");
    }

    #[test]
    fn bracketed_wraps_first_and_last_chunk() {
        let text = "x".repeat(CHUNK_BYTES * 3 + 10);
        let p = plan_paste(&text, true);
        assert!(p.chunks.len() >= 4);
        assert!(p.chunks.first().unwrap().starts_with(b"\x1b[200~"));
        assert!(p.chunks.last().unwrap().ends_with(b"\x1b[201~"));
        assert!(!p.chunks[1].starts_with(b"\x1b"), "middle chunks carry no markers");
        assert!(!plan_paste("hi", false).chunks[0].starts_with(b"\x1b[200~"), "no markers when the program did not ask");
    }

    #[test]
    fn chunks_never_split_utf8() {
        let text = "\u{e9}\u{4F60}\u{1F600}".repeat(10_000);
        let p = plan_paste(&text, false);
        assert!(p.chunks.len() > 1);
        for c in &p.chunks {
            assert!(std::str::from_utf8(c).is_ok(), "a chunk cut a character in half");
            assert!(c.len() <= CHUNK_BYTES);
        }
        assert_eq!(joined(&p), text.as_bytes());
    }

    #[test]
    fn one_megabyte_is_chunked() {
        let text = "line of text\r\n".repeat(1_048_576 / 13 + 1); // 13 bytes per line once CRLF becomes CR: just over 1 MiB
        let p = plan_paste(&text, true);
        assert!(p.chunks.len() >= 256, "{} chunks", p.chunks.len());
        assert!(p.chunks.iter().all(|c| c.len() <= CHUNK_BYTES + 14), "chunk size stays bounded");
    }

    #[test]
    fn multiline_without_bracketed_needs_confirm() {
        assert!(plan_paste("a\nb", false).needs_confirm);
        assert!(plan_paste("a\r\nb", false).needs_confirm);
        assert!(!plan_paste("single line", false).needs_confirm);
        assert!(!plan_paste("a\nb", true).needs_confirm, "bracketed paste is safe for the program to handle");
        assert!(!plan_paste("one line\n", false).needs_confirm, "a single trailing newline is one line");
    }

    #[test]
    fn empty_text_plans_nothing() {
        assert!(plan_paste("", false).chunks.is_empty());
        assert!(plan_paste("", true).chunks.is_empty());
    }

    #[test]
    fn control_characters_other_than_tab_and_newline_are_removed() {
        // ^O accepts the line in bash, ^C interrupts, ^U kills the line, ^D ends input, BEL, BS, DEL.
        let p = plan_paste("rm -rf ~\u{f}\u{3}\u{15}\u{4}\u{7}\u{8}\u{7f}ok\tx", false);
        assert_eq!(joined(&p), b"rm -rf ~ok\tx");
        assert!(!p.needs_confirm);
        let p = plan_paste("a\u{f}\nb", true);
        assert_eq!(joined(&p), b"\x1b[200~a\rb\x1b[201~");
    }
}
