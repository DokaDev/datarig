use super::*;
use crate::export::{
    Target, UpdateTarget, csv as csv_text, html, indent_json, json, json_pretty, markdown, sql_insert as insert,
    sql_update as update, tsv as tsv_text, xml,
};
use crate::sql::dialect::Dialect;

/// The in-memory writer as it was before the stream writer, kept as the oracle: a copy's text
/// must not change by a byte.
struct OldWriter<'a> {
    format: Format<'a>,
    columns: &'a [Column<'a>],
    out: String,
    rows: usize,
}

impl<'a> OldWriter<'a> {
    fn new(format: Format<'a>, columns: &'a [Column<'a>]) -> Self {
        Self { format, columns, out: String::new(), rows: 0 }
    }

    fn rows(&mut self, rows: &[Row]) {
        if rows.is_empty() {
            return;
        }
        let first = self.rows == 0;
        let part = match &self.format {
            Format::Tsv { header } => tsv_text(self.columns, rows, *header && first),
            Format::Csv { header } => csv_text(self.columns, rows, *header && first),
            Format::Json | Format::JsonPretty => json_objects(self.columns, rows),
            Format::Markdown => markdown_rows(rows),
            Format::Sql(d, target) => insert(*d, target, self.columns, rows),
            Format::List => comma_list(rows),
            Format::Html => html_rows(rows),
            Format::Xml => xml_rows(self.columns, rows),
            Format::Update(d, target) => update(*d, target, self.columns, rows),
            Format::Delimited(_) | Format::JsonLines => unreachable!("not a copy format"),
        };
        if !first {
            match self.format {
                Format::Json | Format::JsonPretty => self.out.push_str(",\n"),
                Format::List => self.out.push_str(", "),
                Format::Html | Format::Xml => {}
                _ => self.out.push('\n'),
            }
        }
        self.out.push_str(&part);
        self.rows += rows.len();
    }

    fn finish(self) -> String {
        match self.format {
            Format::Tsv { header } if self.rows == 0 => tsv_text(self.columns, &[], header),
            Format::Csv { header } if self.rows == 0 => csv_text(self.columns, &[], header),
            Format::Json if self.rows == 0 => "[]".to_string(),
            Format::Json => format!("[\n{}\n]", self.out),
            Format::Markdown if self.rows == 0 => markdown_header(self.columns),
            Format::Markdown => format!("{}\n{}", markdown_header(self.columns), self.out),
            Format::JsonPretty if self.rows == 0 => "[]".to_string(),
            Format::JsonPretty => indent_json(&format!("[\n{}\n]", self.out)),
            Format::Html => format!("{}{}{HTML_END}", html_head(self.columns), self.out),
            Format::Xml => format!("{XML_HEAD}{}{XML_END}", self.out),
            _ => self.out,
        }
    }
}

/// A small deterministic generator (xorshift), so a failure repeats.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Pieces values are made of: delimiters, quotes, line breaks, markup, JSON, numbers, NUL-free
/// control characters and text in several scripts (Hangul, Han, Cyrillic, an emoji), written
/// with escapes.
const PIECES: &[&str] = &[
    "a",
    "x y",
    ",",
    ";",
    "\t",
    "\n",
    "\r\n",
    "\r",
    "\"",
    "'",
    "\\",
    "|",
    "<b>",
    "&amp;",
    "{\"k\": [1, 2]}",
    "[]",
    "{}",
    "12",
    "-0.5",
    "1e3",
    "NaN",
    "true",
    "NULL",
    "\u{1}",
    "\u{7f}",
    "\u{d55c}\u{ae00}",
    "\u{c548}\u{b155}\u{d558}\u{c138}\u{c694}",
    "\u{9673}\u{5927}",
    "\u{416}",
    "\u{1f418}",
    " ",
    "",
];

fn value(rng: &mut Rng) -> Option<String> {
    if rng.below(6) == 0 {
        return None;
    }
    let n = rng.below(4);
    Some((0..n).map(|_| PIECES[rng.below(PIECES.len())]).collect())
}

fn columns() -> Vec<Column<'static>> {
    vec![
        Column { name: "id", kind: Kind::Number },
        Column { name: "name", kind: Kind::Text },
        Column { name: "ok", kind: Kind::Bool },
        Column { name: "doc", kind: Kind::Json },
        Column { name: "tags", kind: Kind::Array(crate::export::Element::Text) },
        Column { name: "name", kind: Kind::Text },
        Column { name: "\u{d55c} \"q\"", kind: Kind::Text },
    ]
}

