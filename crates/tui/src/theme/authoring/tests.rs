use super::*;
use crate::theme::{BUILTINS, COLOR_TOKENS, DARK, STYLE_TOKENS, TOKENS, resolve};

/// A scratch themes directory under the temp dir (never the real config dir); removed when
/// dropped.
struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("datarig-authoring-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Dir(dir.join("themes"))
    }

    fn text(&self, name: &str) -> String {
        std::fs::read_to_string(datarig_core::theme::path(&self.0, name)).unwrap()
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        if let Some(parent) = self.0.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}

#[test]
fn names_are_plain_file_names_and_never_a_builtin_one() {
    for ok in ["mine", "my-theme_2", "A", &"x".repeat(64)] {
        assert_eq!(check_name(ok), Ok(()), "{ok}");
    }
    for bad in ["", "my theme", "../mine", "mine.toml", "a/b", "caf\u{e9}", &"x".repeat(65), " mine"] {
        assert_eq!(check_name(bad), Err(AuthorError::BadName(bad.to_string())), "{bad}");
    }
    for builtin in ["dark", "Dark", "TERMINAL", "catppuccin", "gruvbox-light", "High-Contrast"] {
        assert_eq!(check_name(builtin), Err(AuthorError::BuiltinName(builtin.to_string())), "{builtin}");
    }
    // Windows device names, whatever the case; longer names that start like one are fine.
    for device in ["con", "PRN", "Aux", "nul", "com1", "COM9", "lpt1", "Lpt9"] {
        assert_eq!(check_name(device), Err(AuthorError::BadName(device.to_string())), "{device}");
    }
    for ok in ["com0", "com10", "lpt", "console", "nul-theme", "auxiliary"] {
        assert_eq!(check_name(ok), Ok(()), "{ok}");
    }
}

#[test]
fn a_copy_of_every_builtin_loads_as_that_theme() {
    let d = Dir::new("roundtrip");
    let mut copies = 0;
    for background in [Background::Dark, Background::Light] {
        for name in NAMES {
            let new = format!("copy-{name}-{background:?}").to_lowercase();
            let path = copy_theme(&d.0, name, &new, background).unwrap();
            assert_eq!(path, datarig_core::theme::path(&d.0, &new));
            let want = builtin(name, background).unwrap();
            assert_eq!(resolve(&new, background, Some(&d.0)).as_ref(), Ok(want), "{name} {background:?}");
            // Every token is set: it is the same theme on the other background too.
            let other = if background == Background::Dark { Background::Light } else { Background::Dark };
            let spec = datarig_core::theme::parse(&d.text(&new), TOKENS).unwrap();
            assert_eq!(spec.colors.len(), COLOR_TOKENS.len(), "{name}");
            assert_eq!(spec.styles.len(), STYLE_TOKENS.len(), "{name}");
            let (base, _) = spec.extends.clone().unwrap();
            assert!(BUILTINS.iter().any(|(b, _)| *b == base), "extends a variant, not a family: {base}");
            let theme_file = crate::theme::from_spec(&spec, builtin(&base, other).unwrap().clone());
            assert_eq!(&theme_file, want, "{name}");
            copies += 1;
        }
    }
    assert_eq!(copies, 2 * NAMES.len());
}

#[test]
fn a_copy_of_a_theme_file_keeps_its_tokens_and_its_base() {
    let d = Dir::new("user");
    std::fs::create_dir_all(&d.0).unwrap();
    std::fs::write(
        d.0.join("mine.toml"),
        "extends = \"gruvbox\"\n[colors]\nwarning = \"bright-yellow\"\n[styles]\nselection = { bg = \"#123456\", modifiers = [\"bold\", \"crossed-out\"] }\n",
    )
    .unwrap();
    for background in [Background::Light, Background::Dark] {
        let new = format!("mine-{background:?}").to_lowercase();
        copy_theme(&d.0, "mine", &new, background).unwrap();
        let want = resolve("mine", background, Some(&d.0)).unwrap();
        assert_eq!(resolve(&new, background, Some(&d.0)), Ok(want));
        let base = if background == Background::Light { "gruvbox-light" } else { "gruvbox-dark" };
        assert!(d.text(&new).contains(&format!("\nextends = \"{base}\"\n")), "{}", d.text(&new));
    }
}

