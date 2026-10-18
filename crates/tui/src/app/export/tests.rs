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
            path: Some("~/out file.json".into())
        }
    );
    assert_eq!(
        args("selection /srv/x.md"),
        ExportArgs { scope: Some(ExportScope::Selection), path: Some("/srv/x.md".into()), ..Default::default() }
    );
    assert_eq!(args("out.csv"), ExportArgs { path: Some("out.csv".into()), ..Default::default() });
    assert_eq!(args("ndjson all a.txt").format, Some(ExportFormat::JsonLines));
    assert_eq!(args("SQL").format, Some(ExportFormat::SqlInsert));
    assert_eq!(parse_args("parquet"), Err(ArgsError::UnknownFormat("parquet".into())));
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