fn table(rng: &mut Rng, n: usize, width: usize) -> Vec<Vec<Option<String>>> {
    (0..n).map(|_| (0..width).map(|_| value(rng)).collect()).collect()
}

fn as_rows(t: &[Vec<Option<String>>]) -> Vec<Row<'_>> {
    t.iter().map(|r| r.iter().map(|v| v.as_deref()).collect()).collect()
}

/// Random cut points of `n` rows into chunks (some empty).
fn splits(rng: &mut Rng, n: usize) -> Vec<usize> {
    let mut cuts = vec![0];
    let mut at = 0;
    while at < n {
        at = (at + rng.below(4)).min(n);
        cuts.push(at);
    }
    cuts.push(n);
    cuts
}

fn copy_formats() -> Vec<Format<'static>> {
    let table =
        Target::Table { schema: "s", name: "t", columns: vec!["a", "b", "c", "d", "e", "f", "g"], overriding: false };
    let upd = UpdateTarget { schema: "s", name: "t", set: vec![(1, "name"), (3, "doc")], keys: vec![(0, "id")] };
    vec![
        Format::Tsv { header: false },
        Format::Tsv { header: true },
        Format::Csv { header: false },
        Format::Csv { header: true },
        Format::Json,
        Format::JsonPretty,
        Format::Markdown,
        Format::Sql(Dialect::Postgres, table.clone()),
        Format::Sql(Dialect::MySql(Default::default()), Target::Unknown),
        Format::List,
        Format::Html,
        Format::Xml,
        Format::Update(Dialect::Postgres, upd.clone()),
        Format::Update(Dialect::MySql(Default::default()), upd),
    ]
}

#[test]
fn copies_are_byte_identical_to_the_writer_before_the_stream_writer() {
    let cols = columns();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for round in 0..300 {
        let n = rng.below(12);
        let t = table(&mut rng, n, cols.len());
        let rows = as_rows(&t);
        for format in copy_formats() {
            let cuts = splits(&mut rng, rows.len());
            let mut old = OldWriter::new(format.clone(), &cols);
            let mut new = crate::export::Writer::new(format.clone(), &cols);
            for w in cuts.windows(2) {
                old.rows(&rows[w[0]..w[1]]);
                new.rows(&rows[w[0]..w[1]]);
            }
            assert_eq!(new.count(), rows.len());
            assert_eq!(new.finish(), old.finish(), "round {round}, {format:?}, cuts {cuts:?}");
        }
    }
}

#[test]
fn copies_match_the_functions_for_all_rows_at_once() {
    let cols = columns();
    let mut rng = Rng(7);
    for _ in 0..100 {
        let n = rng.below(9);
        let t = table(&mut rng, n, cols.len());
        let rows = as_rows(&t);
        let mut whole = |format: Format<'static>| {
            let mut w = crate::export::Writer::new(format, &cols);
            for r in rows.chunks(1 + rng.below(3)) {
                w.rows(r);
            }
            w.finish()
        };
        assert_eq!(whole(Format::Tsv { header: true }), tsv_text(&cols, &rows, true));
        assert_eq!(whole(Format::Csv { header: true }), csv_text(&cols, &rows, true));
        assert_eq!(whole(Format::Json), json(&cols, &rows));
        assert_eq!(whole(Format::JsonPretty), json_pretty(&cols, &rows));
        assert_eq!(whole(Format::Markdown), markdown(&cols, &rows));
        assert_eq!(whole(Format::Html), html(&cols, &rows));
        assert_eq!(whole(Format::Xml), xml(&cols, &rows));
    }
}

