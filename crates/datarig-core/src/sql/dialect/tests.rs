use super::*;

#[test]
fn defaults_are_postgres() {
    assert_eq!(Dialect::default(), Dialect::Postgres);
    assert_eq!(Language::default(), Language::Sql(Dialect::Postgres));
}

#[test]
fn language_names_its_dialect() {
    assert_eq!(Language::Sql(Dialect::Postgres).dialect(), Dialect::Postgres);
    assert_eq!(Language::default().dialect(), Dialect::default());
}
