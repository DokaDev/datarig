use super::*;
use crate::export::{self, Column, Kind};

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn rows(t: &[&[&str]]) -> Vec<Vec<Value>> {
    t.iter().map(|r| r.iter().map(|v| text(v)).collect()).collect()
}

fn read(s: &str) -> Result<Pasted, PasteError> {
    parse(s, &PasteOpts::default())
}

fn ok(s: &str) -> (Format, Vec<Vec<Value>>) {
    let p = read(s).unwrap_or_else(|e| panic!("{s:?}: {e:?}"));
    (p.format, p.rows)
}

/// `s` read as CSV, as asked.
fn csv(s: &str) -> Result<Vec<Vec<Value>>, PasteError> {
    parse_as(s, Format::Csv, &PasteOpts::default()).map(|p| p.rows)
}

fn ambiguous(csv: Result<usize, PasteError>, plain: Result<Format, PasteError>) -> Result<Pasted, PasteError> {
    Err(PasteError::Ambiguous { csv: csv.map_err(Box::new), plain: plain.map_err(Box::new) })
}

#[test]
fn the_format_is_tsv_then_one_value_per_line_then_one_value_and_commas_are_asked_about() {
    assert_eq!(ok("a\tb\nc\td"), (Format::Tsv, rows(&[&["a", "b"], &["c", "d"]])));
    assert_eq!(read("a,b\nc,d"), ambiguous(Ok(2), Ok(Format::Column)));
    assert_eq!(csv("a,b\nc,d"), Ok(rows(&[&["a", "b"], &["c", "d"]])));
    assert_eq!(ok("a\nb\n"), (Format::Column, rows(&[&["a"], &["b"]])));
    assert_eq!(ok("a b"), (Format::One, rows(&[&["a b"]])));
    // A tab anywhere wins over commas.
    assert_eq!(read("a,b\nc\td"), Err(PasteError::Ragged { row: 2, values: 2, first_row: 1, first_values: 1 }));
    // A comma on any line is asked about.
    let ragged = PasteError::Ragged { row: 2, values: 2, first_row: 1, first_values: 1 };
    assert_eq!(read("a\nb,c"), ambiguous(Err(ragged), Ok(Format::Column)));
    // A comma inside a TSV cell is the cell's.
    assert_eq!(ok("1,5\tx"), (Format::Tsv, rows(&[&["1,5", "x"]])));
    // A tab inside a CSV quote is the value's, and not a TSV.
    assert_eq!(read("\"a\tb\",c"), ambiguous(Ok(2), Ok(Format::One)));
    assert_eq!(csv("\"a\tb\",c"), Ok(rows(&[&["a\tb", "c"]])));
    // A comma inside a quote is not asked about.
    assert_eq!(ok("\"a,b\"\n\"c,d\""), (Format::Column, rows(&[&["a,b"], &["c,d"]])));
    // A line break inside a quote is not a second line.
    assert_eq!(ok("\"a\nb\""), (Format::One, rows(&[&["a\nb"]])));
}

#[test]
fn quotes_are_read_as_rfc_4180_in_tsv_and_csv() {
    assert_eq!(ok("\"a\"\"b\"\tc"), (Format::Tsv, rows(&[&["a\"b", "c"]])));
    assert_eq!(csv("\"x\ny\",z\nu,v"), Ok(rows(&[&["x\ny", "z"], &["u", "v"]])));
    assert_eq!(ok("\"\"\t\"\t\""), (Format::Tsv, rows(&[&["", "\t"]])));
    // A quote inside an unquoted field is a character of it.
    assert_eq!(ok("say \"hi\"\tb"), (Format::Tsv, rows(&[&["say \"hi\"", "b"]])));
    assert_eq!(ok("5\"\t6'"), (Format::Tsv, rows(&[&["5\"", "6'"]])));
    // Open, or followed by text: refused.
    assert_eq!(read("a\t\"b\nc\td"), Err(PasteError::Quote { row: 1 }));
    assert_eq!(read("a\tb\n\"x\"y\tc"), Err(PasteError::Quote { row: 2 }));
}

#[test]
fn a_quote_left_open_is_refused_and_never_read_as_one_value() {
    // What the open quote hides (tabs, line breaks) is not known: no format is guessed.
    assert_eq!(read("\"abc\tdef\nx\ty"), Err(PasteError::Quote { row: 1 }));
    assert_eq!(read("\"a\nb"), Err(PasteError::Quote { row: 1 }));
    assert_eq!(read("\""), Err(PasteError::Quote { row: 1 }));
    assert_eq!(read("\"Hello\n"), Err(PasteError::Quote { row: 1 }));
    // After a line break: one value per line, which refuses it in its row.
    assert_eq!(read("a\n\"b\tc"), Err(PasteError::Quote { row: 2 }));
    // Read as one value on purpose, it is the text as it is.
    let one = parse_as("\"abc\tdef\nx\ty", Format::One, &PasteOpts::default());
    assert_eq!(one.map(|p| p.rows), Ok(rows(&[&["\"abc\tdef\nx\ty"]])));
    // A quote closed and followed by text is one value as it is.
    assert_eq!(ok("\"Hello\" world"), (Format::One, rows(&[&["\"Hello\" world"]])));
}