#[test]
fn a_name_that_is_taken_is_refused_and_the_file_kept() {
    let d = Dir::new("taken");
    std::fs::create_dir_all(&d.0).unwrap();
    let path = datarig_core::theme::path(&d.0, "mine");
    std::fs::write(&path, "# mine, by hand\n").unwrap();
    assert_eq!(copy_theme(&d.0, "dark", "mine", Background::Dark), Err(AuthorError::Exists(path.clone())));
    assert_eq!(new_theme(&d.0, "mine", "dark", Background::Dark), Err(AuthorError::Exists(path.clone())));
    assert_eq!(d.text("mine"), "# mine, by hand\n", "never replaced");
    // A second copy under the same name is refused too: the first one stays.
    copy_theme(&d.0, "nord", "other", Background::Dark).unwrap();
    let first = d.text("other");
    assert!(matches!(copy_theme(&d.0, "dracula", "other", Background::Dark), Err(AuthorError::Exists(_))));
    assert_eq!(d.text("other"), first);
}

#[test]
fn bad_names_unknown_themes_and_unwritable_folders_are_errors() {
    let d = Dir::new("errors");
    assert_eq!(copy_theme(&d.0, "dark", "light", Background::Dark), Err(AuthorError::BuiltinName("light".into())));
    assert_eq!(new_theme(&d.0, "a b", "dark", Background::Dark), Err(AuthorError::BadName("a b".into())));
    assert!(matches!(
        copy_theme(&d.0, "solarized", "mine", Background::Dark),
        Err(AuthorError::Source(ThemeError::Unknown { .. }))
    ));
    assert!(!d.0.exists(), "nothing written for a refused name or source");
    // A file where the folder should be: the write fails and says where (not "taken").
    std::fs::create_dir_all(d.0.parent().unwrap()).unwrap();
    std::fs::write(&d.0, "not a folder").unwrap();
    match copy_theme(&d.0, "dark", "mine", Background::Dark) {
        Err(AuthorError::Write { path, .. }) => assert_eq!(path, d.0),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_new_file_sets_nothing_and_loads_as_the_theme_it_extends() {
    let d = Dir::new("new");
    for base in NAMES {
        let name = format!("new-{base}");
        new_theme(&d.0, &name, base, Background::Light).unwrap();
        let text = d.text(&name);
        assert!(text.contains(&format!("\nextends = \"{base}\"\n")), "{text}");
        let spec = datarig_core::theme::parse(&text, TOKENS).unwrap();
        assert!(spec.colors.is_empty() && spec.styles.is_empty(), "every token commented out");
        assert_eq!(
            resolve(&name, Background::Light, Some(&d.0)).as_ref(),
            Ok(builtin(base, Background::Light).unwrap())
        );
    }
    // A key set by removing its "# " takes over; the others stay the base's.
    let text = d.text("new-dark").replace("# warning = ", "warning = ");
    let spec = datarig_core::theme::parse(&text, TOKENS).unwrap();
    let th = crate::theme::from_spec(&spec, DARK);
    assert_eq!(th, DARK, "the commented value is the inherited one");
    let text = text.replace(&format!("warning = {}", color_text(DARK.warning).unwrap()), "warning = \"red\"");
    let th = crate::theme::from_spec(&datarig_core::theme::parse(&text, TOKENS).unwrap(), DARK);
    assert_eq!(th, Theme { warning: Color::Red, ..DARK });
}

#[test]
fn a_new_file_extending_a_family_says_which_variant_its_values_are() {
    let d = Dir::new("family");
    new_theme(&d.0, "fam-dark", "catppuccin", Background::Dark).unwrap();
    new_theme(&d.0, "fam-light", "catppuccin", Background::Light).unwrap();
    let dark = d.text("fam-dark");
    assert!(dark.contains("\"catppuccin\" (its dark variant, catppuccin-mocha, drawn now)"), "{dark}");
    assert!(dark.contains("\nextends = \"catppuccin\"\n"), "the family: it keeps following the background");
    assert!(d.text("fam-light").contains("(its light variant, catppuccin-latte, drawn now)"));
    let mocha = color_text(crate::theme::CATPPUCCIN_MOCHA.bg).unwrap();
    assert!(dark.contains(&format!("\n# bg = {mocha}\n")), "{dark}");
}

#[test]
fn every_key_has_its_description_on_the_line_above() {
    let d = Dir::new("comments");
    copy_theme(&d.0, "high-contrast", "full", Background::Dark).unwrap();
    new_theme(&d.0, "empty", "terminal", Background::Dark).unwrap();
    for (name, prefix) in [("full", ""), ("empty", "# ")] {
        let text = d.text(name);
        let lines: Vec<&str> = text.lines().collect();
        for (token, about) in COLOR_ABOUT.iter().chain(STYLE_ABOUT) {
            let at = lines
                .iter()
                .position(|l| l.starts_with(&format!("{prefix}{token} = ")))
                .unwrap_or_else(|| panic!("{name}: no key {token}"));
            assert_eq!(lines[at - 1], format!("# {about}"), "{name}: {token}");
        }
    }
}

#[test]
fn every_token_is_described_once_and_every_theme_field_is_a_token() {
    let names: Vec<&str> = COLOR_ABOUT.iter().chain(STYLE_ABOUT).map(|(t, _)| *t).collect();
    let mut unique = names.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), names.len(), "a token twice");
    for (token, about) in COLOR_ABOUT.iter().chain(STYLE_ABOUT) {
        assert!(!about.is_empty() && !about.contains('\n') && about.ends_with('.'), "{token}: {about:?}");
        assert!(!about.contains('|'), "{token}: breaks the table of docs/themes.md");
    }
    // The fields of `Theme`, from its Debug text: each one but `dim` is a token.
    let debug = format!("{DARK:?}");
    let mut fields = Vec::new();
    let mut depth = 0;
    let mut word = String::new();
    let chars: Vec<char> = debug.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth -= 1,
            // `name: value`, not a path (`Style::new()`).
            ':' if depth == 1 && chars.get(i + 1) == Some(&' ') && !word.is_empty() => fields.push(word.clone()),
            _ => {}
        }
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
        } else if c != ':' {
            word.clear();
        }
    }
    fields.retain(|f| f != "dim");
    assert_eq!(fields.len(), names.len(), "{fields:?}");
    for f in &fields {
        assert!(names.contains(&f.as_str()), "{f} has no description");
    }
}

