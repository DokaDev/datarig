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

#[test]
fn the_format_is_tsv_then_csv_then_one_value_per_line_then_one_value() {
    assert_eq!(ok("a\tb\nc\td"), (Format::Tsv, rows(&[&["a", "b"], &["c", "d"]])));
    assert_eq!(ok("a,b\nc,d"), (Format::Csv, rows(&[&["a", "b"], &["c", "d"]])));
    assert_eq!(ok("a\nb\n"), (Format::Column, rows(&[&["a"], &["b"]])));
    assert_eq!(ok("a b"), (Format::One, rows(&[&["a b"]])));
    // A tab anywhere wins over commas; a comma only on a later line is not CSV.
    assert_eq!(read("a,b\nc\td"), Err(PasteError::Ragged { row: 2, values: 2, first_row: 1, first_values: 1 }));
    assert_eq!(ok("a\nb,c").0, Format::Column);
    // A comma inside a TSV cell is the cell's.
    assert_eq!(ok("1,5\tx"), (Format::Tsv, rows(&[&["1,5", "x"]])));
    // A tab inside a CSV quote is the value's, and not a TSV.
    assert_eq!(ok("\"a\tb\",c"), (Format::Csv, rows(&[&["a\tb", "c"]])));
    // A comma inside a quote on the first line is not CSV.
    assert_eq!(ok("\"a,b\"\n\"c,d\""), (Format::Column, rows(&[&["a,b"], &["c,d"]])));
    // A line break inside a quote is not a second line.
    assert_eq!(ok("\"a\nb\""), (Format::One, rows(&[&["a\nb"]])));
}

#[test]
fn quotes_are_read_as_rfc_4180_in_tsv_and_csv() {
    assert_eq!(ok("\"a\"\"b\"\tc"), (Format::Tsv, rows(&[&["a\"b", "c"]])));
    assert_eq!(ok("\"x\ny\",z\nu,v"), (Format::Csv, rows(&[&["x\ny", "z"], &["u", "v"]])));
    assert_eq!(ok("\"\"\t\"\t\""), (Format::Tsv, rows(&[&["", "\t"]])));
    // A quote inside an unquoted field is a character of it.
    assert_eq!(ok("say \"hi\"\tb"), (Format::Tsv, rows(&[&["say \"hi\"", "b"]])));
    assert_eq!(ok("5\"\t6'"), (Format::Tsv, rows(&[&["5\"", "6'"]])));
    // Open, or followed by text: refused.
    assert_eq!(read("a\t\"b\nc\td"), Err(PasteError::Quote { row: 1 }));
    assert_eq!(read("a\tb\n\"x\"y\tc"), Err(PasteError::Quote { row: 2 }));
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
fn trailing_empty_lines_are_dropped_and_inner_ones_are_rows_of_empty_values() {
    assert_eq!(ok("a\tb\n\n\n"), (Format::Tsv, rows(&[&["a", "b"]])));
    assert_eq!(ok("a\tb\n\nc\td\n"), (Format::Tsv, rows(&[&["a", "b"], &["", ""], &["c", "d"]])));
    assert_eq!(ok("\na,b"), (Format::Csv, rows(&[&["", ""], &["a", "b"]])));
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
    assert_eq!(read("a,b\nc"), Err(PasteError::Ragged { row: 2, values: 1, first_row: 1, first_values: 2 }));
    assert_eq!(read("\na,b\nc,d,e"), Err(PasteError::Ragged { row: 3, values: 3, first_row: 2, first_values: 2 }));
}

#[test]
fn a_paste_has_at_most_5000_rows_and_1_mib() {
    let lines = |n: usize| "x\ty\n".repeat(n);
    assert_eq!(ok(&lines(MAX_ROWS)).1.len(), MAX_ROWS);
    assert_eq!(read(&lines(MAX_ROWS + 1)), Err(PasteError::TooManyRows { rows: MAX_ROWS + 1, max: MAX_ROWS }));
    let big = "x".repeat(MAX_BYTES + 1);
    assert_eq!(read(&big), Err(PasteError::TooLarge { bytes: MAX_BYTES + 1, max: MAX_BYTES }));
    assert!(read(&"x".repeat(MAX_BYTES)).is_ok());
}

#[test]
fn only_an_unquoted_null_token_is_null_and_an_empty_field_is_an_empty_text() {
    let (_, r) = ok("\\N\t\"\\N\"\t\t\"\"\tNULL\n");
    assert_eq!(r, [vec![Value::Null, text("\\N"), text(""), text(""), text("NULL")]]);
    assert_eq!(ok("\\N").1, [vec![Value::Null]]);
    assert_eq!(ok("\"\\N\"").1, [vec![text("\\N")]]);
    let opts = PasteOpts { null: "NULL".into() };
    assert_eq!(parse("\\N,NULL", &opts).unwrap().rows, [vec![text("\\N"), Value::Null]]);
    // No token: an empty field stays an empty text.
    let opts = PasteOpts { null: String::new() };
    assert_eq!(parse("a,", &opts).unwrap().rows, [vec![text("a"), text("")]]);
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
        // The NULL token as a whole value is read as NULL: a case of its own.
        Some(if s == "\\N" { "x".into() } else { s })
    }
}

/// Rows the copy writers write, and what each cell must read back as (NULL is written as an
/// empty field, read back as an empty text).
fn corpus(g: &mut Gen, tabs: bool) -> (Vec<Vec<Option<String>>>, Vec<Vec<Value>>) {
    let width = 2 + g.next(4) as usize;
    let height = 1 + g.next(6) as usize;
    let cells: Vec<Vec<Option<String>>> = (0..height).map(|_| (0..width).map(|_| g.cell(tabs)).collect()).collect();
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
        assert_eq!(parse(&tsv, &opts), Ok(Pasted { format: Format::Tsv, rows: want.clone() }), "{tsv:?}");
        let csv = export::csv(&columns, &row_refs, false);
        assert_eq!(parse_as(&csv, Format::Csv, &opts).map(|p| p.rows), Ok(want.clone()), "{csv:?}");
        if !tabs {
            assert_eq!(parse(&csv, &opts), Ok(Pasted { format: Format::Csv, rows: want }), "{csv:?}");
        }
    }
}
