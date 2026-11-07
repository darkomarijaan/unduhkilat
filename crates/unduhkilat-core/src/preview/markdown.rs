//! Markdown to a flat list of blocks the UI can draw (headings, paragraphs, lists, tables, code).
//! Raw HTML in the document is shown as text, never interpreted.

use super::Block;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

pub fn parse(src: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut heading: Option<u8> = None;
    let mut code = false;
    let mut quote = 0usize;
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut item = false;
    let mut table: Option<Vec<Vec<String>>> = None;
    let mut row: Vec<String> = Vec::new();
    let mut cell = false;

    let flush_paragraph = |out: &mut Vec<Block>, text: &mut String, quote: usize| {
        let t = text.trim().to_string();
        text.clear();
        if !t.is_empty() {
            out.push(if quote > 0 { Block::Quote(t) } else { Block::Paragraph(t) });
        }
    };

    for ev in Parser::new_ext(src, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH) {
        match ev {
            Event::Start(Tag::Heading { level, .. }) => {
                flush_paragraph(&mut out, &mut text, quote);
                heading = Some(match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                });
            }
            Event::End(TagEnd::Heading(_)) => {
                out.push(Block::Heading { level: heading.take().unwrap_or(1), text: text.trim().to_string() });
                text.clear();
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                flush_paragraph(&mut out, &mut text, quote);
                code = true;
                let _ = matches!(kind, CodeBlockKind::Fenced(_));
            }
            Event::End(TagEnd::CodeBlock) => {
                code = false;
                out.push(Block::Code(text.trim_end_matches('\n').to_string()));
                text.clear();
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush_paragraph(&mut out, &mut text, quote);
                quote += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush_paragraph(&mut out, &mut text, quote);
                quote = quote.saturating_sub(1);
            }
            Event::Start(Tag::List(start)) => {
                flush_paragraph(&mut out, &mut text, quote);
                lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                item = true;
                text.clear();
            }
            Event::End(TagEnd::Item) => {
                item = false;
                let t = text.trim().to_string();
                text.clear();
                if !t.is_empty() {
                    out.push(Block::ListItem { ordered: lists.last().map(|l| l.is_some()).unwrap_or(false), depth: lists.len().saturating_sub(1).min(8) as u8, text: t });
                }
            }
            Event::Start(Tag::Table(_)) => {
                flush_paragraph(&mut out, &mut text, quote);
                table = Some(Vec::new());
            }
            Event::End(TagEnd::Table) => {
                if let Some(t) = table.take() {
                    out.push(Block::Table(t));
                }
            }
            Event::Start(Tag::TableRow) | Event::Start(Tag::TableHead) => row = Vec::new(),
            Event::End(TagEnd::TableRow) | Event::End(TagEnd::TableHead) => {
                if let Some(t) = table.as_mut() {
                    t.push(std::mem::take(&mut row));
                }
            }
            Event::Start(Tag::TableCell) => {
                cell = true;
                text.clear();
            }
            Event::End(TagEnd::TableCell) => {
                cell = false;
                row.push(text.trim().to_string());
                text.clear();
            }
            Event::End(TagEnd::Paragraph) if !item && !cell => flush_paragraph(&mut out, &mut text, quote),
            Event::Text(t) | Event::Code(t) | Event::Html(t) | Event::InlineHtml(t) => text.push_str(&t),
            Event::SoftBreak => text.push(' '),
            Event::HardBreak => text.push('\n'),
            Event::Rule => {
                flush_paragraph(&mut out, &mut text, quote);
                out.push(Block::Rule);
            }
            _ => {}
        }
    }
    flush_paragraph(&mut out, &mut text, quote);
    let _ = code;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_blocks_have_headings_lists_tables_code() {
        let md = "# Judul\n\nIsi **tebal** dan `kode`.\n\n- satu\n- dua\n  - dalam\n\n1. a\n2. b\n\n| X | Y |\n|---|---|\n| 1 | 2 |\n\n```\nlet x = 1;\n```\n\n> kutipan\n\n---\n";
        let b = parse(md);
        assert_eq!(b[0], Block::Heading { level: 1, text: "Judul".into() });
        assert_eq!(b[1], Block::Paragraph("Isi tebal dan kode.".into()));
        assert!(b.contains(&Block::ListItem { ordered: false, depth: 0, text: "satu".into() }));
        assert!(b.contains(&Block::ListItem { ordered: false, depth: 1, text: "dalam".into() }));
        assert!(b.contains(&Block::ListItem { ordered: true, depth: 0, text: "b".into() }));
        assert!(b.contains(&Block::Table(vec![vec!["X".into(), "Y".into()], vec!["1".into(), "2".into()]])));
        assert!(b.contains(&Block::Code("let x = 1;".into())));
        assert!(b.contains(&Block::Quote("kutipan".into())));
        assert!(b.contains(&Block::Rule));
    }

    #[test]
    fn raw_html_is_text_not_markup() {
        let b = parse("<script>alert(1)</script>\n");
        assert!(b.iter().all(|x| matches!(x, Block::Paragraph(t) if t.contains("<script>"))), "{b:?}");
    }
}