#[test]
fn the_values_are_written_as_the_file_format_reads_them() {
    assert_eq!(color_text(Color::Reset).as_deref(), Some("\"default\""));
    assert_eq!(color_text(Color::Rgb(0x0a, 0xbc, 0xff)).as_deref(), Some("\"#0abcff\""));
    assert_eq!(color_text(Color::DarkGray).as_deref(), Some("\"bright-black\""));
    assert_eq!(color_text(Color::Indexed(200)), None, "not a color a file can hold");
    assert_eq!(style_text(Style::new()).as_deref(), Some("{}"));
    assert_eq!(
        style_text(Style::new().fg(Color::Red).bg(Color::Rgb(1, 2, 3)).add_modifier(Modifier::BOLD | Modifier::ITALIC))
            .as_deref(),
        Some("{ fg = \"red\", bg = \"#010203\", modifiers = [\"bold\", \"italic\"] }")
    );
    assert_eq!(style_text(Style::new().add_modifier(Modifier::SLOW_BLINK)), None);
    assert_eq!(style_text(Style::new().remove_modifier(Modifier::BOLD)), None);
}

#[test]
fn the_example_of_the_doc_is_a_valid_theme_file() {
    let spec = datarig_core::theme::parse(DOC_EXAMPLE, TOKENS).unwrap();
    assert_eq!(spec.extends.map(|e| e.0).as_deref(), Some("dark"));
    let doc = doc();
    assert!(doc.contains(DOC_EXAMPLE));
    for (token, about) in COLOR_ABOUT.iter().chain(STYLE_ABOUT) {
        assert!(doc.contains(&format!("| `{token}` | {about} |")), "{token}");
    }
}

/// Theme file `name` of `d` with `text`.
fn put(d: &Dir, name: &str, text: &str) -> PathBuf {
    std::fs::create_dir_all(&d.0).unwrap();
    let path = datarig_core::theme::path(&d.0, name);
    std::fs::write(&path, text).unwrap();
    path
}

/// `text` without the lines of `tokens` (a key line and the description above it), as a file
/// written before those tokens existed.
fn without(text: &str, tokens: &[&str]) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let key = |l: &str| tokens.iter().any(|t| l.starts_with(&format!("{t} = ")));
        if key(l) || lines.get(i + 1).is_some_and(|n| key(n)) {
            continue;
        }
        out.push(*l);
    }
    out.join("\n") + "\n"
}

