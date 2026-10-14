use super::*;

#[test]
fn read_only_goes_after_the_dsn_options() {
    let mut cfg = Config::new();
    read_only(&mut cfg);
    assert_eq!(cfg.get_options(), Some(READ_ONLY_OPTION));
    let mut cfg = Config::from_str("postgres://u@h/d?options=-c%20search_path%3Dshop").unwrap();
    read_only(&mut cfg);
    assert_eq!(cfg.get_options(), Some("-c search_path=shop -c default_transaction_read_only=on"));
    let mut cfg = Config::from_str("postgres://u@h/d?options=-c%20default_transaction_read_only%3Doff").unwrap();
    read_only(&mut cfg);
    let options = cfg.get_options().unwrap();
    assert!(options.ends_with(READ_ONLY_OPTION), "the last setting wins: {options}");
}

#[test]
fn a_schema_is_quoted_and_escaped_as_one_startup_option() {
    assert_eq!(search_path("shop"), "\"shop\", public");
    assert_eq!(search_path("public"), "\"public\"");
    assert_eq!(search_path("My \"x\""), "\"My \"\"x\"\"\", public");
    assert_eq!(option_escape("\"a b\\c\""), "\"a\\ b\\\\c\"");
    let mut cfg = tokio_postgres::Config::new();
    cfg.options("-c statement_timeout=5s");
    add_option(&mut cfg, &format!("-c search_path={}", option_escape(&search_path("a b"))));
    assert_eq!(cfg.get_options(), Some("-c statement_timeout=5s -c search_path=\"a\\ b\",\\ public"));
}
