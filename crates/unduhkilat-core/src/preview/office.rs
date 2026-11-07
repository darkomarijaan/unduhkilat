//! Text, headings, lists and tables from docx and odt, and value tables from xlsx and ods.
//! The files are ZIP archives of XML. Only the XML parts are read, nothing is executed, and the
//! sizes are bounded: a hostile "ZIP bomb" is refused instead of exhausting memory.

use super::{Block, Sheet};
use crate::error::{AppError, ErrorCode, Result};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::HashMap;
use std::io::{Cursor, Read};
use zip::ZipArchive;

/// Largest XML part we are willing to unpack.
const MAX_PART_BYTES: u64 = 40 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;
const MAX_ROWS: usize = 5_000;
const MAX_COLS: usize = 100;
const MAX_SHEETS: usize = 30;
const MAX_BLOCKS: usize = 50_000;

fn bad(what: &str) -> AppError {
    AppError::new(ErrorCode::ErrPreviewUnsupported, format!("{what} rusak atau bukan dokumen yang valid. Gunakan Unduh."))
}

fn open_zip(bytes: &[u8]) -> Result<ZipArchive<Cursor<&[u8]>>> {
    let z = ZipArchive::new(Cursor::new(bytes)).map_err(|_| bad("Berkas"))?;
    if z.len() > MAX_ENTRIES {
        return Err(AppError::new(ErrorCode::ErrPreviewTooLarge, "Dokumen berisi terlalu banyak bagian untuk dipratinjau."));
    }
    Ok(z)
}

