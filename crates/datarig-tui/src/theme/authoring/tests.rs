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