#[test]
fn the_streaming_indenter_indents_as_the_whole_text_at_every_split() {
    let mut rng = Rng(42);
    let cols = columns();
    let mut texts = vec![
        "[]".to_string(),
        "{}".to_string(),
        "[ ]".to_string(),
        "{ \n }".to_string(),
        "[{}, [], {\"a\": []}, [ {} ]]".to_string(),
        "{\"a\\\"b\": \"x,y:{z}\", \"c\": [1, {\"d\": null}]}".to_string(),
        "[".to_string(),
        "\"\\\\\"".to_string(),
    ];
    for _ in 0..40 {
        let n = rng.below(5);
        let t = table(&mut rng, n, cols.len());
        texts.push(json(&cols, &as_rows(&t)));
    }
    for text in texts {
        let want = indent_json(&text);
        let chars: Vec<char> = text.chars().collect();
        for cut in 0..=chars.len() {
            let (a, b): (String, String) = (chars[..cut].iter().collect(), chars[cut..].iter().collect());
            let mut ind = Indenter::default();
            let mut out = String::new();
            ind.feed(&a, &mut out);
            ind.feed(&b, &mut out);
            ind.finish(&mut out);
            assert_eq!(out, want, "{text:?} cut at {cut}");
        }
    }
}

/// A file's text: written by a [`TextWriter`] with a trailing line break, in chunks.
fn file(format: Format<'_>, cols: &[Column<'_>], rows: &[Row], chunk: usize) -> (String, u64) {
    let mut w = TextWriter::new(format, cols, Vec::new()).trailing_newline(true);
    w.begin().unwrap();
    for c in rows.chunks(chunk.max(1)) {
        w.rows(c).unwrap();
    }
    w.end().unwrap();
    assert_eq!(w.rows_written(), rows.len() as u64);
    let damaged = w.damaged();
    let bytes = w.bytes();
    let out = String::from_utf8(w.into_inner()).unwrap();
    assert_eq!(bytes, out.len() as u64);
    (out, damaged)
}

/// RFC 4180 with `delimiter`: the fields of each record, a field quoted or not (`None` for an
/// unquoted field equal to `null`).
fn parse_delimited(text: &str, delimiter: char, null: &str) -> Vec<Vec<Option<String>>> {
    let mut out = Vec::new();
    let mut record: Vec<Option<String>> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    let mut any = false;
    let close = |field: &mut String, quoted: &mut bool, record: &mut Vec<Option<String>>| {
        let f = std::mem::take(field);
        record.push(if !*quoted && f == null { None } else { Some(f) });
        *quoted = false;
    };
    while let Some(c) = chars.next() {
        any = true;
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' => {
                in_quotes = true;
                quoted = true;
            }
            '\n' => {
                close(&mut field, &mut quoted, &mut record);
                out.push(std::mem::take(&mut record));
                any = false;
            }
            c if c == delimiter => close(&mut field, &mut quoted, &mut record),
            c => field.push(c),
        }
    }
    assert!(!in_quotes, "a quote left open");
    if any {
        close(&mut field, &mut quoted, &mut record);
        out.push(record);
    }
    out
}

fn owned(rows: &[Row]) -> Vec<Vec<Option<String>>> {
    rows.iter().map(|r| r.iter().map(|v| v.map(str::to_string)).collect()).collect()
}

