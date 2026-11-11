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
    tq.pending =
        Some(Statement { query: q.clone(), sql: sql.clone(), filter_span: None, order: Vec::new(), total: false });
    tq.sent(7, std::slice::from_ref(&sql));
    assert_eq!(tq.requested, Some((7, q.clone())));
    // The first page of another run is not of it.
    tq.first_page(6);
    assert_eq!(tq.shown, None);
    tq.first_page(7);
    assert_eq!(tq.shown, Some(q.clone()));
    assert_eq!(tq.shown_filter(), Some("id > 1"));
    // A run of another statement: nothing is claimed about its rows.
    tq.pending = Some(Statement { query: q, sql, filter_span: None, order: Vec::new(), total: false });
    tq.sent(8, &["SELECT 1".to_string()]);
    assert_eq!(tq.requested, None);
    tq.first_page(8);
    assert_eq!(tq.shown, None);
}

#[test]
fn a_query_sent_with_another_condition_order_or_limit_keeps_the_one_before() {
    let mut tq = TableQuery::new(match users().source {
        query::Source::Table(t) => t,
    });
    let send = |tq: &mut TableQuery, q: &Query, qid: u64| {
        let c = query::compile(q, &Target::new(Dialect::Postgres, Vec::new()));
        let sql = c.sql.clone();
        tq.pending = Some(Statement::of(q.clone(), c));
        tq.sent(qid, &[sql]);
    };
    // The tab's first run sends what it was opened with: nothing before it.
    send(&mut tq, &users(), 1);
    tq.first_page(1);
    assert_eq!(tq.before_last, None);
    // The same query again (a reload) keeps nothing.
    send(&mut tq, &users(), 2);
    assert_eq!(tq.before_last, None);
    let a = filtered("id > 1", None);
    send(&mut tq, &a, 3);
    assert_eq!(tq.before_last, Some(users()));
    // An order, then a limit: each keeps the one before it, one level only.
    let ordered = Query { order: vec![OrderKey::user("id", Dir::Desc)], ..a.clone() };
    send(&mut tq, &ordered, 4);
    assert_eq!(tq.before_last, Some(a.clone()));
    let capped = Query { limit: Some(5), ..ordered.clone() };
    send(&mut tq, &capped, 5);
    assert_eq!(tq.before_last, Some(ordered));
    // A run of another statement keeps what was kept.
    tq.sent(6, &["SELECT 1".to_string()]);
    assert_eq!(tq.before_last.as_ref().map(|q| q.limit), Some(None));
}

#[test]
fn a_server_error_keeps_what_the_server_said() {
    let mut tq = TableQuery::new(match users().source {
        query::Source::Table(t) => t,
    });
    let q = filtered("idd = 1", None);
    let c = query::compile(&q, &Target::new(Dialect::Postgres, Vec::new()));
    let sql = c.sql.clone();
    tq.pending = Some(Statement::of(q, c));
    tq.sent(1, &[sql]);
    let m = datarig_core::driver::ServerMessage {
        text: "column \"idd\" does not exist".into(),
        position: None,
        hint: None,
    };
    tq.run_failed(1, &ServerError::Message(&m), Dialect::Postgres);
    assert_eq!(tq.failure().and_then(|f| f.message.as_deref()), Some("column \"idd\" does not exist"));
    tq.run_over(2, Unapplied::Ended);
    let mut other = TableQuery::new(match users().source {
        query::Source::Table(t) => t,
    });
    let q = filtered("id = 1", None);
    let c = query::compile(&q, &Target::new(Dialect::Postgres, Vec::new()));
    let sql = c.sql.clone();
    other.pending = Some(Statement::of(q, c));
    other.sent(1, &[sql]);
    other.run_failed(1, &ServerError::Other, Dialect::Postgres);
    assert_eq!(other.failure().map(|f| f.message.clone()), Some(None), "not a server's refusal: nothing said");
}

