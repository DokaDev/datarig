use super::*;

const TOKENS: Tokens = Tokens { colors: &["bg", "fg", "warning"], styles: &["selection", "danger_mark"] };

fn err(text: &str) -> (Option<usize>, Problem) {
    parse(text, TOKENS).expect_err("an error")
}

#[test]
fn colors_are_hex_ansi_names_or_default() {
    assert_eq!(ColorSpec::parse("#1e1E2e"), Some(ColorSpec::Rgb(0x1e, 0x1e, 0x2e)));
    assert_eq!(ColorSpec::parse("red"), Some(ColorSpec::Ansi(1)));
    assert_eq!(ColorSpec::parse("Bright-Black"), Some(ColorSpec::Ansi(8)));
    assert_eq!(ColorSpec::parse("bright-white"), Some(ColorSpec::Ansi(15)));
    assert_eq!(ColorSpec::parse("default"), Some(ColorSpec::Default));
    for bad in ["#123", "#12345g", "1e1e2e", "grey", "", "#1234567"] {
        assert_eq!(ColorSpec::parse(bad), None, "{bad}");
    }
}

#[test]
fn a_file_is_read_in_file_order() {
    let text = "extends = \"dark\"\n\n[colors]\nwarning = \"yellow\"\nbg = \"#000000\"\n\n[styles]\nselection = { bg = \"blue\", modifiers = [\"bold\", \"Reversed\"] }\ndanger_mark = {}\n";
    let spec = parse(text, TOKENS).unwrap();
    assert_eq!(spec.extends, Some(("dark".into(), 1)));
    assert_eq!(spec.colors, [("warning".to_string(), ColorSpec::Ansi(3)), ("bg".to_string(), ColorSpec::Rgb(0, 0, 0))]);
    assert_eq!(
        spec.styles,
        [
            (
                "selection".to_string(),
                StyleSpec {
                    fg: None,
                    bg: Some(ColorSpec::Ansi(4)),
                    modifiers: vec![ModifierSpec::Bold, ModifierSpec::Reversed]
                }
            ),
            ("danger_mark".to_string(), StyleSpec::default()),
        ]
    );
    assert_eq!(parse("", TOKENS).unwrap(), ThemeSpec::default());
}

#[test]
fn every_problem_has_its_line_and_the_first_one_is_reported() {
    assert_eq!(err("[colors]\nfg = \"#fff\"\n"), (Some(2), Problem::BadColor("#fff".into())));
    assert_eq!(err("[colors]\nborder_x = \"red\"\n"), (Some(2), Problem::UnknownToken("border_x".into())));
    assert_eq!(err("[colors]\nselection = \"red\"\n"), (Some(2), Problem::NotAColor("selection".into())));
    assert_eq!(err("[styles]\nfg = { fg = \"red\" }\n"), (Some(2), Problem::NotAStyle("fg".into())));
    assert_eq!(
        err("[styles]\nselection = { modifiers = [\"blink\"] }\n"),
        (Some(2), Problem::BadModifier("blink".into()))
    );
    assert_eq!(err("[styles]\n\nselection = { fg = \"nope\" }\n"), (Some(3), Problem::BadColor("nope".into())));
    // Keys are checked in file order, not by name.
    assert_eq!(
        err("[colors]\nwarning = \"x\"\nbg = \"y\"\n"),
        (Some(2), Problem::BadColor("x".into())),
        "warning comes first although bg sorts first"
    );
    let (line, p) = err("theme = \"x\"\n");
    assert_eq!(line, Some(1));
    assert!(matches!(p, Problem::Toml(m) if m.contains("theme")), "an unknown top-level key");
    let (line, p) = err("[styles]\nselection = { fg = \"red\", underline = true }\n");
    assert_eq!(line, Some(2));
    assert!(matches!(p, Problem::Toml(m) if m.contains("underline")));
    assert!(matches!(err("[colors\n").1, Problem::Toml(_)));
    assert!(matches!(err("[colors]\nfg = 3\n"), (Some(2), Problem::Toml(_))));
}

#[test]
fn names_are_plain_file_names() {
    for ok in ["mine", "my-theme_2", "A"] {
        assert!(valid_name(ok), "{ok}");
    }
    for bad in ["", "../x", "a b", "x.toml", "a/b", &"x".repeat(65)] {
        assert!(!valid_name(bad), "{bad}");
    }
}

#[test]
fn files_are_listed_and_loaded_from_the_themes_directory() {
    let root = std::env::temp_dir().join(format!("datarig-themes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let dir = dir(&root.join("config.toml")).unwrap();
    assert_eq!(dir, root.join("themes"));
    assert!(names(&dir).is_empty(), "no directory: no themes");
    std::fs::create_dir_all(dir.join("folder.toml")).unwrap();
    std::fs::write(dir.join("zeta.toml"), "[colors]\nfg = \"red\"\n").unwrap();
    std::fs::write(dir.join("alpha.toml"), "").unwrap();
    std::fs::write(dir.join("bad name.toml"), "").unwrap();
    std::fs::write(dir.join("notes.txt"), "").unwrap();
    assert_eq!(names(&dir), ["alpha", "zeta"]);
    assert_eq!(load(&dir, "zeta", TOKENS).unwrap().colors, [("fg".to_string(), ColorSpec::Ansi(1))]);
    assert_eq!(load(&dir, "gone", TOKENS), Err(ThemeError::Unknown { name: "gone".into(), dir: Some(dir.clone()) }));
    assert_eq!(
        load(&dir, "../config", TOKENS),
        Err(ThemeError::Unknown { name: "../config".into(), dir: Some(dir.clone()) }),
        "a name is never a path"
    );
    std::fs::write(dir.join("broken.toml"), "[colors]\n\nwarning = \"orange\"\n").unwrap();
    assert_eq!(
        load(&dir, "broken", TOKENS),
        Err(ThemeError::File {
            path: dir.join("broken.toml"),
            line: Some(3),
            problem: Problem::BadColor("orange".into())
        })
    );
    std::fs::remove_dir_all(&root).unwrap();
}
