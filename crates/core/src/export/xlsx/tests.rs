use super::read::{self, Cell as Data};
use super::*;

fn col(name: &str, kind: Kind) -> Column<'_> {
    Column { name, kind }
}

/// The workbook of `rows` (written in chunks of two) of `columns`, as `setup` makes the writer.
fn workbook<'a>(
    columns: &'a [Column<'a>],
    rows: &[Row],
    setup: impl FnOnce(XlsxWriter<'a, Vec<u8>>) -> XlsxWriter<'a, Vec<u8>>,
) -> (Vec<u8>, u32) {
    let mut w = setup(XlsxWriter::new(columns, Vec::new()).unwrap());
    w.begin().unwrap();
    for chunk in rows.chunks(2) {
        w.rows(chunk).unwrap();
    }
    w.end().unwrap();
    assert_eq!(w.rows_written(), rows.len() as u64);
    let sheets = w.sheets();
    (w.into_inner().unwrap(), sheets)
}

/// Every sheet of a workbook: its name and its cells, row by row.
fn read(bytes: &[u8]) -> Vec<(String, Vec<Vec<Data>>)> {
    read::sheets(bytes).expect("a workbook")
}

/// A part of the workbook (`xl/worksheets/sheet1.xml`) as text.
fn part(bytes: &[u8], name: &str) -> String {
    read::part(bytes, name).expect("the part")
}

fn s(v: &str) -> Data {
    Data::Text(v.to_string())
}

fn n(v: f64) -> Data {
    Data::Number(v)
}

#[test]
fn numbers_are_numbers_only_when_excel_holds_them_exactly() {
    for (v, want) in [
        ("42", Some(42.0)),
        ("-1.50", Some(-1.5)),
        ("0", Some(0.0)),
        ("-0.000", Some(0.0)),
        ("1.5E3", Some(1500.0)),
        ("2e-3", Some(0.002)),
        (".5", Some(0.5)),
        ("123456789012345", Some(123_456_789_012_345.0)),
        ("1234567890.12345", Some(1_234_567_890.123_45)),
        ("100000000000000000000", Some(1e20)),
        // More digits than Excel keeps: text, every digit kept.
        ("9007199254740993", None),
        ("1234567890123456", None),
        ("0.30000000000000004", None),
        ("12345678901234567890.5", None),
        // Not a number Excel has.
        ("NaN", None),
        ("Infinity", None),
        ("-Infinity", None),
        ("inf", None),
        ("1e400", None),
        ("1e-320", None),
        // Not plain decimal text.
        ("", None),
        ("-", None),
        (".", None),
        ("+1", None),
        ("1e", None),
        ("0x10", None),
        ("1 000", None),
        ("$1.00", None),
        ("1.2.3", None),
    ] {
        assert_eq!(excel_number(v), want, "{v}");
    }
}

