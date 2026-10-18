//! The workbooks [`super::XlsxWriter`] writes, read back for tests: each sheet's cells as
//! rust_xlsxwriter writes them (inline strings, numbers, booleans; an omitted or empty cell is
//! [`Cell::Empty`]), and any part's XML. Not a general reader: only what the writer writes.

use std::io::{Cursor, Read};

/// A cell read back.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Empty,
    Number(f64),
    Bool(bool),
    Text(String),
}

/// A part of the workbook (`xl/worksheets/sheet1.xml`) as text, `None` when it has none.
pub fn part(bytes: &[u8], name: &str) -> Option<String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
    let mut text = String::new();
    zip.by_name(name).ok()?.read_to_string(&mut text).ok()?;
    Some(text)
}

/// Every sheet: its name and its cells, row by row from `A1`, each row as wide as the sheet's
/// widest. `None` when the bytes are not a workbook.
pub fn sheets(bytes: &[u8]) -> Option<Vec<(String, Vec<Vec<Cell>>)>> {
    let book = part(bytes, "xl/workbook.xml")?;
    let names: Vec<String> =
        book.split("<sheet ").skip(1).filter_map(|s| attr(s, "name")).map(|n| unescape(&n)).collect();
    names
        .into_iter()
        .enumerate()
        .map(|(i, name)| Some((name, cells(&part(bytes, &format!("xl/worksheets/sheet{}.xml", i + 1))?)?)))
        .collect()
}

/// The value of attribute `name` at the start of `tag` (the text after `<c `).
fn attr(tag: &str, name: &str) -> Option<String> {
    let head = &tag[..tag.find('>')?];
    let at = head
        .find(&format!(" {name}=\""))
        .map(|i| i + 1)
        .or_else(|| head.starts_with(&format!("{name}=\"")).then_some(0))?;
    let rest = &head[at + name.len() + 2..];
    Some(rest[..rest.find('"')?].to_string())
}

/// `A1` as (row, column), from 0.
fn place(r: &str) -> Option<(usize, usize)> {
    let split = r.find(|c: char| c.is_ascii_digit())?;
    let col = r[..split]
        .bytes()
        .try_fold(0usize, |n, b| b.is_ascii_uppercase().then(|| n * 26 + usize::from(b - b'A') + 1))?;
    let row: usize = r[split..].parse().ok()?;
    Some((row.checked_sub(1)?, col.checked_sub(1)?))
}

/// The text between `<tag…>` and `</tag>` in `s` (empty for `<tag/>`).
fn inner<'a>(s: &'a str, tag: &str) -> Option<&'a str> {
    let open = s.find(&format!("<{tag}"))?;
    let after = &s[open..];
    let end = after.find('>')?;
    if after[..end].ends_with('/') {
        return Some("");
    }
    let body = &after[end + 1..];
    Some(&body[..body.find(&format!("</{tag}>"))?])
}

/// XML text with its entities and Excel's `_xHHHH_` escapes undone.
fn unescape(s: &str) -> String {
    let s = s
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(i) = rest.find("_x") {
        let code = rest
            .get(i + 2..i + 6)
            .filter(|h| rest.get(i + 6..i + 7) == Some("_") && h.bytes().all(|b| b.is_ascii_hexdigit()));
        match code.and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32) {
            Some(c) => {
                out.push_str(&rest[..i]);
                out.push(c);
                rest = &rest[i + 7..];
            }
            None => {
                out.push_str(&rest[..i + 2]);
                rest = &rest[i + 2..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The cells of a sheet's XML.
fn cells(xml: &str) -> Option<Vec<Vec<Cell>>> {
    let data = inner(xml, "sheetData")?;
    let mut placed = Vec::new();
    for c in data.split("<c ").skip(1) {
        let (row, col) = place(&attr(c, "r")?)?;
        let head_end = c.find('>')?;
        let cell = if c[..head_end].ends_with('/') {
            Cell::Empty
        } else {
            let body = &c[head_end + 1..c.find("</c>")?];
            match attr(c, "t").as_deref() {
                Some("inlineStr") => Cell::Text(unescape(inner(body, "t")?)),
                Some("b") => Cell::Bool(inner(body, "v")? == "1"),
                None | Some("n") => Cell::Number(inner(body, "v")?.parse().ok()?),
                Some(_) => return None,
            }
        };
        placed.push((row, col, cell));
    }
    let rows = placed.iter().map(|(r, _, _)| r + 1).max().unwrap_or(0);
    let width = placed.iter().map(|(_, c, _)| c + 1).max().unwrap_or(0);
    let mut out = vec![vec![Cell::Empty; width]; rows];
    for (r, c, cell) in placed {
        out[r][c] = cell;
    }
    Some(out)
}