#[test]
fn csv_files_read_back_as_the_rows_with_every_delimiter() {
    let cols = columns();
    let names: Vec<Option<String>> = cols.iter().map(|c| Some(c.name.to_string())).collect();
    let mut rng = Rng(99);
    for delimiter in [',', ';', '\t', '|', '\u{b7}'] {
        for null in ["", "NULL", "\\N"] {
            for quote in [Quote::Necessary, Quote::Always, Quote::NonNumeric] {
                let n = 1 + rng.below(10);
                let t = table(&mut rng, n, cols.len());
                let rows = as_rows(&t);
                let d = Delimited { delimiter, quote, header: true, null: null.to_string() };
                d.check().unwrap();
                let (text, damaged) = file(Format::Delimited(d), &cols, &rows, 1 + rng.below(4));
                assert_eq!(damaged, 0);
                assert!(text.ends_with('\n'));
                let back = parse_delimited(&text, delimiter, null);
                assert_eq!(back[0], names, "{delimiter:?} {quote:?}");
                assert_eq!(&back[1..], owned(&rows).as_slice(), "{delimiter:?} {null:?} {quote:?}\n{text}");
            }
        }
    }
}

#[test]
fn csv_by_default_is_rfc_4180_and_the_copy_text_plus_a_line_break() {
    let cols = columns();
    let mut rng = Rng(5);
    let t = table(&mut rng, 20, cols.len());
    let rows = as_rows(&t);
    let (text, _) = file(Format::Delimited(Delimited::default()), &cols, &rows, 3);
    assert_eq!(text, format!("{}\n", csv_text(&cols, &rows, true)));
    let (empty, _) = file(Format::Delimited(Delimited::default()), &cols, &[], 3);
    assert_eq!(empty, format!("{}\n", csv_text(&cols, &[], true)));
    let none = Delimited { header: false, ..Delimited::default() };
    let (nothing, _) = file(Format::Delimited(none), &cols, &[], 3);
    assert_eq!(nothing, "", "no header and no rows: an empty file");
}

#[test]
fn quotes_follow_the_style() {
    let cols = [Column { name: "n", kind: Kind::Number }, Column { name: "t", kind: Kind::Text }];
    let rows: Vec<Row> = vec![vec![Some("12"), Some("a")], vec![Some("x"), None], vec![Some(""), Some("")]];
    let d = |quote| Delimited { quote, ..Delimited::default() };
    assert_eq!(file(Format::Delimited(d(Quote::Necessary)), &cols, &rows, 2).0, "n,t\n12,a\nx,\n\"\",\"\"\n");
    assert_eq!(
        file(Format::Delimited(d(Quote::Always)), &cols, &rows, 2).0,
        "\"n\",\"t\"\n\"12\",\"a\"\n\"x\",\n\"\",\"\"\n"
    );
    assert_eq!(
        file(Format::Delimited(d(Quote::NonNumeric)), &cols, &rows, 2).0,
        "\"n\",\"t\"\n12,\"a\"\n\"x\",\n\"\",\"\"\n"
    );
    let null = Delimited { null: "NULL".into(), ..Delimited::default() };
    let rows: Vec<Row> = vec![vec![Some("NULL"), None]];
    assert_eq!(
        file(Format::Delimited(null), &cols, &rows, 1).0,
        "n,t\n\"NULL\",NULL\n",
        "a value that reads as NULL is quoted"
    );
}

#[test]
fn suppressed_quotes_count_the_values_they_break() {
    let cols = [Column { name: "a,b", kind: Kind::Text }, Column { name: "c", kind: Kind::Text }];
    let rows: Vec<Row> = vec![vec![Some("x,y"), Some("ok")], vec![Some("line\nbreak"), Some("say \"hi\"")]];
    let d = Delimited { quote: Quote::Never, ..Delimited::default() };
    let (text, damaged) = file(Format::Delimited(d), &cols, &rows, 1);
    assert_eq!(text, "a,b,c\nx,y,ok\nline\nbreak,say \"hi\"\n");
    assert_eq!(damaged, 4, "the header's name and three values");
    let tab = Delimited { delimiter: '\t', quote: Quote::Never, ..Delimited::default() };
    let (text, damaged) = file(Format::Delimited(tab), &cols, &rows[..1], 1);
    assert_eq!(text, "a,b\tc\nx,y\tok\n");
    assert_eq!(damaged, 0, "commas do not break tab-separated columns");
}

