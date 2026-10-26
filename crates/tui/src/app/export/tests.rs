use super::*;

#[test]
fn the_words_of_export_are_a_format_a_scope_and_a_path() {
    let args = |s: &str| parse_args(s).unwrap();
    assert_eq!(args(""), ExportArgs::default());
    assert_eq!(args("csv"), ExportArgs { format: Some(ExportFormat::Csv), ..Default::default() });
    assert_eq!(
        args("json fetched ~/out file.json"),
        ExportArgs {
            format: Some(ExportFormat::Json),
            scope: Some(ExportScope::Fetched),
            path: Some("~/out file.json".into()),
            ..Default::default()
        }
    );
    assert_eq!(
        args("selection /srv/x.md"),
        ExportArgs { scope: Some(ExportScope::Selection), path: Some("/srv/x.md".into()), ..Default::default() }
    );
    assert_eq!(args("out.csv"), ExportArgs { path: Some("out.csv".into()), ..Default::default() });
    assert_eq!(args("ndjson all a.txt").format, Some(ExportFormat::JsonLines));
    assert_eq!(args("SQL").format, Some(ExportFormat::SqlInsert));
    assert_eq!(parse_args("avro"), Err(ArgsError::UnknownFormat("avro".into())));
    assert_eq!(args("parquet").format, Some(ExportFormat::Parquet));
}

#[test]
fn a_path_names_its_format() {
    let of = |p: &str| ExportFormat::of_path(Path::new(p));
    assert_eq!(of("a.CSV"), Some(ExportFormat::Csv));
    assert_eq!(of("a.tsv"), Some(ExportFormat::Tsv));
    assert_eq!(of("a.json"), Some(ExportFormat::Json));
    assert_eq!(of("a.ndjson"), Some(ExportFormat::JsonLines));
    assert_eq!(of("a.md"), Some(ExportFormat::Markdown));
    assert_eq!(of("a.htm"), Some(ExportFormat::Html));
    assert_eq!(of("a.xml"), Some(ExportFormat::Xml));
    assert_eq!(of("a.sql"), Some(ExportFormat::SqlInsert));
    assert_eq!(of("a.XLSX"), Some(ExportFormat::Xlsx));
    assert_eq!(ExportFormat::parse("excel"), Some(ExportFormat::Xlsx));
    assert_eq!(ExportFormat::Xlsx.extension(), "xlsx");
    assert_eq!(of("a.parquet"), Some(ExportFormat::Parquet));
    assert_eq!(ExportFormat::Parquet.extension(), "parquet");
    assert_eq!(of("a.txt"), None);
    assert_eq!(of("noext"), None);
    for f in ExportFormat::ALL {
        assert_eq!(ExportFormat::parse(f.name()), Some(f));
    }
}

#[test]
fn paths_expand_home_and_put_relative_ones_in_the_export_folder() {
    let home = Path::new("/h");
    let folder = Path::new("/h/Downloads");
    assert_eq!(expand_path("~", Some(home), folder), Some(PathBuf::from("/h")));
    assert_eq!(expand_path("~/a/b.csv", Some(home), folder), Some(PathBuf::from("/h/a/b.csv")));
    assert_eq!(expand_path("~/x", None, folder), None, "no home to expand into");
    assert_eq!(expand_path("/abs/x.csv", Some(home), folder), Some(PathBuf::from("/abs/x.csv")));
    assert_eq!(expand_path("x.csv", Some(home), folder), Some(PathBuf::from("/h/Downloads/x.csv")));
    assert_eq!(expand_path("  ", Some(home), folder), None);
}

#[test]
fn names_become_safe_file_names() {
    assert_eq!(file_stem("public.users"), "public.users");
    assert_eq!(file_stem("a/b\\c:d*e?f\"g<h>i|j"), "a_b_c_d_e_f_g_h_i_j");
    assert_eq!(file_stem("tab\tname\n"), "tab_name_");
    assert_eq!(file_stem("  ..  "), "export");
    assert_eq!(file_stem("\u{d14c}\u{c774}\u{be14}"), "\u{d14c}\u{c774}\u{be14}", "Hangul stays");
    assert_eq!(file_stem(&"x".repeat(200)).len(), 80);
}