#[test]
fn a_byte_order_mark_at_the_start_is_not_read() {
    assert_eq!(csv("\u{FEFF}\"a,b\",c"), Ok(rows(&[&["a,b", "c"]])));
    assert_eq!(ok("\u{FEFF}a\tb"), (Format::Tsv, rows(&[&["a", "b"]])));
    let p = parse_as("\u{FEFF}\"x\"", Format::Column, &PasteOpts::default());
    assert_eq!(p.map(|p| p.rows), Ok(rows(&[&["x"]])));
    // Only one, and only at the start.
    assert_eq!(ok("a\u{FEFF}").1, rows(&[&["a\u{FEFF}"]]));
}

#[test]
fn line_ends_are_lf_or_crlf_and_a_lone_cr_is_refused() {
    assert_eq!(ok("a\tb\r\nc\td\r\n"), (Format::Tsv, rows(&[&["a", "b"], &["c", "d"]])));
    assert_eq!(read("a\tb\rc\td"), Err(PasteError::LoneCr { row: 1 }));
    assert_eq!(read("a\tb\nc\td\r"), Err(PasteError::LoneCr { row: 2 }));
    assert_eq!(read("a\rb"), Err(PasteError::LoneCr { row: 1 }));
    assert_eq!(ok("a\r\nb"), (Format::Column, rows(&[&["a"], &["b"]])));
    // Inside quotes every character is the value's.
    assert_eq!(ok("\"a\r\nb\"\t\"c\rd\""), (Format::Tsv, rows(&[&["a\r\nb", "c\rd"]])));
    // One value: its CRLF are line feeds.
    assert_eq!(parse_as("a\r\nb\r\n", Format::One, &PasteOpts::default()).unwrap().rows, rows(&[&["a\nb"]]));
}

#[test]
fn one_final_line_end_is_dropped_and_empty_lines_are_rows() {
    assert_eq!(ok("a\tb\n"), (Format::Tsv, rows(&[&["a", "b"]])));
    assert_eq!(ok("a\tb\r\n"), (Format::Tsv, rows(&[&["a", "b"]])));
    assert_eq!(ok("a\tb\n\nc\td\n"), (Format::Tsv, rows(&[&["a", "b"], &["", ""], &["c", "d"]])));
    // An empty line at the start is a row of empty values.
    assert_eq!(csv("\na,b"), Ok(rows(&[&["", ""], &["a", "b"]])));
    assert_eq!(ok("\na\tb\n"), (Format::Tsv, rows(&[&["", ""], &["a", "b"]])));
    // An empty line at the end is a row of one empty value: kept by one value per line, refused
    // by wider rows.
    assert_eq!(read("a\tb\n\n"), Err(PasteError::Ragged { row: 2, values: 1, first_row: 1, first_values: 2 }));
    assert_eq!(read("a\tb\n\n\n"), Err(PasteError::Ragged { row: 2, values: 1, first_row: 1, first_values: 2 }));
    assert_eq!(ok("a\n"), (Format::One, rows(&[&["a"]])));
    assert_eq!(ok("a\n\n"), (Format::Column, rows(&[&["a"], &[""]])));
    // A spreadsheet's column (a, empty, empty) ends every line.
    assert_eq!(ok("a\r\n\r\n\r\n"), (Format::Column, rows(&[&["a"], &[""], &[""]])));
    assert_eq!(ok("\na\n"), (Format::Column, rows(&[&[""], &["a"]])));
    assert_eq!(ok("a\n\nb"), (Format::Column, rows(&[&["a"], &[""], &["b"]])));
    // A row of empty values is no empty line.
    assert_eq!(ok("a\tb\n\t\n"), (Format::Tsv, rows(&[&["a", "b"], &["", ""]])));
    assert_eq!(read(""), Err(PasteError::Empty));
    assert_eq!(read("\n\r\n"), Err(PasteError::Empty));
}

