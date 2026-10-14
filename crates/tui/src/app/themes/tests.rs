use super::*;

#[test]
fn every_builtin_name_has_its_label() {
    for name in theme::NAMES {
        assert!(builtin_label(name).is_some(), "{name}");
    }
    assert_eq!(builtin_label("mine"), None, "a theme file");
}

#[test]
fn theme_errors_say_the_file_the_line_and_the_problem() {
    let i18n = I18n::new(Lang::En);
    let path = PathBuf::from("themes").join("mine.toml");
    let e = ThemeError::File { path: path.clone(), line: Some(4), problem: Problem::BadColor("orange".into()) };
    let text = i18n.msg(&theme_error_msg(&i18n, &e)).to_string();
    assert!(text.starts_with(&format!("{}:4: ", path.display())) && text.contains("“orange” is not a color"), "{text}");
    let e = ThemeError::File { path: path.clone(), line: None, problem: Problem::Toml("expected `=`".into()) };
    assert_eq!(i18n.msg(&theme_error_msg(&i18n, &e)).to_string(), format!("{}: expected `=`", path.display()));
    let e = ThemeError::Unknown { name: "mine".into(), dir: Some(PathBuf::from("themes")) };
    let text = i18n.msg(&theme_error_msg(&i18n, &e)).to_string();
    assert!(text.contains("“mine”") && text.contains(&path.display().to_string()), "{text}");
}