#[test]
fn the_partial_file_is_next_to_the_file() {
    assert_eq!(partial_path(Path::new("/d/out.csv")), PathBuf::from("/d/out.csv.partial"));
}

#[test]
fn csv_options_are_words_before_the_path() {
    let a = parse_args("csv fetched delim=; quote=never header=off null=NULL ~/out.csv").unwrap();
    assert_eq!(a.csv, Delimited { delimiter: ';', quote: Quote::Never, header: false, null: "NULL".into() });
    assert_eq!(a.path.as_deref(), Some("~/out.csv"));
    assert_eq!(parse_args("csv delim=tab x.csv").unwrap().csv.delimiter, '\t');
    assert_eq!(parse_args("csv delim=\\t x.csv").unwrap().csv.delimiter, '\t');
    assert_eq!(parse_args("csv quote=maybe x.csv"), Err(ArgsError::BadOption("quote=maybe".into())));
    assert_eq!(parse_args("csv a=b.csv").unwrap().path.as_deref(), Some("a=b.csv"), "not an option: a path");
}

#[test]
fn a_writer_that_stops_without_its_word_says_where_its_partial_file_is() {
    let dir = std::env::temp_dir().join(format!("datarig-export-guard-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let partial = dir.join("x.csv.partial");
    std::fs::write(&partial, "id\n1\n").unwrap();
    let (tx, rx) = mpsc::channel();
    let guard = EndGuard { out: Out::Local(tx), job: 1, tab: TabId(1), sent: false, partial: Default::default() };
    *guard.partial.borrow_mut() = Some(partial.clone());
    drop(guard);
    match rx.recv().unwrap() {
        AppEvent::Export(ExportEvent { news: ExportNews::Ended(Outcome::Failed { partial: p, .. }), .. }) => {
            assert_eq!(p, Some(partial.clone()));
        }
        other => panic!("{other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// An output that fails once `room` bytes went in (a disk that filled up).
struct Full {
    room: usize,
    written: usize,
}

impl std::io::Write for Full {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.written + buf.len() > self.room {
            return Err(std::io::Error::from(std::io::ErrorKind::StorageFull));
        }
        self.written += buf.len();
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A workbook writer of one row into `out`, and the app's end of its control channel.
fn workbook_of_a_row<'a, W: std::io::Write + Send>(
    columns: &'a [core_export::Column<'a>],
    out: W,
) -> (XlsxWriter<'a, BufWriter<W>>, CtlSender, tokio::sync::mpsc::UnboundedReceiver<Ctl>) {
    let mut w = XlsxWriter::new(columns, BufWriter::new(out)).unwrap();
    w.begin().unwrap();
    w.rows(&[vec![Some("1")]]).unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (w, tx, rx)
}

#[test]
fn a_workbook_that_cannot_be_made_keeps_nothing_and_says_what_failed_first() {
    let columns = [core_export::Column { name: "n", kind: Kind::Number }];
    let partial = Path::new("/d/x.xlsx.partial");
    let (w, _tx, mut rx) = workbook_of_a_row(&columns, Full { room: 600, written: 0 });
    let made = make_workbook(w, Stopped::Done, &mut rx, partial, |_| Ok(0)).map_err(|o| *o);
    match made {
        Err(Outcome::Failed { why: Failure::NotSaved { first: None, .. }, rows: 1, partial: None }) => {}
        other => panic!("{other:?}"),
    }
    // The rows had stopped for a value too long: that is said too.
    let (w, _tx, mut rx) = workbook_of_a_row(&columns, Full { room: 600, written: 0 });
    let long = xlsx::TooLong { column: "n".into(), row: 2, chars: 40_000 };
    let made =
        make_workbook(w, Stopped::Failed(Failure::TooLong(long.clone())), &mut rx, partial, |_| Ok(0)).map_err(|o| *o);
    match made {
        Err(Outcome::Failed { why: Failure::NotSaved { first: Some(first), .. }, partial: None, .. }) => {
            assert!(matches!(*first, Failure::TooLong(ref t) if *t == long), "{first:?}");
        }
        other => panic!("{other:?}"),
    }
    // Finishing it (its sync) fails: nothing is kept either.
    let (w, _tx, mut rx) = workbook_of_a_row(&columns, Vec::new());
    let made =
        make_workbook(w, Stopped::Cancelled, &mut rx, partial, |_| Err(std::io::Error::other("sync"))).map_err(|o| *o);
    assert!(matches!(made, Err(Outcome::Failed { why: Failure::NotSaved { first: None, .. }, .. })), "{made:?}");
}

#[test]
fn a_drop_while_the_workbook_is_made_drops_it_and_a_stale_cancel_does_not() {
    let columns = [core_export::Column { name: "n", kind: Kind::Number }];
    let partial = Path::new("/d/x.xlsx.partial");
    let make = |words: Vec<Ctl>, stopped: Stopped| {
        let (w, tx, mut rx) = workbook_of_a_row(&columns, Vec::new());
        for c in words {
            tx.send(c).unwrap();
        }
        make_workbook(w, stopped, &mut rx, partial, |out| {
            assert!(xlsx::read::sheets(&out).is_some(), "a workbook");
            Ok(out.len() as u64)
        })
        .map_err(|o| *o)
    };
    // A cancel sent before the app said a cancel now keeps nothing (a second Ctrl+C, one that
    // came while the last rows were written): the workbook is made, and stays the cancel's.
    let made = make(vec![Ctl::Cancel], Stopped::Cancelled);
    assert!(matches!(made, Ok((size, Stopped::Cancelled)) if size > 0), "{made:?}");
    // The drop: nothing kept.
    let made = make(vec![Ctl::Drop], Stopped::Cancelled);
    assert!(matches!(made, Err(Outcome::Dropped { rows: 1, first: None })), "{made:?}");
    // What a quit sends: the cancel the rows did not take, then the drop.
    let made = make(vec![Ctl::Cancel, Ctl::Drop], Stopped::Done);
    assert!(matches!(made, Err(Outcome::Dropped { rows: 1, first: None })), "{made:?}");
    // After a failure: it is said with it.
    let made = make(vec![Ctl::Drop], Stopped::Failed(Failure::Key));
    assert!(matches!(made, Err(Outcome::Dropped { rows: 1, first: Some(ref f) }) if matches!(**f, Failure::Key)));
    // Any other word is not a drop: the workbook is made, and how the rows stopped stays.
    let made = make(vec![Ctl::Finish, Ctl::Fail(Failure::Stale)], Stopped::Cancelled);
    assert!(matches!(made, Ok((size, Stopped::Cancelled)) if size > 0), "{made:?}");
}

#[test]
fn a_rows_failure_of_a_workbook_is_its_temporary_file_or_a_value_too_long() {
    let long = xlsx::TooLong { column: "c".into(), row: 3, chars: 40_000 };
    assert!(matches!(workbook_failure(std::io::Error::other(long.clone())), Failure::TooLong(t) if t == long));
    match workbook_failure(std::io::Error::from(std::io::ErrorKind::StorageFull)) {
        Failure::TempFile(_, folder) => assert_eq!(folder, std::env::temp_dir()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_parquet_file_whose_disk_filled_up_keeps_nothing_and_says_the_disk_is_full() {
    let dir = std::env::temp_dir().join(format!("datarig-export-parquet-full-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let partial = dir.join("x.parquet.partial");
    std::fs::write(&partial, "PAR1").unwrap();
    let columns = [ParquetColumn { name: "t", kind: ValueKind::Text, type_name: "text" }];
    let out = BufWriter::new(Full { room: 1024, written: 0 });
    let mut w = ParquetWriter::new(&columns, out).unwrap().row_groups(1, parquet::GROUP_BYTES);
    // The row group of the first row does not fit: the rows stop for that.
    let big = "x".repeat(64 * 1024);
    let e = w.rows(&[vec![Some(&big)]]).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::StorageFull);
    let mut finished = 0;
    let ended = end_parquet(w, &partial, |_| {
        finished += 1;
        Ok(0)
    });
    // The first error is the failure (not "a row group failed" with no cause), and the partial
    // file, which does not read, is gone.
    match ended {
        Err(Failure::NotFinished(f)) => {
            assert_eq!(f.kind, datarig_core::fault::FaultKind::Io(std::io::ErrorKind::StorageFull), "{f:?}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(finished, 0, "nothing synced");
    assert!(!partial.exists());
    let _ = std::fs::remove_dir_all(&dir);
}