#[test]
fn rows_of_another_width_are_refused() {
    assert_eq!(
        read("a\tb\tc\nd\te\tf\ng\th\ti\tj"),
        Err(PasteError::Ragged { row: 3, values: 4, first_row: 1, first_values: 3 })
    );
    assert_eq!(csv("a,b\nc"), Err(PasteError::Ragged { row: 2, values: 1, first_row: 1, first_values: 2 }));
    assert_eq!(csv("\na,b\nc,d,e"), Err(PasteError::Ragged { row: 3, values: 3, first_row: 2, first_values: 2 }));
}

#[test]
fn a_paste_has_at_most_5000_rows_and_1_mib() {
    let lines = |n: usize| "x\ty\n".repeat(n);
    assert_eq!(ok(&lines(MAX_ROWS)).1.len(), MAX_ROWS);
    assert_eq!(read(&lines(MAX_ROWS + 1)), Err(PasteError::TooManyRows { rows: MAX_ROWS + 1, max: MAX_ROWS }));
    let big = "x".repeat(MAX_BYTES + 1);
    assert_eq!(read(&big), Err(PasteError::TooLarge { bytes: MAX_BYTES + 1, max: MAX_BYTES }));
    assert!(read(&"x".repeat(MAX_BYTES)).is_ok());
    // Reading stops at the first row past the limit, before the rest is looked at.
    assert_eq!(
        read(&format!("{}x\ry", lines(MAX_ROWS + 1))),
        Err(PasteError::TooManyRows { rows: MAX_ROWS + 1, max: MAX_ROWS })
    );
    assert_eq!(read(&"\n".repeat(MAX_ROWS + 1)), Err(PasteError::TooManyRows { rows: MAX_ROWS + 1, max: MAX_ROWS }));
}

#[test]
fn only_an_unquoted_null_token_is_null_and_an_empty_field_is_an_empty_text() {
    let (_, r) = ok("\\N\t\"\\N\"\t\t\"\"\tNULL\n");
    assert_eq!(r, [vec![Value::Null, text("\\N"), text(""), text(""), text("NULL")]]);
    assert_eq!(ok("\\N").1, [vec![Value::Null]]);
    assert_eq!(ok("\"\\N\"").1, [vec![text("\\N")]]);
    let opts = PasteOpts { null: "NULL".into() };
    assert_eq!(parse_as("\\N,NULL", Format::Csv, &opts).unwrap().rows, [vec![text("\\N"), Value::Null]]);
    // No token: an empty field stays an empty text.
    let opts = PasteOpts { null: String::new() };
    assert_eq!(parse_as("a,", Format::Csv, &opts).unwrap().rows, [vec![text("a"), text("")]]);
    assert!(ok("DEFAULT\t\\N").1.iter().flatten().all(|v| *v != Value::Default));
}

#[test]
fn the_text_reads_again_as_the_next_format() {
    let opts = PasteOpts::default();
    assert_eq!(
        [Format::Tsv, Format::Csv, Format::One, Format::Column].map(Format::next),
        [Format::Csv, Format::One, Format::Tsv, Format::One]
    );
    let s = "a,b\tc\n";
    assert_eq!(parse_as(s, Format::Tsv, &opts).unwrap().rows, rows(&[&["a,b", "c"]]));
    assert_eq!(parse_as(s, Format::Csv, &opts).unwrap().rows, rows(&[&["a", "b\tc"]]));
    assert_eq!(parse_as(s, Format::One, &opts).unwrap().rows, rows(&[&["a,b\tc"]]));
    assert_eq!(parse_as("\"a\"\"b\"\n", Format::One, &opts).unwrap().rows, rows(&[&["a\"b"]]));
    assert_eq!(parse_as("\"a\"b\"", Format::One, &opts).unwrap().rows, rows(&[&["\"a\"b\""]]));
    assert_eq!(parse_as("x\ny", Format::One, &opts).unwrap().format, Format::One);
}

/// A small generator of cell texts (no crate: a fixed sequence), with every character the
/// writers quote or that a paste reads specially.
struct Gen(u64);

impl Gen {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }

    fn cell(&mut self, tabs: bool) -> Option<String> {
        const PARTS: &[&str] = &[
            "a",
            "Z9",
            " ",
            "\t",
            ",",
            "\"",
            "\"\"",
            "\n",
            "\r",
            "\r\n",
            "x\"y",
            "-1.5",
            "\u{E9}",
            "\u{1F600}",
            "N",
            "\\",
        ];
        if self.next(8) == 0 {
            return None;
        }
        let len = self.next(5);
        let mut s = String::new();
        for _ in 0..len {
            let p = PARTS[self.next(PARTS.len() as u64) as usize];
            if tabs || p != "\t" {
                s.push_str(p);
            }
        }
        // Known loss: the copies write a text that is the NULL token as it is, so it reads back
        // as NULL.
        Some(if s == "\\N" { "x".into() } else { s })
    }
}