/// Every line of `old` is in `new`, unchanged and in order; every other line of `new` is a
/// comment, blank, or one of `headers`.
fn only_added(old: &str, new: &str, headers: &[&str]) {
    let mut added = Vec::new();
    let mut old_lines = old.lines().peekable();
    for l in new.lines() {
        if old_lines.peek() == Some(&l) {
            old_lines.next();
        } else {
            added.push(l);
        }
    }
    assert_eq!(old_lines.next(), None, "a line of the file changed or went:\n{new}");
    for l in added {
        assert!(l.is_empty() || l.starts_with("# ") || headers.contains(&l), "added {l:?}");
    }
}

#[test]
fn an_old_copy_gets_what_it_does_not_list_commented_out_at_the_end_of_each_section() {
    let d = Dir::new("update-old");
    copy_theme(&d.0, "dark", "mine", Background::Unknown).unwrap();
    let old = without(&d.text("mine"), &["fg_dim", "grid_cursor_row", "grid_cursor_header"]);
    let path = put(&d, "mine", &old);
    let before = resolve("mine", Background::Unknown, Some(&d.0)).unwrap();
    let spec = datarig_core::theme::parse(&old, TOKENS).unwrap();
    assert_eq!(unlisted(&old, &spec), (vec!["fg_dim"], vec!["grid_cursor_row", "grid_cursor_header"]));
    assert_eq!(update_theme(&d.0, "mine", Background::Unknown), Ok(Updated::Added { path: path.clone(), count: 3 }));
    let new = d.text("mine");
    only_added(&old, &new, &[]);
    // Each where it is set when uncommented: the color in [colors], the styles in [styles].
    let colors_at = new.find("[colors]").unwrap();
    let styles_at = new.find("[styles]").unwrap();
    let fg_dim = new.find("# fg_dim = ").unwrap();
    assert!(colors_at < fg_dim && fg_dim < styles_at, "{new}");
    assert!(new.find("# grid_cursor_row = ").unwrap() > styles_at, "{new}");
    // With the value it takes now, and the description above.
    assert!(new.contains(&format!("# fg_dim = {}\n", color_text(DARK.fg_dim).unwrap())), "{new}");
    // Nothing drawn changes; everything is listed now; a second update adds nothing.
    assert_eq!(resolve("mine", Background::Unknown, Some(&d.0)).unwrap(), before);
    let spec = datarig_core::theme::parse(&new, TOKENS).unwrap();
    assert_eq!(unlisted(&new, &spec), (Vec::new(), Vec::new()));
    assert_eq!(update_theme(&d.0, "mine", Background::Unknown), Ok(Updated::Complete { path }));
    assert_eq!(d.text("mine"), new);
    // Uncommented, a line added is a valid key of its section.
    let set = new.replacen("# fg_dim = ", "fg_dim = ", 1).replacen("# grid_cursor_row = ", "grid_cursor_row = ", 1);
    let spec = datarig_core::theme::parse(&set, TOKENS).unwrap();
    assert!(spec.colors.iter().any(|c| c.0 == "fg_dim") && spec.styles.iter().any(|s| s.0 == "grid_cursor_row"));
}

#[test]
fn a_short_file_gets_the_sections_it_lacks_and_a_new_file_lists_everything() {
    let d = Dir::new("update-short");
    let old = "# mine\nextends = \"light\"";
    put(&d, "short", old);
    let Ok(Updated::Added { count, .. }) = update_theme(&d.0, "short", Background::Unknown) else { panic!() };
    assert_eq!(count, COLOR_TOKENS.len() + STYLE_TOKENS.len());
    let new = d.text("short");
    only_added(old, &new, &["[colors]", "[styles]"]);
    assert!(new.find("[colors]").unwrap() < new.find("[styles]").unwrap(), "{new}");
    let spec = datarig_core::theme::parse(&new, TOKENS).unwrap();
    assert!(spec.colors.is_empty() && spec.styles.is_empty());
    // The doc's example: only what it does not set.
    put(&d, "example", DOC_EXAMPLE);
    let Ok(Updated::Added { count, .. }) = update_theme(&d.0, "example", Background::Unknown) else { panic!() };
    assert_eq!(count, COLOR_TOKENS.len() + STYLE_TOKENS.len() - 4);
    only_added(DOC_EXAMPLE, &d.text("example"), &[]);
    // `:theme new` lists every token commented out already.
    let path = new_theme(&d.0, "fresh", "dark", Background::Unknown).unwrap();
    assert_eq!(unlisted_of(&d.0, "fresh"), Some(Vec::new()));
    assert_eq!(update_theme(&d.0, "fresh", Background::Unknown), Ok(Updated::Complete { path }));
}

