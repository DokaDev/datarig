use super::*;
use datarig_core::query::{Cond, Dir, OrderKey};

fn users() -> Query {
    Query::table(TableName { database: None, schema: "shop".into(), name: "users".into() })
}

fn filtered(text: &str, recognised: Option<Cond>) -> Query {
    Query { filter: Some(Filter { text: text.into(), recognised }), ..users() }
}

#[test]
fn same_query_leaves_out_what_was_recognised_and_blank_filters() {
    let a = filtered("id > 1", None);
    let b = filtered("id > 1", Some(Cond::Raw("id > 1".into())));
    assert!(same_query(&a, &b));
    assert!(!same_query(&a, &filtered("id > 2", None)));
    assert!(same_query(&filtered("  ", None), &users()));
    assert!(!same_query(&a, &users()));
    let ordered = Query { order: vec![OrderKey::user("id", Dir::Desc)], ..users() };
    assert!(!same_query(&ordered, &users()));
    assert!(same_rows(&ordered, &users()));
    assert!(!same_rows(&Query { limit: Some(5), ..users() }, &users()));
    let other = Query::table(TableName { database: None, schema: "shop".into(), name: "orders".into() });
    assert!(!same_rows(&other, &users()));
}

#[test]
fn the_query_is_bound_to_the_run_that_sends_its_statement() {
    let mut tq = TableQuery::new(match users().source {
        query::Source::Table(t) => t,
    });
    let q = filtered("id > 1", None);
    let sql = "SELECT * FROM \"shop\".\"users\" WHERE (id > 1)".to_string();
    tq.pending = Some((q.clone(), sql.clone()));
    tq.sent(7, std::slice::from_ref(&sql));
    assert_eq!(tq.requested, Some((7, q.clone())));
    // The first page of another run is not of it.
    tq.first_page(6);
    assert_eq!(tq.shown, None);
    tq.first_page(7);
    assert_eq!(tq.shown, Some(q.clone()));
    assert_eq!(tq.shown_filter(), Some("id > 1"));
    // A run of another statement: nothing is claimed about its rows.
    tq.pending = Some((q, sql));
    tq.sent(8, &["SELECT 1".to_string()]);
    assert_eq!(tq.requested, None);
    tq.first_page(8);
    assert_eq!(tq.shown, None);
}

#[test]
fn every_refusal_has_its_own_words() {
    use datarig_core::sql::risk::{Class, ReadOnlyBlock};
    let i18n = I18n::new(datarig_i18n::Lang::En);
    let lex = |problem| BarRefusal::Lexical { problem, at: 0..1 };
    let shape = |problem| BarRefusal::Shape { problem, at: None };
    let all = [
        lex(LexProblem::Semicolon),
        lex(LexProblem::Comment),
        lex(LexProblem::ExecutableComment),
        lex(LexProblem::ClientCommand),
        lex(LexProblem::Unclosed),
        lex(LexProblem::ClosesOutside),
        lex(LexProblem::Unterminated),
        lex(LexProblem::Ambiguous),
        shape(ShapeProblem::NotOneSelect),
        shape(ShapeProblem::Source),
        shape(ShapeProblem::SetOperation),
        shape(ShapeProblem::Clause),
        shape(ShapeProblem::Locking),
        shape(ShapeProblem::Into),
        shape(ShapeProblem::Assignment),
        BarRefusal::NotRead { class: Class::Write, danger: None },
        BarRefusal::RefusedCalls(vec!["pg_sleep".into()]),
        BarRefusal::UncheckedCalls(vec!["my_fn".into()]),
        BarRefusal::ReadOnly(ReadOnlyBlock::ReadWrite),
    ];
    let words: Vec<String> = all.iter().map(|r| refusal_why(&i18n, r)).collect();
    for (i, w) in words.iter().enumerate() {
        assert!(!w.is_empty(), "{:?}", all[i]);
        assert!(!words[..i].contains(w), "{w}");
    }
    assert!(words[16].contains("pg_sleep") && words[17].contains("my_fn"));
    assert_eq!(refusal_at(&lex(LexProblem::Comment)), Some(0..1));
    assert_eq!(refusal_at(&BarRefusal::RefusedCalls(vec![])), None);
}

#[test]
fn every_order_problem_has_its_own_words() {
    let i18n = I18n::new(datarig_i18n::Lang::En);
    let all = [
        OrderProblem::MissingColumn,
        OrderProblem::NotAColumn,
        OrderProblem::UnknownColumn("x".into()),
        OrderProblem::AmbiguousColumn("x".into()),
        OrderProblem::Duplicate("x".into()),
        OrderProblem::Unterminated,
        OrderProblem::NullsUnsupported,
        OrderProblem::NullsIncomplete,
        OrderProblem::Unexpected,
    ];
    let words: Vec<String> =
        all.iter().map(|k| order_why(&i18n, &OrderError { range: 0..1, kind: k.clone() })).collect();
    for (i, w) in words.iter().enumerate() {
        assert!(!w.is_empty() && !words[..i].contains(w), "{w}");
    }
}
