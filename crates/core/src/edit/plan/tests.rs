use super::super::fake::{ALL, Broken, Fake};
use super::super::{ColumnReason, Duplicates, EditTarget, Exactness, Identity, Inexact, TableColumn, TableRef};
use super::*;
use crate::driver::structure::{ColumnFill, StructureColumn};
use crate::driver::{Cell, ValueKind};
use crate::sql::dialect::{Dialect, Language};
use crate::sql::risk::Classifier;

fn tcol(name: &str, kind: ValueKind) -> Option<TableColumn> {
    let exact = match kind {
        ValueKind::Float => Exactness::Inexact(Inexact::Approximate),
        _ => Exactness::Exact,
    };
    let column = StructureColumn {
        name: name.into(),
        type_name: "t".into(),
        not_null: false,
        default: None,
        fill: ColumnFill::Default,
        enum_labels: Vec::new(),
        collation: None,
    };
    Some(TableColumn { column, kind, exact, repeated: false })
}

/// `app.t` shown as (id, name, score float, upper(name) computed), found by `identity`.
fn target(identity: Identity) -> EditTarget {
    EditTarget {
        table: TableRef { schema: "app".into(), name: "t".into() },
        columns: vec![
            tcol("id", ValueKind::Integer),
            tcol("name", ValueKind::Text),
            tcol("score", ValueKind::Float),
            None,
        ],
        identity,
        all_columns_shown: true,
        unshown_required: Vec::new(),
    }
}

fn keyed() -> EditTarget {
    target(Identity::Key { primary: true, columns: vec![0] })
}

fn by_values(duplicates: Duplicates) -> EditTarget {
    target(Identity::Values { columns: vec![0, 1], duplicates })
}