#[test]
fn an_update_that_cannot_be_done_safely_leaves_the_file_as_it_is() {
    let d = Dir::new("update-refused");
    assert_eq!(update_theme(&d.0, "Dark", Background::Unknown), Err(AuthorError::NotAFile("Dark".into())));
    assert!(matches!(
        update_theme(&d.0, "none", Background::Unknown),
        Err(AuthorError::Source(ThemeError::Unknown { .. }))
    ));
    assert_eq!(update_theme(&d.0, "../x", Background::Unknown), Err(AuthorError::BadName("../x".into())));
    // A file with an error: said, never touched.
    let broken = "extends = \"dark\"\n[colors]\nbg = \"nope\"\n";
    put(&d, "broken", broken);
    assert!(matches!(
        update_theme(&d.0, "broken", Background::Unknown),
        Err(AuthorError::Source(ThemeError::File { line: Some(3), .. }))
    ));
    assert_eq!(d.text("broken"), broken);
    assert_eq!(unlisted_of(&d.0, "broken"), None);
    // Colors as dotted keys: a [colors] table added would not read; refused.
    let dotted = "extends = \"dark\"\ncolors.bg = \"#000000\"\n";
    let path = put(&d, "dotted", dotted);
    assert_eq!(update_theme(&d.0, "dotted", Background::Unknown), Err(AuthorError::Unchanged(path)));
    assert_eq!(d.text("dotted"), dotted);
    // A file that may not be written is not replaced.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = put(&d, "locked", "extends = \"dark\"\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(matches!(update_theme(&d.0, "locked", Background::Unknown), Err(AuthorError::Write { .. })));
        assert_eq!(d.text("locked"), "extends = \"dark\"\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
}

#[test]
fn a_file_with_windows_line_breaks_or_a_bom_keeps_them() {
    let d = Dir::new("update-crlf");
    copy_theme(&d.0, "dark", "win", Background::Unknown).unwrap();
    let old = without(&d.text("win"), &["fg_dim", "grid_cursor_row"]).replace('\n', "\r\n");
    put(&d, "win", &old);
    assert!(matches!(update_theme(&d.0, "win", Background::Unknown), Ok(Updated::Added { count: 2, .. })));
    let new = d.text("win");
    assert!(!new.replace("\r\n", "").contains('\n'), "only CRLF line breaks");
    only_added(&old.replace("\r\n", "\n"), &new.replace("\r\n", "\n"), &[]);
    // A BOM before the first table: its header is still found.
    let bom = "\u{feff}[colors]\naccent = \"#89b4fa\"\n";
    put(&d, "bom", bom);
    assert!(matches!(update_theme(&d.0, "bom", Background::Unknown), Ok(Updated::Added { .. })));
    let new = d.text("bom");
    assert!(new.starts_with(bom) && new.matches("[colors]").count() == 1, "{new}");
}

#[test]
fn quoted_table_names_get_the_lines_in_their_own_table() {
    let d = Dir::new("update-quoted");
    copy_theme(&d.0, "dark", "quoted", Background::Unknown).unwrap();
    let old = without(&d.text("quoted"), &["fg_dim", "grid_cursor_row"])
        .replace("\n[colors]\n", "\n[\"colors\"]\n")
        .replace("\n[styles]\n", "\n['styles']  # quoted\n");
    let path = put(&d, "quoted", &old);
    assert_eq!(update_theme(&d.0, "quoted", Background::Unknown), Ok(Updated::Added { path, count: 2 }));
    let new = d.text("quoted");
    only_added(&old, &new, &[]);
    assert!(!new.contains("\n[colors]") && !new.contains("\n[styles]"), "no second table: {new}");
    let colors_at = new.find("[\"colors\"]").unwrap();
    let styles_at = new.find("['styles']").unwrap();
    let fg_dim = new.find("# fg_dim = ").unwrap();
    assert!(colors_at < fg_dim && fg_dim < styles_at && new.find("# grid_cursor_row = ").unwrap() > styles_at);
}