#[test]
fn delimiters_that_cannot_be_read_back_are_refused() {
    let d = |delimiter: char, null: &str| Delimited { delimiter, null: null.into(), ..Delimited::default() };
    assert_eq!(d('"', "").check(), Err(BadDelimited::Delimiter));
    assert_eq!(d('\n', "").check(), Err(BadDelimited::Delimiter));
    assert_eq!(d(',', "a,b").check(), Err(BadDelimited::Null));
    assert_eq!(d(';', "\"").check(), Err(BadDelimited::Null));
    assert_eq!(d('\t', "\\N").check(), Ok(()));
}

#[test]
fn json_files_parse_with_types_nulls_and_unicode() {
    let cols = columns();
    let mut rng = Rng(3);
    for format in [Format::Json, Format::JsonPretty] {
        for n in [0, 1, 7] {
            let t = table(&mut rng, n, cols.len());
            let rows = as_rows(&t);
            let (text, _) = file(format.clone(), &cols, &rows, 2);
            assert!(text.ends_with('\n'));
            let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
            let items = v.as_array().unwrap();
            assert_eq!(items.len(), n);
            for (item, row) in items.iter().zip(&rows) {
                let obj = item.as_object().unwrap();
                assert_eq!(obj.len(), cols.len(), "a name used twice gets a suffix");
                assert_eq!(obj["name"].as_str(), row[1], "text stays text");
                assert_eq!(obj["name_2"].as_str(), row[5]);
                assert_eq!(obj["\u{d55c} \"q\""].as_str(), row[6], "Hangul keys and values");
                if row[0].is_none() {
                    assert!(obj["id"].is_null());
                }
            }
        }
    }
}

#[test]
fn json_lines_files_have_one_object_per_line() {
    let cols = columns();
    let mut rng = Rng(11);
    let t = table(&mut rng, 25, cols.len());
    let rows = as_rows(&t);
    let (text, _) = file(Format::JsonLines, &cols, &rows, 4);
    let lines: Vec<&str> = text.split_terminator('\n').collect();
    assert_eq!(lines.len(), rows.len());
    for (line, row) in lines.iter().zip(&rows) {
        let v: serde_json::Value = serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}"));
        assert_eq!(v["name"].as_str(), row[1]);
        assert_eq!(v["\u{d55c} \"q\""].as_str(), row[6]);
    }
    assert_eq!(file(Format::JsonLines, &cols, &[], 4).0, "", "no rows: an empty file");
    let one: Vec<Row> = vec![vec![Some("1"), Some("\u{c548}\u{b155}"), Some("true"), None, None, None, None]];
    assert_eq!(
        file(Format::JsonLines, &cols, &one, 1).0,
        "{\"id\":1,\"name\":\"\u{c548}\u{b155}\",\"ok\":true,\"doc\":null,\"tags\":null,\"name_2\":null,\"\u{d55c} \\\"q\\\"\":null}\n"
    );
}

/// XML or HTML text with its entities read back.
fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&#13;", "\r")
        .replace("&amp;", "&")
}