fn read_part(z: &mut ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Option<Vec<u8>>> {
    let Ok(entry) = z.by_name(name) else { return Ok(None) };
    if entry.size() > MAX_PART_BYTES {
        return Err(AppError::new(ErrorCode::ErrPreviewTooLarge, "Isi dokumen terlalu besar untuk dipratinjau (kemungkinan berkas ZIP berbahaya)."));
    }
    let mut out = Vec::new();
    // The declared size can lie: the real read is capped too.
    entry.take(MAX_PART_BYTES + 1).read_to_end(&mut out).map_err(|_| bad("Berkas"))?;
    if out.len() as u64 > MAX_PART_BYTES {
        return Err(AppError::new(ErrorCode::ErrPreviewTooLarge, "Isi dokumen terlalu besar untuk dipratinjau (kemungkinan berkas ZIP berbahaya)."));
    }
    Ok(Some(out))
}

fn need(z: &mut ZipArchive<Cursor<&[u8]>>, name: &str, what: &str) -> Result<Vec<u8>> {
    read_part(z, name)?.ok_or_else(|| bad(what))
}

fn local(e: &BytesStart<'_>) -> String {
    String::from_utf8_lossy(e.local_name().as_ref()).into_owned()
}

fn attr(e: &BytesStart<'_>, key: &str) -> Option<String> {
    e.attributes().flatten().find(|a| a.key.local_name().as_ref() == key.as_bytes()).and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

fn push_text(buf: &mut String, t: &quick_xml::events::BytesText<'_>) {
    if let Ok(s) = t.unescape() {
        buf.push_str(&s);
    }
}

// ---- docx --------------------------------------------------------------------------------------

pub fn docx_blocks(bytes: &[u8]) -> Result<Vec<Block>> {
    let mut z = open_zip(bytes)?;
    let xml = need(&mut z, "word/document.xml", "Dokumen Word")?;
    let mut r = Reader::from_reader(xml.as_slice());
    let mut buf = Vec::new();
    let mut out: Vec<Block> = Vec::new();
    let (mut text, mut style, mut list, mut in_t) = (String::new(), None::<String>, false, false);
    let mut table_depth = 0usize;
    let (mut table, mut row, mut cell): (Vec<Vec<String>>, Vec<String>, String) = (Vec::new(), Vec::new(), String::new());
    loop {
        match r.read_event_into(&mut buf).map_err(|_| bad("Dokumen Word"))? {
            Event::Start(e) | Event::Empty(e) if matches!(local(&e).as_str(), "tab" | "br" | "cr" | "pStyle" | "numPr") => match local(&e).as_str() {
                "tab" => text.push('\t'),
                "br" | "cr" => text.push('\n'),
                "pStyle" => style = attr(&e, "val"),
                _ => list = true,
            },
            Event::Start(e) => match local(&e).as_str() {
                "p" if table_depth <= 1 => {
                    text.clear();
                    style = None;
                    list = false;
                }
                "t" => in_t = true,
                "tbl" => {
                    table_depth += 1;
                    if table_depth == 1 {
                        table.clear();
                    }
                }
                "tr" if table_depth == 1 => row.clear(),
                "tc" if table_depth == 1 => cell.clear(),
                _ => {}
            },
            Event::Text(t) if in_t => push_text(&mut text, &t),
            Event::End(e) => match String::from_utf8_lossy(e.local_name().as_ref()).as_ref() {
                "t" => in_t = false,
                "p" if table_depth >= 1 => {
                    let t = text.trim();
                    if !t.is_empty() {
                        if !cell.is_empty() {
                            cell.push(' ');
                        }
                        cell.push_str(t);
                    }
                    text.clear();
                }
                "p" => {
                    let t = text.trim().to_string();
                    text.clear();
                    if !t.is_empty() {
                        let heading = style.as_deref().and_then(|s| if s == "Title" { Some(1) } else { s.strip_prefix("Heading").and_then(|n| n.trim().parse::<u8>().ok()) });
                        out.push(match (heading, list) {
                            (Some(level), _) => Block::Heading { level: level.clamp(1, 6), text: t },
                            (None, true) => Block::ListItem { ordered: false, depth: 0, text: t },
                            _ => Block::Paragraph(t),
                        });
                    }
                }
                "tc" if table_depth == 1 => row.push(std::mem::take(&mut cell)),
                "tr" if table_depth == 1 => table.push(std::mem::take(&mut row)),
                "tbl" => {
                    table_depth = table_depth.saturating_sub(1);
                    if table_depth == 0 && !table.is_empty() {
                        out.push(Block::Table(std::mem::take(&mut table)));
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
        if out.len() > MAX_BLOCKS {
            break;
        }
    }
    Ok(out)
}

// ---- odt ---------------------------------------------------------------------------------------

pub fn odt_blocks(bytes: &[u8]) -> Result<Vec<Block>> {
    let mut z = open_zip(bytes)?;
    let xml = need(&mut z, "content.xml", "Dokumen OpenDocument")?;
    let mut r = Reader::from_reader(xml.as_slice());
    let mut buf = Vec::new();
    let mut out: Vec<Block> = Vec::new();
    let (mut text, mut heading, mut item, mut in_text) = (String::new(), None::<u8>, 0usize, false);
    let mut table_depth = 0usize;
    let (mut table, mut row, mut cell): (Vec<Vec<String>>, Vec<String>, String) = (Vec::new(), Vec::new(), String::new());
    loop {
        match r.read_event_into(&mut buf).map_err(|_| bad("Dokumen OpenDocument"))? {
            Event::Empty(e) => match local(&e).as_str() {
                "s" => text.push_str(&" ".repeat(attr(&e, "c").and_then(|c| c.parse::<usize>().ok()).unwrap_or(1).min(200))),
                "tab" => text.push('\t'),
                "line-break" => text.push('\n'),
                _ => {}
            },
            Event::Start(e) => match local(&e).as_str() {
                "h" => {
                    text.clear();
                    in_text = true;
                    heading = Some(attr(&e, "outline-level").and_then(|l| l.parse().ok()).unwrap_or(1u8).clamp(1, 6));
                }
                "p" => {
                    text.clear();
                    in_text = true;
                    heading = None;
                }
                "list-item" => item += 1,
                "table" => {
                    table_depth += 1;
                    if table_depth == 1 {
                        table.clear();
                    }
                }
                "table-row" if table_depth == 1 => row.clear(),
                "table-cell" if table_depth == 1 => cell.clear(),
                _ => {}
            },
            Event::Text(t) if in_text => push_text(&mut text, &t),
            Event::End(e) => match String::from_utf8_lossy(e.local_name().as_ref()).as_ref() {
                "h" | "p" => {
                    in_text = false;
                    let t = text.trim().to_string();
                    text.clear();
                    if t.is_empty() {
                    } else if table_depth >= 1 {
                        if !cell.is_empty() {
                            cell.push(' ');
                        }
                        cell.push_str(&t);
                    } else if let Some(level) = heading.take() {
                        out.push(Block::Heading { level, text: t });
                    } else if item > 0 {
                        out.push(Block::ListItem { ordered: false, depth: (item - 1).min(8) as u8, text: t });
                    } else {
                        out.push(Block::Paragraph(t));
                    }
                }
                "list-item" => item = item.saturating_sub(1),
                "table-cell" if table_depth == 1 => row.push(std::mem::take(&mut cell)),
                "table-row" if table_depth == 1 => table.push(std::mem::take(&mut row)),
                "table" => {
                    table_depth = table_depth.saturating_sub(1);
                    if table_depth == 0 && !table.is_empty() {
                        out.push(Block::Table(std::mem::take(&mut table)));
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
        if out.len() > MAX_BLOCKS {
            break;
        }
    }
    Ok(out)
}

// ---- xlsx --------------------------------------------------------------------------------------

/// Column letters ("A", "AB") to a 0-based index.
fn col_index(cell_ref: &str) -> usize {
    cell_ref.bytes().take_while(|b| b.is_ascii_alphabetic()).fold(0usize, |n, b| n * 26 + (b.to_ascii_uppercase() - b'A') as usize + 1).saturating_sub(1)
}

fn shared_strings(z: &mut ZipArchive<Cursor<&[u8]>>) -> Result<Vec<String>> {
    let Some(xml) = read_part(z, "xl/sharedStrings.xml")? else { return Ok(Vec::new()) };
    let mut r = Reader::from_reader(xml.as_slice());
    let mut buf = Vec::new();
    let (mut out, mut cur, mut in_t, mut in_phonetic) = (Vec::new(), String::new(), false, false);
    loop {
        match r.read_event_into(&mut buf).map_err(|_| bad("Lembar kerja"))? {
            Event::Start(e) => match local(&e).as_str() {
                "si" => cur.clear(),
                "t" if !in_phonetic => in_t = true,
                "rPh" => in_phonetic = true,
                _ => {}
            },
            Event::Text(t) if in_t => push_text(&mut cur, &t),
            Event::End(e) => match String::from_utf8_lossy(e.local_name().as_ref()).as_ref() {
                "t" => in_t = false,
                "rPh" => in_phonetic = false,
                "si" => out.push(std::mem::take(&mut cur)),
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
        if out.len() > 2_000_000 {
            return Err(AppError::new(ErrorCode::ErrPreviewTooLarge, "Lembar kerja terlalu besar untuk dipratinjau."));
        }
    }
    Ok(out)
}

pub fn xlsx_sheets(bytes: &[u8]) -> Result<Vec<Sheet>> {
    let mut z = open_zip(bytes)?;
    let wb = need(&mut z, "xl/workbook.xml", "Lembar kerja Excel")?;
    let rels = need(&mut z, "xl/_rels/workbook.xml.rels", "Lembar kerja Excel")?;
    let mut targets: HashMap<String, String> = HashMap::new();
    {
        let mut r = Reader::from_reader(rels.as_slice());
        let mut buf = Vec::new();
        loop {
            match r.read_event_into(&mut buf).map_err(|_| bad("Lembar kerja Excel"))? {
                Event::Start(e) | Event::Empty(e) if local(&e) == "Relationship" => {
                    if let (Some(id), Some(t)) = (attr(&e, "Id"), attr(&e, "Target")) {
                        targets.insert(id, t);
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
    }
    let mut sheets: Vec<(String, String)> = Vec::new();
    {
        let mut r = Reader::from_reader(wb.as_slice());
        let mut buf = Vec::new();
        loop {
            match r.read_event_into(&mut buf).map_err(|_| bad("Lembar kerja Excel"))? {
                Event::Start(e) | Event::Empty(e) if local(&e) == "sheet" => {
                    if let (Some(name), Some(rid)) = (attr(&e, "name"), attr(&e, "id")) {
                        if let Some(t) = targets.get(&rid) {
                            let path = if let Some(abs) = t.strip_prefix('/') { abs.to_string() } else { format!("xl/{t}") };
                            sheets.push((name, path));
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
    }
    let strings = shared_strings(&mut z)?;
    let mut out = Vec::new();
    for (name, path) in sheets.into_iter().take(MAX_SHEETS) {
        let Some(xml) = read_part(&mut z, &path)? else { continue };
        out.push(parse_sheet(&name, &xml, &strings)?);
    }
    Ok(out)
}

fn parse_sheet(name: &str, xml: &[u8], strings: &[String]) -> Result<Sheet> {
    let mut r = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut rows: Vec<Vec<String>> = Vec::new();
    let (mut truncated, mut row, mut cur_col, mut cell_type, mut value, mut in_v, mut in_t) = (false, Vec::<String>::new(), 0usize, String::new(), String::new(), false, false);
    let mut in_row = false;
    loop {
        match r.read_event_into(&mut buf).map_err(|_| bad("Lembar kerja Excel"))? {
            Event::Start(e) => match local(&e).as_str() {
                "row" => {
                    in_row = true;
                    row.clear();
                }
                "c" if in_row => {
                    cur_col = attr(&e, "r").map(|r| col_index(&r)).unwrap_or(row.len());
                    cell_type = attr(&e, "t").unwrap_or_default();
                    value.clear();
                }
                "v" => in_v = true,
                "t" => in_t = true,
                _ => {}
            },
            Event::Text(t) if in_v || in_t => push_text(&mut value, &t),
            Event::End(e) => match String::from_utf8_lossy(e.local_name().as_ref()).as_ref() {
                "v" => in_v = false,
                "t" => in_t = false,
                "c" if in_row => {
                    if cur_col >= MAX_COLS {
                        truncated = true;
                    } else {
                        let shown = match cell_type.as_str() {
                            "s" => value.trim().parse::<usize>().ok().and_then(|i| strings.get(i).cloned()).unwrap_or_default(),
                            "b" => if value.trim() == "1" { "TRUE".to_string() } else { "FALSE".to_string() },
                            _ => std::mem::take(&mut value),
                        };
                        if row.len() <= cur_col {
                            row.resize(cur_col + 1, String::new());
                        }
                        row[cur_col] = shown;
                    }
                    value.clear();
                }
                "row" => {
                    in_row = false;
                    if rows.len() >= MAX_ROWS {
                        truncated = true;
                    } else {
                        rows.push(std::mem::take(&mut row));
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    while rows.last().map(|r| r.iter().all(|c| c.is_empty())).unwrap_or(false) {
        rows.pop();
    }
    Ok(Sheet { name: name.to_string(), rows, truncated })
}

// ---- ods ---------------------------------------------------------------------------------------

pub fn ods_sheets(bytes: &[u8]) -> Result<Vec<Sheet>> {
    struct St {
        sheets: Vec<Sheet>,
        cur: Option<Sheet>,
        row: Vec<String>,
        cell: String,
        cell_repeat: usize,
        row_repeat: usize,
        value_attr: Option<String>,
        first_p: bool,
    }
    impl St {
        fn start_cell(&mut self, e: &BytesStart<'_>) {
            self.cell.clear();
            self.first_p = true;
            self.cell_repeat = attr(e, "number-columns-repeated").and_then(|n| n.parse().ok()).unwrap_or(1usize);
            self.value_attr = attr(e, "value");
        }
        fn end_cell(&mut self) {
            let text = if self.cell.is_empty() { self.value_attr.take().unwrap_or_default() } else { self.cell.clone() };
            // Empty repeats keep later cells in their columns (trailing ones are trimmed at the row end).
            for _ in 0..self.cell_repeat.min(if text.is_empty() { MAX_COLS } else { 20 }) {
                if self.row.len() < MAX_COLS {
                    self.row.push(text.clone());
                }
            }
        }
        fn start_row(&mut self, e: &BytesStart<'_>) {
            self.row.clear();
            self.row_repeat = attr(e, "number-rows-repeated").and_then(|n| n.parse().ok()).unwrap_or(1usize);
        }
        fn end_row(&mut self) {
            while self.row.last().map(|c| c.is_empty()).unwrap_or(false) {
                self.row.pop();
            }
            if let Some(s) = self.cur.as_mut() {
                for _ in 0..self.row_repeat.min(20) {
                    if s.rows.len() < MAX_ROWS {
                        s.rows.push(self.row.clone());
                    } else {
                        s.truncated = true;
                    }
                }
            }
        }
        fn end_table(&mut self) {
            if let Some(mut s) = self.cur.take() {
                while s.rows.last().map(|r| r.is_empty()).unwrap_or(false) {
                    s.rows.pop();
                }
                self.sheets.push(s);
            }
        }
    }

    let mut z = open_zip(bytes)?;
    let xml = need(&mut z, "content.xml", "Lembar kerja OpenDocument")?;
    let mut r = Reader::from_reader(xml.as_slice());
    let mut buf = Vec::new();
    let mut st = St { sheets: Vec::new(), cur: None, row: Vec::new(), cell: String::new(), cell_repeat: 1, row_repeat: 1, value_attr: None, first_p: true };
    let mut in_p = false;
    loop {
        match r.read_event_into(&mut buf).map_err(|_| bad("Lembar kerja OpenDocument"))? {
            Event::Empty(e) => match local(&e).as_str() {
                "table-cell" | "covered-table-cell" => {
                    st.start_cell(&e);
                    st.end_cell();
                }
                "table-row" => {
                    st.start_row(&e);
                    st.end_row();
                }
                _ => {}
            },
            Event::Start(e) => match local(&e).as_str() {
                "table" => st.cur = Some(Sheet { name: attr(&e, "name").unwrap_or_else(|| "Sheet".into()), rows: Vec::new(), truncated: false }),
                "table-row" => st.start_row(&e),
                "table-cell" | "covered-table-cell" => st.start_cell(&e),
                "p" => {
                    in_p = true;
                    if !st.first_p {
                        st.cell.push('\n');
                    }
                    st.first_p = false;
                }
                _ => {}
            },
            Event::Text(t) if in_p => push_text(&mut st.cell, &t),
            Event::End(e) => match String::from_utf8_lossy(e.local_name().as_ref()).as_ref() {
                "p" => in_p = false,
                "table-cell" | "covered-table-cell" => st.end_cell(),
                "table-row" => st.end_row(),
                "table" => {
                    st.end_table();
                    if st.sheets.len() >= MAX_SHEETS {
                        break;
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(st.sheets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn zip_of(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in parts {
            w.start_file(*name, SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn docx_text_headings_lists_tables() {
        let doc = r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Judul Besar</w:t></w:r></w:p>
<w:p><w:r><w:t>Halo </w:t></w:r><w:r><w:t xml:space="preserve">dunia &amp; semua</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>butir satu</w:t></w:r></w:p>
<w:tbl><w:tr><w:tc><w:p><w:r><w:t>A1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B1</w:t></w:r></w:p><w:p><w:r><w:t>lagi</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>A2</w:t></w:r></w:p></w:tc><w:tc><w:p/></w:tc></w:tr></w:tbl>
<w:p><w:r><w:t>Penutup</w:t></w:r></w:p></w:body></w:document>"#;
        let b = docx_blocks(&zip_of(&[("word/document.xml", doc)])).unwrap();
        assert_eq!(b[0], Block::Heading { level: 1, text: "Judul Besar".into() });
        assert_eq!(b[1], Block::Paragraph("Halo dunia & semua".into()));
        assert_eq!(b[2], Block::ListItem { ordered: false, depth: 0, text: "butir satu".into() });
        assert_eq!(b[3], Block::Table(vec![vec!["A1".into(), "B1 lagi".into()], vec!["A2".into(), String::new()]]));
        assert_eq!(b[4], Block::Paragraph("Penutup".into()));
        assert_eq!(b.len(), 5);
    }

    #[test]
    fn odt_text_and_tables() {
        let content = r#"<?xml version="1.0"?><office:document-content xmlns:office="o" xmlns:text="t" xmlns:table="tb"><office:body><office:text>
<text:h text:outline-level="2">Bagian</text:h>
<text:p>Satu<text:s text:c="2"/>dua<text:tab/>tiga</text:p>
<text:list><text:list-item><text:p>poin</text:p></text:list-item></text:list>
<table:table><table:table-row><table:table-cell><text:p>x</text:p></table:table-cell><table:table-cell><text:p>y</text:p></table:table-cell></table:table-row></table:table>
</office:text></office:body></office:document-content>"#;
        let b = odt_blocks(&zip_of(&[("content.xml", content)])).unwrap();
        assert_eq!(b[0], Block::Heading { level: 2, text: "Bagian".into() });
        assert_eq!(b[1], Block::Paragraph("Satu  dua\ttiga".into()));
        assert_eq!(b[2], Block::ListItem { ordered: false, depth: 0, text: "poin".into() });
        assert_eq!(b[3], Block::Table(vec![vec!["x".into(), "y".into()]]));
    }

    fn xlsx_fixture() -> Vec<u8> {
        zip_of(&[
            ("xl/workbook.xml", r#"<workbook xmlns:r="r"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/><sheet name="Kedua" sheetId="2" r:id="rId2"/></sheets></workbook>"#),
            ("xl/_rels/workbook.xml.rels", r#"<Relationships><Relationship Id="rId1" Target="worksheets/sheet1.xml"/><Relationship Id="rId2" Target="/xl/worksheets/sheet2.xml"/></Relationships>"#),
            ("xl/sharedStrings.xml", r#"<sst><si><t>Nama</t></si><si><r><t>Eko </t></r><r><t>Hadi</t></r></si><si><t>x</t><rPh><t>PHONETIC</t></rPh></si></sst>"#),
            ("xl/worksheets/sheet1.xml", r#"<worksheet><sheetData>
<row r="1"><c r="A1" t="s"><v>0</v></c><c r="C1" t="s"><v>1</v></c></row>
<row r="2"><c r="A2"><v>42.5</v></c><c r="B2" t="b"><v>1</v></c><c r="C2" t="inlineStr"><is><t>inline</t></is></c><c r="D2" t="s"><v>2</v></c></row>
<row r="3"/></sheetData></worksheet>"#),
            ("xl/worksheets/sheet2.xml", r#"<worksheet><sheetData><row r="1"><c r="B1"><v>7</v></c></row></sheetData></worksheet>"#),
        ])
    }

    #[test]
    fn xlsx_sheets_as_value_tables_with_shared_strings() {
        let s = xlsx_sheets(&xlsx_fixture()).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].name, "Data");
        assert_eq!(s[0].rows[0], vec!["Nama", "", "Eko Hadi"], "cells are placed by column letter; rich text runs are joined");
        assert_eq!(s[0].rows[1], vec!["42.5", "TRUE", "inline", "x"], "numbers, booleans, inline strings; phonetic text is skipped");
        assert_eq!(s[0].rows.len(), 2, "the empty trailing row is dropped");
        assert_eq!((s[1].name.as_str(), s[1].rows.clone()), ("Kedua", vec![vec![String::new(), "7".to_string()]]));
        assert!(!s[0].truncated);
    }

    #[test]
    fn ods_sheets() {
        let content = r#"<office:document-content xmlns:office="o" xmlns:text="t" xmlns:table="tb"><office:body><office:spreadsheet>
<table:table table:name="Lembar1">
<table:table-row><table:table-cell office:value-type="string"><text:p>A</text:p></table:table-cell><table:table-cell table:number-columns-repeated="2"/><table:table-cell office:value="3"><text:p>3</text:p></table:table-cell></table:table-row>
<table:table-row table:number-rows-repeated="1000000"><table:table-cell/></table:table-row>
</table:table></office:spreadsheet></office:body></office:document-content>"#;
        let s = super::ods_sheets(&zip_of(&[("content.xml", content)])).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].name, "Lembar1");
        assert_eq!(s[0].rows, vec![vec!["A".to_string(), String::new(), String::new(), "3".to_string()]], "huge empty repeats do not blow up");
    }

    #[test]
    fn zip_bomb_is_refused() {
        // 60 MB of zeros compresses to a few KB.
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        w.start_file("word/document.xml", SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)).unwrap();
        let zeros = vec![b' '; 1024 * 1024];
        for _ in 0..60 {
            w.write_all(&zeros).unwrap();
        }
        let bomb = w.finish().unwrap().into_inner();
        assert!(bomb.len() < 200_000, "the fixture is small on disk: {}", bomb.len());
        assert_eq!(docx_blocks(&bomb).unwrap_err().code, ErrorCode::ErrPreviewTooLarge);
        // Too many entries.
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for i in 0..10_001 {
            w.start_file(format!("f{i}"), SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
        }
        let many = w.finish().unwrap().into_inner();
        assert_eq!(xlsx_sheets(&many).unwrap_err().code, ErrorCode::ErrPreviewTooLarge);
    }

    #[test]
    fn corrupt_and_wrong_files_are_errors_not_panics() {
        assert_eq!(docx_blocks(b"not a zip at all").unwrap_err().code, ErrorCode::ErrPreviewUnsupported);
        assert_eq!(docx_blocks(&zip_of(&[("other.xml", "x")])).unwrap_err().code, ErrorCode::ErrPreviewUnsupported, "missing document part");
        assert!(docx_blocks(&zip_of(&[("word/document.xml", "<w:p><w:t>unclosed")])).is_ok() || docx_blocks(&zip_of(&[("word/document.xml", "<w:p><w:t>unclosed")])).is_err());
    }
}