/// Row `r` as read: (r, "n<r>", 1.5, "N<r>").
fn orig(r: usize) -> Vec<Cell> {
    vec![Some(r.to_string()), Some(format!("n{r}")), Some("1.5".into()), Some(format!("N{r}"))]
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn all_whole(_: usize, _: usize) -> bool {
    true
}

fn plan_with(
    t: &EditTarget,
    b: &EditBatch,
    f: &Fake,
    matches: &[(usize, u64)],
    choices: &[(usize, Choice)],
) -> Result<Plan, PlanError> {
    let matches: BTreeMap<usize, u64> = matches.iter().copied().collect();
    let choices: BTreeMap<usize, Choice> = choices.iter().copied().collect();
    plan(
        t,
        b,
        f,
        &Facts {
            whole: &all_whole,
            matches: &matches,
            choices: &choices,
            invalid: &BTreeSet::new(),
            invalid_for: b.revision(),
        },
    )
}

fn steps(p: &Plan) -> Vec<(&str, Expect)> {
    p.steps.iter().map(|s| (s.sql.as_str(), s.expect)).collect()
}

#[test]
fn a_keyed_update_checks_the_row_then_sets_it_where_its_key_and_changed_values_are_as_read() {
    let mut b = EditBatch::new();
    b.set(&[(4, 1)], &text("it's"), orig);
    let p = plan_with(&keyed(), &b, &ALL, &[], &[]).unwrap();
    let w = r#"id IS NOT DISTINCT FROM '4' AND name IS NOT DISTINCT FROM 'n4'"#;
    assert_eq!(
        steps(&p),
        [
            (format!(r#"SELECT 1 FROM app.t WHERE {w} FOR UPDATE NOWAIT"#).as_str(), Expect::Rows(1)),
            (format!(r#"UPDATE app.t SET name = 'it''s' WHERE {w}"#).as_str(), Expect::Affected(1)),
        ]
    );
    assert_eq!(p.steps[1].origin, Origin::Row(4));
    assert_eq!(p.counts.updates, 1);
}

#[test]
fn an_inexact_or_cut_value_is_left_out_of_the_lost_update_check_but_not_out_of_the_key() {
    let mut b = EditBatch::new();
    b.set(&[(4, 2)], &text("2.5"), orig);
    b.set(&[(5, 1)], &text("x"), orig);
    let p = plan_with(&keyed(), &b, &ALL, &[], &[]).unwrap();
    assert!(p.steps[2].sql.ends_with(r#"WHERE id IS NOT DISTINCT FROM '4'"#), "{}", p.steps[2].sql);
    let whole = |row: usize, col: usize| !(row == 5 && col == 1);
    let none = BTreeMap::new();
    let p = plan(
        &keyed(),
        &b,
        &ALL,
        &Facts {
            whole: &whole,
            matches: &none,
            choices: &BTreeMap::new(),
            invalid: &BTreeSet::new(),
            invalid_for: b.revision(),
        },
    )
    .unwrap();
    assert!(p.steps[3].sql.ends_with(r#"WHERE id IS NOT DISTINCT FROM '5'"#), "{}", p.steps[3].sql);
    let whole = |row: usize, col: usize| !(row == 5 && col == 0);
    let err = plan(
        &keyed(),
        &b,
        &ALL,
        &Facts {
            whole: &whole,
            matches: &none,
            choices: &BTreeMap::new(),
            invalid: &BTreeSet::new(),
            invalid_for: b.revision(),
        },
    );
    assert_eq!(err, Err(PlanError::NotWhole { row: 5, column: 0 }));
}

#[test]
fn a_delete_finds_its_row_by_the_key_and_every_exact_value_and_null_by_is_null() {
    let mut b = EditBatch::new();
    b.delete(&[7], |r| {
        let mut o = orig(r);
        o[1] = None;
        o
    });
    let p = plan_with(&keyed(), &b, &ALL, &[], &[]).unwrap();
    assert_eq!(
        p.steps[1].sql, r#"DELETE FROM app.t WHERE id IS NOT DISTINCT FROM '7' AND name IS NULL"#,
        "score is inexact, upper(name) is not the table's"
    );
    assert_eq!(p.counts.deletes, 1);
}

#[test]
fn new_rows_give_only_the_cells_set_and_every_default_otherwise() {
    let mut b = EditBatch::new();
    b.add(1, None, vec![Value::Default, text("a"), Value::Null, Value::Default]);
    b.add(1, None, vec![Value::Default; 4]);
    let p = plan_with(&keyed(), &b, &ALL, &[], &[]).unwrap();
    assert_eq!(
        steps(&p),
        [
            (r#"INSERT INTO app.t (name, score) VALUES ('a', NULL)"#, Expect::Affected(1)),
            (r#"INSERT INTO app.t DEFAULT VALUES"#, Expect::Affected(1)),
        ]
    );
    let mut b = EditBatch::new();
    let id = b.add(1, None, vec![Value::Default, Value::Default, Value::Default, text("X")]);
    assert_eq!(
        plan_with(&keyed(), &b, &ALL, &[], &[]),
        Err(PlanError::Column { origin: Origin::New(id), column: 3, why: ColumnReason::Computed })
    );
}

#[test]
fn checks_come_first_then_deletes_updates_and_inserts() {
    let mut b = EditBatch::new();
    b.add(0, None, vec![Value::Default; 4]);
    b.set(&[(1, 1)], &text("x"), orig);
    b.delete(&[2], orig);
    let p = plan_with(&keyed(), &b, &ALL, &[], &[]).unwrap();
    let kinds: Vec<&str> = p.steps.iter().map(|s| s.sql.split(' ').next().unwrap()).collect();
    assert_eq!(kinds, ["SELECT", "SELECT", "DELETE", "UPDATE", "INSERT"]);
    assert_eq!(p.counts, Counts { updates: 1, inserts: 1, deletes: 1 });
}

#[test]
fn a_column_that_may_not_be_set_refuses_the_plan() {
    let mut t = keyed();
    t.columns[0].as_mut().unwrap().column.fill = ColumnFill::IdentityAlways;
    let mut b = EditBatch::new();
    b.set(&[(1, 0)], &text("9"), orig);
    assert_eq!(
        plan_with(&t, &b, &ALL, &[], &[]),
        Err(PlanError::Column { origin: Origin::Row(1), column: 0, why: ColumnReason::IdentityAlways })
    );
    let mut b = EditBatch::new();
    b.set(&[(1, 3)], &text("X"), orig);
    assert_eq!(
        plan_with(&t, &b, &ALL, &[], &[]),
        Err(PlanError::Column { origin: Origin::Row(1), column: 3, why: ColumnReason::Computed })
    );
}

#[test]
fn rows_found_by_their_values_are_counted_first_and_must_match_one() {
    let t = by_values(Duplicates::Refuse);
    let mut b = EditBatch::new();
    b.set(&[(3, 2)], &text("2"), orig);
    let counts = count_statements(&t, &b, &ALL, &all_whole).unwrap();
    let w = r#"id IS NOT DISTINCT FROM '3' AND name IS NOT DISTINCT FROM 'n3'"#;
    assert_eq!(counts, [(3, format!(r#"SELECT count(*) FROM app.t WHERE {w}"#))]);
    assert_eq!(count_statements(&keyed(), &b, &ALL, &all_whole), Ok(Vec::new()), "a key needs no count");
    assert_eq!(plan_with(&t, &b, &ALL, &[], &[]), Err(PlanError::NotCounted { row: 3 }));
    assert_eq!(plan_with(&t, &b, &ALL, &[(3, 0)], &[]), Err(PlanError::Gone { row: 3 }));
    assert_eq!(
        plan_with(&t, &b, &ALL, &[(3, 3)], &[(3, Choice::All)]),
        Err(PlanError::Ambiguous { row: 3, matches: 3 })
    );
    let p = plan_with(&t, &b, &ALL, &[(3, 1)], &[]).unwrap();
    assert_eq!(p.steps.iter().map(|s| s.expect).collect::<Vec<_>>(), [Expect::Rows(1), Expect::Affected(1)]);
}

#[test]
fn identical_rows_change_one_or_all_as_chosen() {
    let t = by_values(Duplicates::Choose);
    let mut b = EditBatch::new();
    b.set(&[(3, 1)], &text("x"), orig);
    assert_eq!(plan_with(&t, &b, &ALL, &[(3, 3)], &[]), Err(PlanError::Choose { row: 3, matches: 3 }));
    let all = plan_with(&t, &b, &ALL, &[(3, 3)], &[(3, Choice::All)]).unwrap();
    assert_eq!(all.steps.iter().map(|s| s.expect).collect::<Vec<_>>(), [Expect::Rows(3), Expect::Affected(3)]);
    let one = plan_with(&t, &b, &ALL, &[(3, 3)], &[(3, Choice::One)]).unwrap();
    assert_eq!(one.steps.iter().map(|s| s.expect).collect::<Vec<_>>(), [Expect::Rows(3), Expect::Affected(1)]);
    assert!(one.steps[1].sql.contains("LIMIT 1"), "{}", one.steps[1].sql);
    let no_one = Fake { one: false, ..ALL };
    assert_eq!(plan_with(&t, &b, &no_one, &[(3, 3)], &[(3, Choice::One)]), Err(PlanError::NoOneOfK { row: 3 }));
    assert!(plan_with(&t, &b, &no_one, &[(3, 3)], &[(3, Choice::All)]).is_ok());
}

#[test]
fn a_statement_that_is_not_what_it_must_be_is_an_internal_error_and_nothing_is_planned() {
    let mut b = EditBatch::new();
    b.set(&[(1, 1)], &text("x"), orig);
    let mut d = EditBatch::new();
    d.delete(&[1], orig);
    let writes =
        [Broken::NoWhere, Broken::AlwaysTrue, Broken::Two, Broken::Widened, Broken::Commented, Broken::Subquery];
    let others = [Broken::WritingCheck, Broken::CommittingCheck, Broken::Swapped, Broken::OtherTable, Broken::Joined];
    for broken in writes.into_iter().chain(others) {
        // A swapped or joined builder breaks only updates.
        let only_updates = matches!(broken, Broken::Swapped | Broken::Joined);
        let batches: &[&EditBatch] = if only_updates { &[&b] } else { &[&b, &d] };
        for batch in batches {
            let got = plan_with(&keyed(), batch, &Fake::broken(broken), &[], &[]);
            assert!(matches!(got, Err(PlanError::Internal(_))), "{broken:?}: {got:?}");
        }
    }
    assert!(plan_with(&keyed(), &b, &Fake::broken(Broken::No), &[], &[]).is_ok());
    let mut n = EditBatch::new();
    n.add(0, None, vec![Value::Default; 4]);
    let got = plan_with(&keyed(), &n, &Fake::broken(Broken::InsertDeletes), &[], &[]);
    assert!(matches!(got, Err(PlanError::Internal(_))), "{got:?}");
    let got = count_statements(&by_values(Duplicates::Refuse), &b, &Fake::broken(Broken::DroppingCount), &all_whole);
    assert!(matches!(got, Err(PlanError::Internal(_))), "{got:?}");
    // One of several rows: the format's subquery form passes, the same shape widened does not.
    let t = by_values(Duplicates::Choose);
    assert!(plan_with(&t, &b, &ALL, &[(1, 2)], &[(1, Choice::One)]).is_ok());
    let got = plan_with(&t, &b, &Fake::broken(Broken::Widened), &[(1, 2)], &[(1, Choice::One)]);
    assert!(matches!(got, Err(PlanError::Internal(_))), "{got:?}");
}

#[test]
fn names_that_are_key_words_and_values_that_look_like_the_condition_are_planned() {
    let mut t = keyed();
    t.columns[1].as_mut().unwrap().column.name = "update".into();
    t.columns[2].as_mut().unwrap().column.name = "delete".into();
    let mut b = EditBatch::new();
    b.set(&[(1, 1)], &text("id IS NOT DISTINCT FROM '1'"), orig);
    b.add(1, None, vec![Value::Default, text("from"), Value::Null, Value::Default]);
    let p = plan_with(&t, &b, &ALL, &[], &[]).unwrap();
    assert_eq!(
        p.steps[1].sql,
        "UPDATE app.t SET update = 'id IS NOT DISTINCT FROM ''1''' WHERE id IS NOT DISTINCT FROM '1' AND update IS NOT DISTINCT FROM 'n1'"
    );
    assert_eq!(p.steps[2].sql, "INSERT INTO app.t (update, delete) VALUES ('from', NULL)");
}

#[test]
fn a_table_name_is_read_back_as_the_server_reads_it() {
    let mut b = EditBatch::new();
    b.set(&[(1, 1)], &text("x"), orig);
    for (schema, name, ok) in [("app", "t", true), ("App", "T", true), ("a.b", "it's \"q\"", true)] {
        let mut t = keyed();
        t.table = TableRef { schema: schema.into(), name: name.into() };
        assert_eq!(plan_with(&t, &b, &ALL, &[], &[]).is_ok(), ok, "{schema}.{name}");
    }
}

#[test]
fn a_row_without_all_its_values_as_read_is_never_found_by_null() {
    for t in [keyed(), by_values(Duplicates::Refuse)] {
        let mut b = EditBatch::new();
        b.set(&[(1, 1)], &text("x"), |_| vec![Some("1".into())]);
        let got = plan_with(&t, &b, &ALL, &[(1, 1)], &[]);
        assert!(matches!(got, Err(PlanError::Internal(_))), "{got:?}");
        let got = count_statements(&t, &b, &ALL, &all_whole);
        assert!(matches!(got, Ok(ref v) if v.is_empty()) || matches!(got, Err(PlanError::Internal(_))), "{got:?}");
    }
}

/// A small deterministic generator (xorshift), so the sweep needs no dependency.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

#[test]
fn every_write_of_any_batch_is_one_statement_with_a_condition_on_the_rows_identity() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let lang = Language::Sql(Dialect::Postgres);
    let values = [text("a"), text("it's"), text(""), Value::Null, Value::Default, text("\\ ; --")];
    let targets = [keyed(), by_values(Duplicates::Refuse), by_values(Duplicates::Choose)];
    let mut writes = 0;
    for round in 0..400 {
        let t = &targets[round % targets.len()];
        let mut b = EditBatch::new();
        for _ in 0..rng.below(6) {
            let row = rng.below(5) as usize;
            match rng.below(4) {
                0 => {
                    b.delete(&[row], orig);
                }
                1 => {
                    let cells = (0..4).map(|_| values[rng.below(values.len() as u64) as usize].clone()).collect();
                    b.add(row, None, cells);
                }
                _ => {
                    let col = rng.below(3) as usize;
                    b.set(&[(row, col)], &values[rng.below(values.len() as u64) as usize], |r| {
                        let mut o = orig(r);
                        o[1] = (r % 2 == 0).then(|| "it's".to_string());
                        o
                    });
                }
            }
        }
        let matches: Vec<(usize, u64)> = (0..5).map(|r| (r, 1 + rng.below(3))).collect();
        let choices: Vec<(usize, Choice)> =
            (0..5).map(|r| (r, if rng.below(2) == 0 { Choice::One } else { Choice::All })).collect();
        let Ok(p) = plan_with(t, &b, &ALL, &matches, &choices) else { continue };
        for s in &p.steps {
            let r = Classifier::classify_once(lang, &s.sql);
            if s.sql.starts_with("UPDATE") || s.sql.starts_with("DELETE") {
                writes += 1;
                assert!(r.writes && r.no_where.is_none(), "{}", s.sql);
                // The condition names the identity: the key, or each value it is found by.
                let id = if matches!(t.identity, Identity::Key { .. }) { "id IS" } else { "name IS" };
                assert!(s.sql.contains(" WHERE ") && s.sql.contains(id), "{}", s.sql);
            }
        }
    }
    assert!(writes > 200, "the sweep wrote enough statements: {writes}");
}

#[test]
fn only_the_planner_asks_a_format_for_an_update_or_a_delete_and_it_names_no_format() {
    use std::path::{Path, PathBuf};
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("edit");
    let mut files = vec![dir.with_extension("rs")];
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") && !p.ends_with("tests.rs") {
                files.push(p);
            }
        }
    }
    let callers: Vec<&PathBuf> = files
        .iter()
        .filter(|p| {
            let s = std::fs::read_to_string(p).unwrap();
            // A call of `update` or `delete` on anything but the format itself (`self`).
            [".update(", ".delete("].iter().any(|call| {
                s.match_indices(call).any(|(i, _)| {
                    let before = &s[..i];
                    let start = before.rfind(|c: char| !(c.is_alphanumeric() || c == '_')).map_or(0, |j| j + 1);
                    !before[start..].is_empty() && &before[start..] != "self"
                })
            })
        })
        .collect();
    assert_eq!(callers, [&dir.join("plan.rs")]);
    let planner = std::fs::read_to_string(dir.join("plan.rs")).unwrap();
    assert!(!planner.contains("EditFormat"), "the planner is the same for every format");
}

#[test]
fn a_new_row_leaves_no_not_null_column_without_a_default_to_its_default() {
    // `name` NOT NULL without a default; `id` NOT NULL numbered by the table, `score` NOT NULL
    // with a default.
    let mut t = keyed();
    fn col(t: &mut EditTarget, i: usize) -> &mut StructureColumn {
        &mut t.columns[i].as_mut().unwrap().column
    }
    col(&mut t, 1).not_null = true;
    col(&mut t, 0).not_null = true;
    col(&mut t, 0).fill = ColumnFill::IdentityByDefault;
    col(&mut t, 2).not_null = true;
    col(&mut t, 2).default = Some("0".into());
    let mut b = EditBatch::new();
    let given = b.add(0, None, vec![Value::Default, text("a"), Value::Default, Value::Default]);
    let left = b.add(0, None, vec![Value::Default; 4]);
    assert_eq!(no_default(&t, &b), [(left, 1)]);
    assert_eq!(plan_with(&t, &b, &ALL, &[], &[]), Err(PlanError::NoDefault { origin: Origin::New(left), column: 1 }));
    b.set_new(&[(left, 1)], &text("b"));
    assert_eq!(no_default(&t, &b), []);
    let p = plan_with(&t, &b, &ALL, &[], &[]).unwrap();
    assert_eq!(p.steps.iter().map(|s| s.origin).collect::<Vec<_>>(), [Origin::New(given), Origin::New(left)]);
    // Generated, auto-increment and identity columns fill themselves.
    for fill in [ColumnFill::Stored("1".into()), ColumnFill::AutoIncrement, ColumnFill::IdentityAlways] {
        let mut t = t.clone();
        col(&mut t, 1).fill = fill;
        let mut b = EditBatch::new();
        b.add(0, None, vec![Value::Default; 4]);
        assert_eq!(no_default(&t, &b), []);
    }
}

#[test]
fn a_new_row_is_refused_while_the_table_has_a_column_it_must_be_given_that_the_result_does_not_show() {
    let mut t = keyed();
    t.unshown_required = vec!["email".into()];
    let mut b = EditBatch::new();
    b.set(&[(0, 1)], &text("x"), orig);
    // Changes of rows read leave the table's other columns as they are.
    assert_eq!(not_shown(&t, &b), []);
    assert!(plan_with(&t, &b, &ALL, &[], &[]).is_ok());
    let id = b.add(0, None, vec![Value::Default, text("a"), text("1"), Value::Default]);
    assert_eq!(not_shown(&t, &b), [(id, "email".to_string())]);
    let refused = Err(PlanError::NotShown { origin: Origin::New(id), column: "email".into() });
    assert_eq!(plan_with(&t, &b, &ALL, &[], &[]), refused);
}

fn problems_with(
    t: &EditTarget,
    b: &EditBatch,
    f: &Fake,
    matches: &[(usize, u64)],
    choices: &[(usize, Choice)],
) -> Vec<PlanError> {
    let matches: BTreeMap<usize, u64> = matches.iter().copied().collect();
    let choices: BTreeMap<usize, Choice> = choices.iter().copied().collect();
    plan_problems(
        t,
        b,
        f,
        &Facts {
            whole: &all_whole,
            matches: &matches,
            choices: &choices,
            invalid: &BTreeSet::new(),
            invalid_for: b.revision(),
        },
    )
}

#[test]
fn every_row_s_problem_is_listed_in_the_order_the_plan_meets_them() {
    let mut t = by_values(Duplicates::Choose);
    t.columns[1].as_mut().unwrap().column.not_null = true;
    t.unshown_required = vec!["email".into()];
    let mut b = EditBatch::new();
    b.set(&[(1, 3)], &text("X"), orig);
    b.set(&[(2, 1)], &text("x"), orig);
    b.set(&[(3, 1)], &text("x"), orig);
    b.delete(&[4], orig);
    b.set(&[(5, 1)], &text("x"), orig);
    b.set(&[(6, 1)], &text("x"), orig);
    let id = b.add(0, None, vec![Value::Default, Value::Default, Value::Default, text("X")]);
    let no_one = Fake { one: false, ..ALL };
    let matches = [(2, 0), (3, 3), (4, 1), (5, 2), (6, 2)];
    let got = problems_with(&t, &b, &no_one, &matches, &[(5, Choice::One), (6, Choice::All)]);
    let new = Origin::New(id);
    assert_eq!(
        got,
        [
            PlanError::NoDefault { origin: new, column: 1 },
            PlanError::NotShown { origin: new, column: "email".into() },
            PlanError::Column { origin: Origin::Row(1), column: 3, why: ColumnReason::Computed },
            PlanError::Gone { row: 2 },
            PlanError::Choose { row: 3, matches: 3 },
            PlanError::NoOneOfK { row: 5 },
            PlanError::Column { origin: new, column: 3, why: ColumnReason::Computed },
        ],
        "rows 4 and 6 are fine"
    );
    let err = plan_with(&t, &b, &no_one, &matches, &[(5, Choice::One), (6, Choice::All)]);
    assert_eq!(err, Err(got[0].clone()), "the plan stops at the first");
    // Uncounted rows found by their values.
    assert_eq!(
        problems_with(&by_values(Duplicates::Refuse), &b, &ALL, &[(3, 2)], &[]),
        [
            PlanError::Column { origin: Origin::Row(1), column: 3, why: ColumnReason::Computed },
            PlanError::NotCounted { row: 2 },
            PlanError::Ambiguous { row: 3, matches: 2 },
            PlanError::NotCounted { row: 4 },
            PlanError::NotCounted { row: 5 },
            PlanError::NotCounted { row: 6 },
            PlanError::Column { origin: new, column: 3, why: ColumnReason::Computed },
        ]
    );
}

/// A target, a batch, a format, the rows counted and the choices.
type Case = (EditTarget, EditBatch, Fake, Vec<(usize, u64)>, Vec<(usize, Choice)>);

#[test]
fn a_batch_the_plan_takes_has_no_problem_and_one_it_refuses_has_its_error_first() {
    let mut clean = EditBatch::new();
    clean.set(&[(1, 1)], &text("x"), orig);
    clean.delete(&[2], orig);
    clean.add(0, None, vec![Value::Default, text("a"), Value::Null, Value::Default]);
    for t in [keyed(), by_values(Duplicates::Refuse), by_values(Duplicates::Choose)] {
        assert!(plan_with(&t, &clean, &ALL, &[(1, 1), (2, 1)], &[]).is_ok());
        assert_eq!(problems_with(&t, &clean, &ALL, &[(1, 1), (2, 1)], &[]), []);
    }
    assert_eq!(problems_with(&keyed(), &EditBatch::new(), &ALL, &[], &[]), []);
    // Each refusal alone: the plan errs exactly when there are problems, with the first.
    let mut cases: Vec<Case> = Vec::new();
    let mut b = EditBatch::new();
    b.set(&[(3, 1)], &text("x"), orig);
    for (m, c) in
        [(vec![], vec![]), (vec![(3, 0)], vec![]), (vec![(3, 2)], vec![]), (vec![(3, 2)], vec![(3, Choice::One)])]
    {
        cases.push((by_values(Duplicates::Choose), b.clone(), Fake { one: false, ..ALL }, m.clone(), c.clone()));
        cases.push((by_values(Duplicates::Refuse), b.clone(), ALL, m, c));
    }
    let mut computed = EditBatch::new();
    computed.set(&[(1, 3)], &text("X"), orig);
    cases.push((keyed(), computed, ALL, vec![], vec![]));
    for (t, b, f, m, c) in &cases {
        let plan = plan_with(t, b, f, m, c);
        let problems = problems_with(t, b, f, m, c);
        assert_eq!(plan.is_err(), !problems.is_empty(), "{plan:?} {problems:?}");
        if let Err(e) = plan {
            assert_eq!(problems.first(), Some(&e));
        }
    }
    // A key value not shown whole: that row's problem, the other row is fine.
    let mut cut = EditBatch::new();
    cut.set(&[(5, 1)], &text("x"), orig);
    cut.set(&[(6, 1)], &text("x"), orig);
    let whole = |row: usize, col: usize| !(row == 5 && col == 0);
    let none = BTreeMap::new();
    let facts = Facts {
        whole: &whole,
        matches: &none,
        choices: &BTreeMap::new(),
        invalid: &BTreeSet::new(),
        invalid_for: cut.revision(),
    };
    let not_whole = PlanError::NotWhole { row: 5, column: 0 };
    assert_eq!(plan(&keyed(), &cut, &ALL, &facts).err(), Some(not_whole.clone()));
    assert_eq!(plan_problems(&keyed(), &cut, &ALL, &facts), [not_whole]);
}

#[test]
fn a_cell_known_invalid_stops_the_plan_before_any_statement() {
    let mut b = EditBatch::new();
    b.set(&[(4, 1)], &text("ok"), orig);
    b.set(&[(5, 0)], &text("abc"), orig);
    let n = b.add(usize::MAX, None, vec![Value::Default, text("x"), text("not a number"), Value::Default]);
    let none = BTreeMap::new();
    let with = |invalid: &BTreeSet<(Origin, usize)>| {
        let facts =
            Facts { whole: &all_whole, matches: &none, choices: &none_chosen(), invalid, invalid_for: b.revision() };
        (plan(&keyed(), &b, &ALL, &facts), plan_problems(&keyed(), &b, &ALL, &facts))
    };
    let (p, problems) = with(&BTreeSet::new());
    assert!(p.is_ok() && problems.is_empty());
    let invalid = BTreeSet::from([(Origin::New(n), 2), (Origin::Row(5), 0)]);
    let (p, problems) = with(&invalid);
    let row = PlanError::Invalid { origin: Origin::Row(5), column: 0 };
    let new = PlanError::Invalid { origin: Origin::New(n), column: 2 };
    assert_eq!(p, Err(row.clone()), "no steps");
    assert_eq!(problems, [row, new], "every invalid cell");
    // A cell no longer set (or of a row to be deleted, of a new row gone, or of a new row left
    // to its default) is not the plan's.
    let stale =
        BTreeSet::from([(Origin::Row(9), 1), (Origin::Row(4), 0), (Origin::New(n + 1), 0), (Origin::New(n), 0)]);
    assert!(with(&stale).0.is_ok());
}

#[test]
fn invalid_cells_found_for_another_revision_stop_the_plan() {
    let mut b = EditBatch::new();
    b.set(&[(4, 1)], &text("ok"), orig);
    let found_for = b.revision();
    // A cell set since: whether it is invalid is not known.
    b.set(&[(5, 1)], &text("not checked"), orig);
    let none = BTreeMap::new();
    let invalid = BTreeSet::new();
    let facts =
        Facts { whole: &all_whole, matches: &none, choices: &none_chosen(), invalid: &invalid, invalid_for: found_for };
    let p = plan(&keyed(), &b, &ALL, &facts);
    assert!(matches!(p, Err(PlanError::Internal(_))), "{p:?}");
    let problems = plan_problems(&keyed(), &b, &ALL, &facts);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(Some(&p.unwrap_err()), problems.first());
    let facts = Facts { invalid_for: b.revision(), ..facts };
    assert!(plan(&keyed(), &b, &ALL, &facts).is_ok());
}

fn none_chosen() -> BTreeMap<usize, Choice> {
    BTreeMap::new()
}

/// Whatever the batch and the invalid cells, a plan made while any cell it sets is invalid has no
/// statement: it is refused.
#[test]
fn no_plan_sends_an_invalid_value() {
    let none = BTreeMap::new();
    let mut seed = 11u64;
    let mut pick = |n: u64| {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) % n
    };
    for _ in 0..300 {
        let mut b = EditBatch::new();
        // Each value names its cell, so a statement that sends it shows.
        for _ in 0..pick(4) {
            let (r, c) = (pick(6) as usize, 1 + pick(2) as usize);
            b.set(&[(r, c)], &text(&format!("r{r}c{c}z")), orig);
        }
        // New rows are numbered from 0.
        for id in 0..pick(3) {
            let score = if pick(2) == 0 { Value::Default } else { text(&format!("d{id}c2z")) };
            let n = b.add(usize::MAX, None, vec![Value::Default, text(&format!("d{id}c1z")), score, Value::Default]);
            assert_eq!(n, id);
        }
        if pick(3) == 0 {
            b.delete(&[pick(6) as usize], orig);
        }
        let invalid: BTreeSet<(Origin, usize)> = (0..pick(4))
            .map(|_| {
                let o = if pick(2) == 0 { Origin::Row(pick(6) as usize) } else { Origin::New(pick(3)) };
                (o, pick(4) as usize)
            })
            .collect();
        let touched = invalid.iter().any(|&(o, c)| match o {
            Origin::Row(r) => b.value(r, c).is_some(),
            Origin::New(id) => {
                b.drafts().iter().any(|d| d.id == id && d.cells.get(c).is_some_and(|v| *v != Value::Default))
            }
        });
        let facts = Facts {
            whole: &all_whole,
            matches: &none,
            choices: &none_chosen(),
            invalid: &invalid,
            invalid_for: b.revision(),
        };
        let p = plan(&keyed(), &b, &ALL, &facts);
        if touched {
            assert!(matches!(p, Err(PlanError::Invalid { .. })), "{p:?}");
        } else {
            assert!(!matches!(p, Err(PlanError::Invalid { .. })), "{p:?}");
        }
        // On what is made, not how it is decided: no statement sends an invalid cell's value.
        if let Ok(p) = &p {
            for &(o, c) in &invalid {
                let value = match o {
                    Origin::Row(r) => format!("r{r}c{c}z"),
                    Origin::New(id) => format!("d{id}c{c}z"),
                };
                assert!(p.steps.iter().all(|s| !s.sql.contains(&value)), "{value} sent: {p:?}");
            }
        }
    }
}