/// The `<column>` elements of `<row>`s: (name, value; `None` for `null="true"`).
fn parse_xml(text: &str) -> Vec<Vec<(String, Option<String>)>> {
    assert!(text.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rows>\n"));
    assert!(text.ends_with("</rows>\n"));
    let body = &text[text.find("<rows>\n").unwrap() + 7..text.len() - "</rows>\n".len()];
    let mut out = Vec::new();
    for row in body.split("  <row>\n").filter(|r| !r.is_empty()) {
        let row = row.strip_suffix("  </row>\n").expect("a row ends");
        let mut cells = Vec::new();
        let mut rest = row;
        while let Some(start) = rest.find("<column name=\"") {
            rest = &rest[start + 14..];
            let q = rest.find('"').unwrap();
            let name = unescape(&rest[..q]);
            rest = &rest[q + 1..];
            if let Some(r) = rest.strip_prefix(" null=\"true\"/>\n") {
                cells.push((name, None));
                rest = r;
            } else {
                let r = rest.strip_prefix('>').unwrap();
                let end = r.find("</column>\n").unwrap();
                assert!(!r[..end].contains('<'), "text holds no markup");
                cells.push((name, Some(unescape(&r[..end]))));
                rest = &r[end + 10..];
            }
        }
        out.push(cells);
    }
    out
}

/// What XML 1.0 keeps of `v` ([`crate::export::xml_text`]).
fn xml_kept(v: &str) -> String {
    v.chars().map(|c| if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') { '\u{FFFD}' } else { c }).collect()
}

#[test]
fn xml_files_read_back_as_the_rows() {
    let cols = columns();
    let mut rng = Rng(17);
    let t = table(&mut rng, 30, cols.len());
    let rows = as_rows(&t);
    let (text, _) = file(Format::Xml, &cols, &rows, 7);
    let back = parse_xml(&text);
    assert_eq!(back.len(), rows.len());
    for (cells, row) in back.iter().zip(&rows) {
        for ((name, v), (c, want)) in cells.iter().zip(cols.iter().zip(row)) {
            assert_eq!(name, c.name);
            assert_eq!(v.as_deref(), want.map(xml_kept).as_deref());
        }
    }
    assert_eq!(parse_xml(&file(Format::Xml, &cols, &[], 7).0).len(), 0);
}

#[test]
fn html_files_are_one_table_with_a_row_per_row() {
    let cols = columns();
    let mut rng = Rng(23);
    let t = table(&mut rng, 12, cols.len());
    let rows = as_rows(&t);
    let (text, _) = file(Format::Html, &cols, &rows, 5);
    assert!(text.starts_with("<table>\n  <thead>\n"));
    assert!(text.ends_with("  </tbody>\n</table>\n"));
    assert_eq!(text.matches("<tr>").count(), rows.len() + 1);
    assert_eq!(text.matches("<td>").count(), rows.len() * cols.len());
    let body = &text[text.find("<tbody>\n").unwrap()..];
    for (line, row) in body.lines().skip(1).zip(&rows) {
        let cells: Vec<String> = line
            .trim()
            .strip_prefix("<tr><td>")
            .and_then(|l| l.strip_suffix("</td></tr>"))
            .unwrap()
            .split("</td><td>")
            .map(unescape)
            .collect();
        for (cell, v) in cells.iter().zip(row) {
            match v {
                None => assert_eq!(cell, "NULL"),
                Some(v) if !v.contains(['\n', '\r']) && !v.chars().any(char::is_control) => assert_eq!(cell, v),
                Some(_) => {}
            }
        }
    }
}

#[test]
fn markdown_files_are_a_table_with_a_line_per_row() {
    let cols = columns();
    let mut rng = Rng(29);
    let t = table(&mut rng, 9, cols.len());
    let rows = as_rows(&t);
    let (text, _) = file(Format::Markdown, &cols, &rows, 2);
    let lines: Vec<&str> = text.split_terminator('\n').collect();
    assert_eq!(lines.len(), rows.len() + 2, "a header, its rule and one line per row:\n{text}");
    for line in &lines {
        assert!(line.starts_with("| ") && line.ends_with(" |"));
        // Cells split at the bars that are not escaped.
        let mut cells = 0;
        let mut escaped = false;
        for c in line.chars() {
            match c {
                '\\' if !escaped => escaped = true,
                '|' if !escaped => cells += 1,
                _ => escaped = false,
            }
        }
        assert_eq!(cells, cols.len() + 1, "{line}");
    }
    assert_eq!(file(Format::Markdown, &cols, &[], 2).0.lines().count(), 2);
}

#[test]
fn sql_files_end_every_statement_and_the_file_with_a_line_break() {
    let cols = [Column { name: "id", kind: Kind::Number }, Column { name: "t", kind: Kind::Text }];
    let rows: Vec<Row> = vec![vec![Some("1"), Some("a'b")], vec![Some("2"), None]];
    let target = Target::Table { schema: "s", name: "t", columns: vec!["id", "t"], overriding: false };
    let (text, _) = file(Format::Sql(Dialect::Postgres, target), &cols, &rows, 1);
    assert_eq!(
        text,
        "INSERT INTO \"s\".\"t\" (\"id\", \"t\") VALUES (1, 'a''b');\nINSERT INTO \"s\".\"t\" (\"id\", \"t\") VALUES (2, NULL);\n"
    );
    let upd = UpdateTarget { schema: "s", name: "t", set: vec![(1, "t")], keys: vec![(0, "id")] };
    let (text, _) = file(Format::Update(Dialect::MySql(Default::default()), upd), &cols, &rows, 2);
    assert_eq!(
        text,
        "UPDATE `s`.`t` SET `t` = 'a''b' WHERE `id` = 1;\nUPDATE `s`.`t` SET `t` = NULL WHERE `id` = 2;\n"
    );
}

#[test]
fn tsv_files_are_the_copy_text_and_a_line_break() {
    let cols = columns();
    let mut rng = Rng(31);
    let t = table(&mut rng, 6, cols.len());
    let rows = as_rows(&t);
    let (text, _) = file(Format::Tsv { header: true }, &cols, &rows, 4);
    assert_eq!(text, format!("{}\n", tsv_text(&cols, &rows, true)));
}

#[test]
fn a_stopped_export_still_ends_its_format() {
    let cols = columns();
    let mut rng = Rng(37);
    let t = table(&mut rng, 3, cols.len());
    let rows = as_rows(&t);
    // Fewer rows than there were, then `end`: still well-formed.
    let (text, _) = file(Format::JsonPretty, &cols, &rows[..2], 1);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 2);
    let (text, _) = file(Format::Xml, &cols, &rows[..1], 1);
    assert_eq!(parse_xml(&text).len(), 1);
}

#[test]
fn every_text_format_writes_the_same_bytes_whatever_the_chunks() {
    let cols = columns();
    let mut rng = Rng(41);
    let t = table(&mut rng, 40, cols.len());
    let rows = as_rows(&t);
    let mut formats = copy_formats();
    formats.push(Format::JsonLines);
    formats.push(Format::Delimited(Delimited { delimiter: '\t', quote: Quote::NonNumeric, ..Delimited::default() }));
    for format in formats {
        let (whole, _) = file(format.clone(), &cols, &rows, rows.len());
        for chunk in [1, 2, 3, 7, 39] {
            assert_eq!(file(format.clone(), &cols, &rows, chunk).0, whole, "{format:?} by {chunk}");
        }
    }
}

#[test]
fn json_values_with_line_breaks_keep_json_lines_one_line_per_row() {
    let cols = [
        Column { name: "doc", kind: Kind::Json },
        Column { name: "docs", kind: Kind::Array(crate::export::Element::Json) },
        Column { name: "t", kind: Kind::Text },
    ];
    let doc = "{\n  \"a\": [1,\n 2],\n \"s\": \"x\\ny\",\n \"big\": 123456789012345678901234567890\n}";
    // `json[]` as PostgreSQL writes it: an element quoted, with a line break inside.
    let arr = "{\"{\n \\\"b\\\": 1\n}\"}";
    let rows: Vec<Row> = vec![vec![Some(doc), Some(arr), Some("line\nbreak")], vec![None, None, None]];
    let (text, _) = file(Format::JsonLines, &cols, &rows, 1);
    let lines: Vec<&str> = text.split_terminator('\n').collect();
    assert_eq!(lines.len(), rows.len(), "{text}");
    let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(v["doc"]["s"], serde_json::json!("x\ny"), "text inside strings is kept");
    assert_eq!(v["docs"][0]["b"], serde_json::json!(1));
    assert!(lines[0].contains("123456789012345678901234567890"), "numbers keep every digit");
    // The array formats stay valid JSON with the same values.
    for f in [Format::Json, Format::JsonPretty] {
        let (text, _) = file(f, &cols, &rows, 1);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
        assert_eq!(v[0]["doc"]["a"], serde_json::json!([1, 2]));
    }
}