#[test]
fn cells_have_their_types() {
    let columns = [
        col("id", Kind::Number),
        col("big", Kind::Number),
        col("price", Kind::Number),
        col("ok", Kind::Bool),
        col("name", Kind::Text),
        col("doc", Kind::Json),
        col("data", Kind::Bytes),
        col("at", Kind::Text),
        col("tags", Kind::Array(super::super::Element::Number)),
    ];
    let rows: Vec<Row> = vec![
        vec![
            Some("1"),
            Some("9223372036854775807"),
            Some("12.50"),
            Some("true"),
            Some("=1+1"),
            Some("{\"a\": 1}"),
            Some("\\x0102"),
            Some("2024-01-02 03:04:05+09"),
            Some("{1,2}"),
        ],
        vec![
            Some("-2"),
            Some("NaN"),
            Some("0.1"),
            Some("f"),
            Some("\u{d55c}\u{ae00} \u{e9}"),
            Some("[]"),
            Some(""),
            Some("2024-01-02"),
            Some("{}"),
        ],
        vec![Some("3"), None, None, None, None, None, None, None, None],
    ];
    let (bytes, sheets) = workbook(&columns, &rows, |w| w);
    assert_eq!(sheets, 1);
    let book = read(&bytes);
    assert_eq!(book.len(), 1);
    let (name, cells) = &book[0];
    assert_eq!(name, "Sheet1");
    let header: Vec<Data> = columns.iter().map(|c| s(c.name)).collect();
    assert_eq!(cells[0], header);
    assert_eq!(
        cells[1],
        vec![
            n(1.0),
            s("9223372036854775807"),
            n(12.5),
            Data::Bool(true),
            s("=1+1"),
            s("{\"a\": 1}"),
            s("\\x0102"),
            s("2024-01-02 03:04:05+09"),
            s("{1,2}"),
        ]
    );
    assert_eq!(
        cells[2],
        vec![
            n(-2.0),
            s("NaN"),
            n(0.1),
            Data::Bool(false),
            s("\u{d55c}\u{ae00} \u{e9}"),
            s("[]"),
            // The empty string: an empty cell too (Excel tells them apart no more than a
            // reader does); `null_text` tells NULL apart.
            Data::Empty,
            s("2024-01-02"),
            s("{}"),
        ]
    );
    let mut nulls = vec![n(3.0)];
    nulls.resize(columns.len(), Data::Empty);
    assert_eq!(cells[3], nulls);
    // `=1+1` is text, not a formula.
    assert!(!part(&bytes, "xl/worksheets/sheet1.xml").contains("<f>"));
}

#[test]
fn a_boolean_that_is_not_one_is_text() {
    let columns = [col("ok", Kind::Bool)];
    let (bytes, _) = workbook(&columns, &[vec![Some("maybe")]], |w| w);
    assert_eq!(read(&bytes)[0].1[1], vec![s("maybe")]);
}

#[test]
fn null_is_an_empty_cell_or_the_null_text() {
    let columns = [col("a", Kind::Number), col("b", Kind::Text)];
    let rows: Vec<Row> = vec![vec![None, Some("")], vec![Some("1"), None]];
    let (bytes, _) = workbook(&columns, &rows, |w| w);
    let cells = &read(&bytes)[0].1;
    assert_eq!(cells[1], vec![Data::Empty, Data::Empty]);
    assert_eq!(cells[2], vec![n(1.0), Data::Empty]);

    let (bytes, _) = workbook(&columns, &rows, |w| w.null_text("NULL"));
    let cells = &read(&bytes)[0].1;
    assert_eq!(cells[1], vec![s("NULL"), Data::Empty]);
    assert_eq!(cells[2], vec![n(1.0), s("NULL")]);
}