/// Rows the copy writers write, and what each cell must read back as.
///
/// Known losses of the copies as they are, left out here: NULL is written as an empty field, so
/// it reads back as an empty text (below); a text that is the NULL token reads back as NULL
/// ([`Gen::cell`]); in one column, the last cell written as an empty field (NULL, or an empty
/// text in TSV) is an empty last line, which the final line end drops.
fn corpus(g: &mut Gen, tabs: bool) -> (Vec<Vec<Option<String>>>, Vec<Vec<Value>>) {
    let width = 1 + g.next(5) as usize;
    let height = 1 + g.next(6) as usize;
    let mut cells: Vec<Vec<Option<String>>> = (0..height).map(|_| (0..width).map(|_| g.cell(tabs)).collect()).collect();
    if width == 1 {
        let last = &mut cells[height - 1][0];
        if last.as_deref().is_none_or(str::is_empty) {
            *last = Some("a".into());
        }
    }
    let want = cells.iter().map(|r| r.iter().map(|c| text(c.as_deref().unwrap_or(""))).collect()).collect();
    (cells, want)
}

#[test]
fn what_the_tsv_and_csv_copies_write_reads_back_as_the_same_cells() {
    let opts = PasteOpts::default();
    let mut g = Gen(7);
    for i in 0..2000 {
        let tabs = i % 2 == 0;
        let (cells, want) = corpus(&mut g, tabs);
        let columns: Vec<Column> = (0..cells[0].len()).map(|_| Column { name: "c", kind: Kind::Text }).collect();
        let row_refs: Vec<export::Row> = cells.iter().map(|r| r.iter().map(|c| c.as_deref()).collect()).collect();
        let tsv = export::tsv(&columns, &row_refs, false);
        let csv = export::csv(&columns, &row_refs, false);
        if columns.len() == 1 {
            // One column: one value per line, whatever its values look like; without tabs, as
            // it looks, commas in its values asked about, never split.
            assert_eq!(parse_as(&tsv, Format::Column, &opts).map(|p| p.rows), Ok(want.clone()), "{tsv:?}");
            assert_eq!(parse_as(&csv, Format::Column, &opts).map(|p| p.rows), Ok(want.clone()), "{csv:?}");
            for copy in [&tsv, &csv].into_iter().filter(|_| !tabs) {
                let got = match parse(copy, &opts) {
                    Err(PasteError::Ambiguous { plain: Ok(f), .. }) => parse_as(copy, f, &opts),
                    got => got,
                };
                assert_eq!(got.map(|p| p.rows), Ok(want.clone()), "{copy:?}");
            }
            continue;
        }
        assert_eq!(parse(&tsv, &opts), Ok(Pasted { format: Format::Tsv, rows: want.clone() }), "{tsv:?}");
        assert_eq!(parse_as(&csv, Format::Csv, &opts).map(|p| p.rows), Ok(want.clone()), "{csv:?}");
        if !tabs {
            // CSV is never guessed: asked about, as wide as it is.
            let width = cells[0].len();
            assert!(
                matches!(parse(&csv, &opts), Err(PasteError::Ambiguous { csv: Ok(w), .. }) if w == width),
                "{csv:?}"
            );
        }
    }
}

#[test]
fn commas_without_a_tab_are_asked_about_with_what_each_reading_gives() {
    // One line: CSV, or one value.
    assert_eq!(read("q@x,qq"), ambiguous(Ok(2), Ok(Format::One)));
    assert_eq!(read("1,5\n"), ambiguous(Ok(2), Ok(Format::One)));
    // A column copied whose values have commas: never split.
    let copy = export::tsv(&[Column { name: "c", kind: Kind::Text }], &[vec![Some("1,5")], vec![Some("x")]], false);
    let ragged = PasteError::Ragged { row: 2, values: 1, first_row: 1, first_values: 2 };
    assert_eq!(read(&copy), ambiguous(Err(ragged), Ok(Format::Column)));
    assert_eq!(parse_as(&copy, Format::Column, &PasteOpts::default()).unwrap().rows, rows(&[&["1,5"], &["x"]]));
    // Only one reading reads it: the other one's refusal is said.
    assert_eq!(read("\"a\",b\nc,d"), ambiguous(Ok(2), Err(PasteError::Quote { row: 1 })));
    assert_eq!(read("a,\"b"), ambiguous(Err(PasteError::Quote { row: 1 }), Ok(Format::One)));
    // Neither: refused as CSV refuses it.
    assert_eq!(read("a,b\n\"c"), Err(PasteError::Quote { row: 2 }));
    // No comma outside quotes: nothing to ask.
    assert_eq!(ok("\"a,b\""), (Format::One, rows(&[&["a,b"]])));
}