#[test]
fn a_run_that_ends_without_its_first_page_is_not_applied() {
    let mut tq = TableQuery::new(match users().source {
        query::Source::Table(t) => t,
    });
    let q = filtered("id > 1", None);
    let sql = "SELECT * FROM \"shop\".\"users\" WHERE (id > 1)".to_string();
    let send = |tq: &mut TableQuery, qid: u64| {
        tq.pending =
            Some(Statement { query: q.clone(), sql: sql.clone(), filter_span: None, order: Vec::new(), total: false });
        tq.sent(qid, std::slice::from_ref(&sql));
    };
    send(&mut tq, 1);
    tq.first_page(1);
    // Its first page landed: a cancel after it (or its end) is not a failure of the query.
    tq.run_over(1, Unapplied::Cancelled);
    assert_eq!(tq.failure().map(|f| (f.at.clone(), f.how)), None);
    send(&mut tq, 2);
    // Another run's end says nothing about it.
    tq.run_over(1, Unapplied::Ended);
    assert_eq!(tq.failure().map(|f| (f.at.clone(), f.how)), None);
    tq.run_over(2, Unapplied::Cancelled);
    assert_eq!(tq.failure().map(|f| (f.at.clone(), f.how)), Some((None, Unapplied::Cancelled)));
    assert_eq!(tq.applied(), &q, "the line keeps the query, marked");
    // The server's refusal, marked first, stays the reason.
    send(&mut tq, 3);
    tq.run_failed(3, &ServerError::Other, Dialect::Postgres);
    tq.run_over(3, Unapplied::Ended);
    assert_eq!(tq.failure().map(|f| (f.at.clone(), f.how)), Some((None, Unapplied::Refused)));
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
        shape(ShapeProblem::Sequence),
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
    assert!(words[17].contains("pg_sleep") && words[18].contains("my_fn"));
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

#[test]
fn mysql_quotes_the_text_from_where_it_stopped() {
    let m = "ERROR 1064 (42000): You have an error in your SQL syntax; check the manual that corresponds to \
             your MySQL server version for the right syntax to use near 'name = 'x')' at line 1";
    assert_eq!(mysql_near(m), Some("name = 'x')"));
    assert_eq!(mysql_near("ERROR 1064 (42000): ... near '' at line 1"), None, "nothing quoted");
    assert_eq!(mysql_near("ERROR 1054 (42S22): Unknown column 'x' in 'where clause'"), None);
}

#[test]
fn a_server_error_marks_the_filter_where_it_points() {
    use datarig_core::driver::ServerMessage;
    let table = TableName { database: None, schema: "shop".into(), name: "users".into() };
    let sent = |tq: &mut TableQuery, d: Dialect, filter: &str, qid: u64| {
        let q = filtered(filter, None);
        let c = query::compile(&q, &Target::new(d, Vec::new()));
        let sql = c.sql.clone();
        tq.pending = Some(Statement::of(q, c));
        tq.sent(qid, &[sql]);
    };
    let pg = |position| ServerMessage { text: "ERROR".into(), position: Some(position), hint: None };
    let mut tq = TableQuery::new(table.clone());
    // `SELECT * FROM "shop"."users" WHERE (` is 36 characters: the filter's first is the 37th.
    sent(&mut tq, Dialect::Postgres, "\u{e9}\u{e9} = 1 OR idd = 2", 1);
    tq.run_failed(1, &ServerError::Message(&pg(37 + 10)), Dialect::Postgres);
    assert_eq!(
        tq.failure().map(|f| (f.at.clone(), f.how)),
        Some((Some(12..15), Unapplied::Refused)),
        "characters, not bytes: `idd`"
    );
    // On a blank: that one character.
    tq.run_failed(1, &ServerError::Message(&pg(37 + 9)), Dialect::Postgres);
    assert_eq!(tq.failure().map(|f| (f.at.clone(), f.how)), Some((Some(11..12), Unapplied::Refused)));
    // Before the filter, after it, or no place: failed, nothing marked.
    for p in [3, 37 + 17] {
        tq.run_failed(1, &ServerError::Message(&pg(p)), Dialect::Postgres);
        assert_eq!(tq.failure().map(|f| (f.at.clone(), f.how)), Some((None, Unapplied::Refused)), "{p}");
    }
    tq.run_failed(1, &ServerError::Other, Dialect::Postgres);
    assert_eq!(tq.failure().map(|f| (f.at.clone(), f.how)), Some((None, Unapplied::Refused)));
    // Another run's failure is not this query's; a page of the run clears it.
    let mut other = TableQuery::new(table.clone());
    sent(&mut other, Dialect::Postgres, "id = 1", 2);
    other.run_failed(3, &ServerError::Other, Dialect::Postgres);
    assert_eq!(other.failure(), None);
    tq.first_page(1);
    assert_eq!(tq.failure().map(|f| (f.at.clone(), f.how)), None);
    // MySQL: the quoted text, found once and inside the filter, marks it to the filter's end.
    let my = Dialect::MySql(Default::default());
    let near =
        |s: &str| ServerMessage { text: format!("ERROR 1064 (42000): x near '{s}' at line 1"), ..Default::default() };
    let mut tq = TableQuery::new(table);
    sent(&mut tq, my, "id = 1 AND rank = 1", 4);
    tq.run_failed(4, &ServerError::Message(&near("= 1")), my);
    assert_eq!(
        tq.failure().map(|f| (f.at.clone(), f.how)),
        Some((None, Unapplied::Refused)),
        "twice in the statement: not located"
    );
    tq.run_failed(4, &ServerError::Message(&near("= 1)")), my);
    assert_eq!(
        tq.failure().map(|f| (f.at.clone(), f.how)),
        Some((Some(16..19), Unapplied::Refused)),
        "the statement's `)` is not the filter's"
    );
    tq.run_failed(4, &ServerError::Message(&near("rank = 1)")), my);
    assert_eq!(tq.failure().map(|f| (f.at.clone(), f.how)), Some((Some(11..19), Unapplied::Refused)));
}

#[test]
fn the_order_completes_columns_and_directions() {
    use crate::app::table_filter::order_completion;
    use datarig_core::query::{ColumnFacts, ColumnKind};
    let cols = |d| {
        Target::new(d, ["id", "name", "Nick Name"].map(|n| ColumnFacts::new(n.to_string(), ColumnKind::Other)).into())
    };
    let labels = |text: &str, d, force| {
        order_completion(text, text.len(), &cols(d), force)
            .map(|c| (c.items.into_iter().map(|i| i.label).collect::<Vec<_>>(), c.replace_start))
    };
    let pg = Dialect::Postgres;
    let my = Dialect::MySql(Default::default());
    assert_eq!(labels("", pg, false), None, "nothing typed, not asked");
    assert_eq!(labels("", pg, true), Some((vec!["id".into(), "name".into(), "\"Nick Name\"".into()], 0)));
    assert_eq!(labels("id, N", pg, false), Some((vec!["name".into(), "\"Nick Name\"".into()], 4)));
    assert_eq!(labels("id, \"N", pg, false), Some((vec!["\"Nick Name\"".into()], 4)), "quoted: case kept");
    assert_eq!(labels("id ", pg, true).unwrap().0, ["ASC", "DESC", "NULLS FIRST", "NULLS LAST"]);
    assert_eq!(labels("id ", my, true).unwrap().0, ["ASC", "DESC"], "MySQL has no NULLS FIRST/LAST");
    assert_eq!(labels("id de", pg, false).unwrap().0, ["DESC"]);
    assert_eq!(labels("id desc ", pg, true).unwrap().0, ["NULLS FIRST", "NULLS LAST"]);
    assert_eq!(labels("id desc ", my, true), None);
    assert_eq!(labels("id desc nulls ", pg, true).unwrap().0, ["FIRST", "LAST"]);
    assert_eq!(labels("id desc nulls last ", pg, true), None);
    assert_eq!(labels("`Ni", my, false).unwrap().0, ["`Nick Name`"]);
}

#[test]
fn a_mysql_snippet_found_again_overlapping_is_not_located() {
    use datarig_core::driver::ServerMessage;
    let my = Dialect::MySql(Default::default());
    let mut tq = TableQuery::new(TableName { database: None, schema: "shop".into(), name: "users".into() });
    let q = filtered("name = 'aaa'", None);
    let c = query::compile(&q, &Target::new(my, Vec::new()));
    let sql = c.sql.clone();
    assert_eq!(sql.matches("aa").count(), 1, "once without overlaps: {sql}");
    tq.pending = Some(Statement::of(q, c));
    tq.sent(1, &[sql]);
    let near = ServerMessage { text: "ERROR 1064 (42000): x near 'aa' at line 1".into(), ..Default::default() };
    tq.run_failed(1, &ServerError::Message(&near), my);
    assert_eq!(
        tq.failure().map(|f| (f.at.clone(), f.how)),
        Some((None, Unapplied::Refused)),
        "twice, overlapping: not located"
    );
}

#[test]
fn a_limit_is_a_whole_number_of_rows_or_none() {
    assert_eq!(parse_limit(""), Ok(None));
    assert_eq!(parse_limit("   "), Ok(None));
    assert_eq!(parse_limit("100"), Ok(Some(100)));
    assert_eq!(parse_limit(" 7 "), Ok(Some(7)));
    assert_eq!(parse_limit(&LIMIT_MAX.to_string()), Ok(Some(LIMIT_MAX)));
    // Where the refused text is: what is between the blanks.
    assert_eq!(parse_limit(" 0 "), Err((1..2, LimitProblem::Zero)));
    assert_eq!(parse_limit("000"), Err((0..3, LimitProblem::Zero)));
    for text in ["-1", "1e3", "10.5", "1_000", "1,000", "+5", "ten", "5 rows", "\u{0661}"] {
        assert_eq!(parse_limit(text).map_err(|(_, p)| p), Err(LimitProblem::NotANumber), "{text}");
    }
    assert_eq!(parse_limit(&(LIMIT_MAX + 1).to_string()), Err((0..19, LimitProblem::TooLarge)));
    assert_eq!(parse_limit("99999999999999999999999").map_err(|(_, p)| p), Err(LimitProblem::TooLarge));
}

#[test]
fn a_page_past_the_cap_is_cut_there() {
    // No cap, or one not reached: the page as it came.
    let mut rows = vec![1, 2, 3];
    assert_eq!(cap_page(&mut rows, 0, None, true), None);
    assert_eq!(cap_page(&mut rows, 0, Some(4), true), None);
    assert_eq!(rows, vec![1, 2, 3]);
    // Below a page: cut, and the server had more.
    assert_eq!(cap_page(&mut rows, 0, Some(2), true), Some(true));
    assert_eq!(rows, vec![1, 2]);
    // Reached exactly: nothing cut; whether the server had more says what to close.
    let mut rows = vec![1, 2, 3];
    assert_eq!(cap_page(&mut rows, 0, Some(3), false), Some(false));
    assert_eq!(cap_page(&mut rows, 0, Some(3), true), Some(true));
    assert_eq!(rows.len(), 3);
    // A later page, after the rows held.
    let mut rows = vec![4, 5, 6];
    assert_eq!(cap_page(&mut rows, 3, Some(5), true), Some(true));
    assert_eq!(rows, vec![4, 5]);
    let mut rows = vec![4, 5, 6];
    assert_eq!(cap_page(&mut rows, 3, Some(9), true), None);
}