#[test]
fn the_header_is_bold_and_frozen() {
    let columns = [col("a", Kind::Text), col("b", Kind::Text)];
    let (bytes, _) = workbook(&columns, &[vec![Some("x"), Some("y")]], |w| w);
    let sheet = part(&bytes, "xl/worksheets/sheet1.xml");
    assert!(sheet.contains(r#"<pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/>"#), "{sheet}");
    // The header's cells have the bold style (1), the rows the default one.
    assert!(sheet.contains(r#"<c r="A1" s="1" t="inlineStr">"#), "{sheet}");
    assert!(sheet.contains(r#"<c r="A2" t="inlineStr">"#), "{sheet}");
    let styles = part(&bytes, "xl/styles.xml");
    assert!(styles.contains("<b/>"), "{styles}");
}

#[test]
fn rows_past_a_sheet_go_on_in_the_next_one_with_the_header() {
    let columns = [col("n", Kind::Number), col("t", Kind::Text)];
    let values: Vec<String> = (1..=5).map(|i| i.to_string()).collect();
    let rows: Vec<Row> = values.iter().map(|v| vec![Some(v.as_str()), Some("x")]).collect();
    // Three rows a sheet: the header and two rows.
    let (bytes, sheets) = workbook(&columns, &rows, |w| w.sheet_rows(3));
    assert_eq!(sheets, 3);
    let book = read(&bytes);
    let names: Vec<&str> = book.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["Sheet1", "Sheet2", "Sheet3"]);
    let header = vec![s("n"), s("t")];
    let firsts: Vec<Vec<f64>> = book
        .iter()
        .map(|(_, cells)| {
            assert_eq!(cells[0], header);
            cells[1..]
                .iter()
                .map(|r| match r[0] {
                    Data::Number(f) => f,
                    ref d => panic!("{d:?}"),
                })
                .collect()
        })
        .collect();
    assert_eq!(firsts, vec![vec![1.0, 2.0], vec![3.0, 4.0], vec![5.0]]);
    for i in 1..=3 {
        let sheet = part(&bytes, &format!("xl/worksheets/sheet{i}.xml"));
        assert!(sheet.contains(r#"state="frozen""#), "sheet {i}");
    }

    // Rows that fill the last sheet exactly make no empty sheet after it.
    let (bytes, sheets) = workbook(&columns, &rows[..4], |w| w.sheet_rows(3));
    assert_eq!(sheets, 2);
    assert_eq!(read(&bytes).len(), 2);
}

#[test]
fn a_sheet_holds_excels_rows_at_most() {
    let columns = [col("n", Kind::Number)];
    let w = XlsxWriter::new(&columns, Vec::new()).unwrap();
    assert_eq!(w.max_rows, MAX_ROWS);
    assert_eq!(w.sheet_rows(u32::MAX).max_rows, MAX_ROWS);
    let w = XlsxWriter::new(&columns, Vec::new()).unwrap();
    assert_eq!(w.sheet_rows(0).max_rows, 2);
}

#[test]
fn more_columns_than_a_sheet_holds_are_refused() {
    let names: Vec<String> = (0..=MAX_COLUMNS).map(|i| format!("c{i}")).collect();
    let columns: Vec<Column> = names.iter().map(|n| col(n, Kind::Text)).collect();
    assert_eq!(XlsxWriter::new(&columns, Vec::new()).err(), Some(TooWide { columns: MAX_COLUMNS + 1 }));
    let columns = &columns[..MAX_COLUMNS];
    let row: Row = vec![Some("x"); MAX_COLUMNS];
    let (bytes, _) = workbook(columns, &[row], |w| w);
    let cells = &read(&bytes)[0].1;
    assert_eq!(cells[1].len(), MAX_COLUMNS);
    assert_eq!(cells[0][MAX_COLUMNS - 1], s("c16383"));
}

#[test]
fn a_value_too_long_for_a_cell_fails_with_its_column_and_keeps_the_rows_before() {
    let columns = [col("id", Kind::Number), col("body", Kind::Text)];
    let longest = "a".repeat(MAX_CELL_CHARS);
    // More bytes than a cell's characters, but not more characters.
    let wide = "\u{d55c}".repeat(MAX_CELL_CHARS);
    let long = "a".repeat(MAX_CELL_CHARS + 1);
    let mut w = XlsxWriter::new(&columns, Vec::new()).unwrap();
    w.begin().unwrap();
    w.rows(&[vec![Some("1"), Some(&longest)], vec![Some("2"), Some(&wide)]]).unwrap();
    let e =
        w.rows(&[vec![Some("3"), Some("ok")], vec![Some("4"), Some(&long)], vec![Some("5"), Some("x")]]).unwrap_err();
    assert_eq!(TooLong::of(&e), Some(&TooLong { column: "body".to_string(), row: 4, chars: MAX_CELL_CHARS + 1 }));
    assert_eq!(e.to_string(), "a value of column body (row 4) has 32768 characters; a cell holds 32767");
    assert_eq!(w.rows_written(), 3);
    // The workbook of the rows before it.
    w.end().unwrap();
    let book = read(&w.into_inner().unwrap());
    let cells = &book[0].1;
    assert_eq!(cells.len(), 4);
    assert_eq!(cells[1], vec![n(1.0), s(&longest)]);
    assert_eq!(cells[2], vec![n(2.0), s(&wide)]);
    assert_eq!(cells[3], vec![n(3.0), s("ok")]);
    // Another error is not one.
    assert_eq!(TooLong::of(&io::Error::other("disk full")), None);
}

#[test]
fn the_null_text_too_long_for_a_cell_fails() {
    let columns = [col("a", Kind::Text)];
    let null = "n".repeat(MAX_CELL_CHARS + 1);
    let mut w = XlsxWriter::new(&columns, Vec::new()).unwrap().null_text(&null);
    w.begin().unwrap();
    let e = w.rows(&[vec![None]]).unwrap_err();
    assert_eq!(TooLong::of(&e).map(|t| t.row), Some(1));
}

#[test]
fn ending_early_makes_a_workbook_of_the_rows_so_far() {
    // What a cancel does: the rows written so far, then the end.
    let columns = [col("n", Kind::Number)];
    let mut w = XlsxWriter::new(&columns, Vec::new()).unwrap();
    w.begin().unwrap();
    w.rows(&[vec![Some("1")], vec![Some("2")]]).unwrap();
    w.end().unwrap();
    // Ending twice does nothing more.
    w.end().unwrap();
    let book = read(&w.into_inner().unwrap());
    assert_eq!(book[0].1, vec![vec![s("n")], vec![n(1.0)], vec![n(2.0)]]);
}

#[test]
fn a_workbook_without_rows_has_its_header() {
    let columns = [col("a", Kind::Text)];
    let (bytes, sheets) = workbook(&columns, &[], |w| w);
    assert_eq!(sheets, 1);
    assert_eq!(read(&bytes)[0].1, vec![vec![s("a")]]);
}

#[test]
fn the_workbook_appears_only_at_the_end() {
    let columns = [col("a", Kind::Text)];
    let mut w = XlsxWriter::new(&columns, Vec::new()).unwrap();
    w.begin().unwrap();
    w.rows(&[vec![Some("abc")], vec![Some("\u{d55c}")]]).unwrap();
    assert_eq!(w.bytes(), 6);
    let mut w = w;
    w.end().unwrap();
    assert!(!w.into_inner().unwrap().is_empty());
}

#[test]
fn a_broken_writer_writes_nothing_more_and_makes_no_workbook() {
    let columns = [col("a", Kind::Text)];
    let mut w = XlsxWriter::new(&columns, Vec::new()).unwrap();
    w.begin().unwrap();
    w.rows(&[vec![Some("x")]]).unwrap();
    // What a panic of the library (its temporary file failed) leaves.
    w.broken = true;
    assert!(w.rows(&[vec![Some("y")]]).is_err());
    assert_eq!(w.rows_written(), 1);
    assert!(w.end().is_err());
    assert_eq!(w.into_inner().unwrap(), Vec::<u8>::new());
}

#[test]
fn a_panic_of_the_library_breaks_the_writer() {
    let columns = [col("a", Kind::Text)];
    let mut w = XlsxWriter::new(&columns, Vec::new()).unwrap();
    let e = w.guarded(|_| -> Result<(), XlsxError> { panic!("temporary file") }).unwrap_err();
    assert!(e.to_string().contains("temporary file failed"), "{e}");
    assert!(w.broken);
}

#[test]
fn a_cell_holds_32767_characters_as_excel_counts_them() {
    // Excel counts UTF-16 units: a character outside the Basic Multilingual Plane counts two.
    let columns = [col("e", Kind::Text)];
    let fits = format!("{}a", "\u{1F600}".repeat(MAX_CELL_CHARS / 2));
    let over = "\u{1F600}".repeat(MAX_CELL_CHARS / 2 + 1);
    assert_eq!(fits.encode_utf16().count(), MAX_CELL_CHARS);
    assert!(over.chars().count() < MAX_CELL_CHARS, "fewer characters than a cell holds");
    let mut w = XlsxWriter::new(&columns, Vec::new()).unwrap();
    w.begin().unwrap();
    w.rows(&[vec![Some(&fits)]]).unwrap();
    let e = w.rows(&[vec![Some(&over)]]).unwrap_err();
    assert_eq!(TooLong::of(&e), Some(&TooLong { column: "e".to_string(), row: 2, chars: MAX_CELL_CHARS + 1 }));
    w.end().unwrap();
    assert_eq!(read(&w.into_inner().unwrap())[0].1[1], vec![s(&fits)]);
}

#[test]
fn text_excel_would_read_otherwise_comes_back_as_it_was() {
    // Control characters, a non-character, Excel's own escape written literally, and spaces at
    // the ends (kept with xml:space).
    let columns = [col("t", Kind::Text)];
    let values = [
        "a\u{1}b\u{1F}c",
        "\u{FFFF}",
        "\u{FFFE}x",
        "_x0041_",
        "_x005F_x0041_",
        "  leading",
        "trailing  ",
        " both ",
        "tab\there",
        "line\nbreak\r\nend",
        "<&>\"'",
    ];
    let rows: Vec<Row> = values.iter().map(|v| vec![Some(*v)]).collect();
    let (bytes, _) = workbook(&columns, &rows, |w| w);
    let cells = &read(&bytes)[0].1;
    for (i, v) in values.iter().enumerate() {
        assert_eq!(cells[i + 1], vec![s(v)], "{v:?}");
    }
}

/// The local file headers of a ZIP archive: each part's name and the ids of its extra fields.
/// Only for the writer's own archives, small ones whose compressed data holds no header by chance.
fn local_headers(bytes: &[u8]) -> Vec<(String, Vec<u16>)> {
    let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let mut found = Vec::new();
    let mut i = 0;
    while let Some(at) = bytes[i..].windows(4).position(|w| w == b"PK\x03\x04") {
        let h = i + at;
        let (name_len, extra_len) = (u16_at(h + 26) as usize, u16_at(h + 28) as usize);
        let name = String::from_utf8_lossy(&bytes[h + 30..h + 30 + name_len]).into_owned();
        let extra = &bytes[h + 30 + name_len..h + 30 + name_len + extra_len];
        let mut ids = Vec::new();
        let mut e = 0;
        while e + 4 <= extra.len() {
            ids.push(u16::from_le_bytes([extra[e], extra[e + 1]]));
            e += 4 + u16::from_le_bytes([extra[e + 2], extra[e + 3]]) as usize;
        }
        found.push((name, ids));
        i = h + 4;
    }
    found
}

#[test]
fn the_workbook_is_a_zip64_archive() {
    // Without ZIP64 a part of more than 4 GiB fails the end (`a_sheet_over_4_gib_is_made`).
    let columns = [col("a", Kind::Text)];
    let (bytes, _) = workbook(&columns, &[vec![Some("x")]], |w| w);
    let parts = zip::ZipArchive::new(io::Cursor::new(&bytes)).unwrap().len();
    let headers = local_headers(&bytes);
    assert_eq!(headers.len(), parts, "{headers:?}");
    for (name, ids) in &headers {
        assert_eq!(ids.first(), Some(&0x0001), "the ZIP64 extra field of {name}");
    }
    assert!(headers.iter().any(|(n, _)| n == "xl/worksheets/sheet1.xml"));
}

#[test]
fn a_stop_while_the_workbook_is_made_drops_it() {
    let columns = [col("n", Kind::Number)];
    let mut w = XlsxWriter::new(&columns, Vec::new()).unwrap();
    w.begin().unwrap();
    w.rows(&[vec![Some("1")], vec![Some("2")]]).unwrap();
    let mut asked = 0;
    let mut stop = || {
        asked += 1;
        asked > 3
    };
    let e = w.end_unless(&mut stop).unwrap_err();
    assert!(Dropped::of(&e), "{e}");
    assert_eq!(asked, 4, "nothing written after the stop");
    let out = w.into_inner().unwrap();
    assert!(!out.is_empty() && read::sheets(&out).is_none(), "part of a workbook, not one");
    assert!(!Dropped::of(&io::Error::other("disk full")));
    // Never told to stop: the workbook is made.
    let mut w = XlsxWriter::new(&columns, Vec::new()).unwrap();
    w.begin().unwrap();
    w.rows(&[vec![Some("1")]]).unwrap();
    w.end_unless(&mut || false).unwrap();
    assert_eq!(read(&w.into_inner().unwrap())[0].1, vec![vec![s("n")], vec![n(1.0)]]);
}

/// An output that fails once `room` bytes went in (a disk that filled up).
struct Full {
    room: usize,
    written: Vec<u8>,
}

impl Write for Full {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.written.len() + buf.len() > self.room {
            return Err(io::Error::from(io::ErrorKind::StorageFull));
        }
        self.written.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn an_output_that_fails_while_the_workbook_is_made_fails_the_end() {
    let columns = [col("n", Kind::Number)];
    let mut w = XlsxWriter::new(&columns, Full { room: 600, written: Vec::new() }).unwrap();
    w.begin().unwrap();
    w.rows(&[vec![Some("1")]]).unwrap();
    let e = w.end().unwrap_err();
    assert!(!Dropped::of(&e), "{e}");
    // The library hands the zip writer's error on as its text.
    assert!(e.to_string().contains("no storage space"), "{e}");
    let out = w.into_inner().unwrap();
    assert!(out.written.len() <= 600 && read::sheets(&out.written).is_none());
}

/// A sheet of more than 4 GiB of XML (1,048,575 rows of 4,200 characters) is made, and reads as
/// a ZIP64 archive: without ZIP64, its end fails. Slow (minutes, about 5 GiB in the temporary
/// folder): `cargo test --release -p datarig-core -- --ignored over_4_gib`.
#[test]
#[ignore]
fn a_sheet_over_4_gib_is_made() {
    let columns: Vec<Column> = ["a", "b", "c", "d", "e", "f"].iter().map(|n| col(n, Kind::Text)).collect();
    let value = "0123456789".repeat(70);
    let row: Row = vec![Some(value.as_str()); columns.len()];
    let chunk: Vec<Row> = vec![row; 1_000];
    let total = (MAX_ROWS - 1) as usize;
    let path = std::env::temp_dir().join(format!("datarig-xlsx-4gib-{}.xlsx", std::process::id()));
    let file = std::fs::File::create(&path).unwrap();
    let mut w = XlsxWriter::new(&columns, io::BufWriter::new(file)).unwrap();
    w.begin().unwrap();
    let mut written = 0;
    while written < total {
        let n = (total - written).min(chunk.len());
        w.rows(&chunk[..n]).unwrap();
        written += n;
    }
    w.end().unwrap();
    assert_eq!(w.sheets(), 1);
    drop(w);
    let mut zip = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
    let sheet = zip.by_name("xl/worksheets/sheet1.xml").unwrap();
    let size = sheet.size();
    drop(sheet);
    drop(zip);
    let _ = std::fs::remove_file(&path);
    assert!(size > u64::from(u32::MAX), "{size} bytes of XML");
}

/// Excel's own number of rows, for real: the row after the 1,048,575th goes to `Sheet2`. Slow
/// (a million rows): `cargo test -p datarig-core -- --ignored excels_own`.
#[test]
#[ignore]
fn excels_own_row_limit_goes_on_in_a_second_sheet() {
    let columns = [col("n", Kind::Number)];
    let total = MAX_ROWS as usize; // One more than a sheet holds with its header.
    let values: Vec<String> = (1..=total).map(|i| i.to_string()).collect();
    let mut w = XlsxWriter::new(&columns, Vec::new()).unwrap();
    w.begin().unwrap();
    for chunk in values.chunks(2_000) {
        let rows: Vec<Row> = chunk.iter().map(|v| vec![Some(v.as_str())]).collect();
        w.rows(&rows).unwrap();
    }
    w.end().unwrap();
    assert_eq!(w.sheets(), 2);
    let book = read(&w.into_inner().unwrap());
    assert_eq!(book.len(), 2);
    assert_eq!(book[0].1.len(), MAX_ROWS as usize);
    assert_eq!(book[0].1.last().unwrap()[0], n(f64::from(MAX_ROWS - 1)));
    assert_eq!(book[1].1, vec![vec![s("n")], vec![n(f64::from(MAX_ROWS))]]);
}
