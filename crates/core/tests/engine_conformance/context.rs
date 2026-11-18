//! A function call judged in each place the app judges one: run in the editor (the confirm and
//! the read-only verdict, with the default read-only context and with one that allows a
//! function, and the hint a refusal gives), run again (`repeatable`: a re-run, a count) and
//! inside a table's filter (on a read-write and on a read-only profile).

use crate::Engine::{MariaDb, MySql, Pg};
use crate::{Case, Engine, verdict};
use datarig_core::query::{ColumnFacts, ColumnKind, TableName, Target, check_filter};
use datarig_core::sql::dialect::Language;
use datarig_core::sql::risk::{AllowedFunction, Classifier, ReadOnlyContext};

/// The verdicts on expression `expr` in each context, on `e`: `SELECT <expr> FROM t` in the
/// editor (read-only with the default context, then with one whose environment allows
/// `my_function`, and the hint the default context gives for the call it refuses) and run
/// again, and `<expr> IS NOT NULL` as the filter of table `t`.
pub fn verdicts(e: Engine, expr: &&'static str) -> String {
    let d = e.dialect();
    let mut classifier = Classifier::new(Language::Sql(d));
    let sql = format!("SELECT {expr} FROM t");
    let risk = classifier.classify(&sql);
    let default = ReadOnlyContext::default();
    let allowing = ReadOnlyContext {
        allowed: vec![AllowedFunction::parse("my_function").expect("an allowed function entry")],
        ..ReadOnlyContext::default()
    };
    let hint = risk.refused_call_name(&default).map(|name| default.hint(name));
    let table = TableName { database: None, schema: "s".to_string(), name: "t".to_string() };
    let target =
        Target::new(d, vec![ColumnFacts::new("id", ColumnKind::Number), ColumnFacts::new("a", ColumnKind::Text)]);
    let filter = format!("{expr} IS NOT NULL");
    let filter_rw = check_filter(&filter, &table, &target, &classifier, false);
    let filter_ro = check_filter(&filter, &table, &target, &classifier, true);
    format!(
        "run {:?} | read-only {} | read-only allowing my_function {} | hint {:?} | re-run {} | filter {} \
         | read-only filter {}",
        risk.confirm(false),
        verdict(&risk.read_only_in(&default)),
        verdict(&risk.read_only_in(&allowing)),
        hint,
        verdict(&classifier.repeatable(&sql)),
        filter_verdict(&filter_rw),
        filter_verdict(&filter_ro),
    )
}

fn filter_verdict<T: std::fmt::Debug, E: std::fmt::Debug>(r: &Result<T, E>) -> String {
    match r {
        Ok(v) => format!("{v:?}"),
        Err(e) => format!("refused {e:?}"),
    }
}

pub fn cases() -> Vec<Case<&'static str>> {
    vec![
        Case {
            name: "row_count",
            input: "ROW_COUNT()",
            expect: &[
                (
                    Pg,
                    "run None | read-only UnknownFunction(\"row_count\") | read-only allowing my_function UnknownFunction(\"row_count\") | hint Some(NotAllowed) | re-run UserFunction | filter NeedsConfirm([Calls([\"row_count\"])]) | read-only filter refused UncheckedCalls([\"row_count\"])",
                ),
                (
                    MySql,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run Volatile(\"row_count\") | filter refused RefusedCalls([\"ROW_COUNT\"]) | read-only filter refused RefusedCalls([\"ROW_COUNT\"])",
                ),
                (
                    MariaDb,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run Volatile(\"row_count\") | filter refused RefusedCalls([\"ROW_COUNT\"]) | read-only filter refused RefusedCalls([\"ROW_COUNT\"])",
                ),
            ],
        },
        Case {
            name: "found_rows",
            input: "FOUND_ROWS()",
            expect: &[
                (
                    Pg,
                    "run None | read-only UnknownFunction(\"found_rows\") | read-only allowing my_function UnknownFunction(\"found_rows\") | hint Some(NotAllowed) | re-run UserFunction | filter NeedsConfirm([Calls([\"found_rows\"])]) | read-only filter refused UncheckedCalls([\"found_rows\"])",
                ),
                (
                    MySql,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run Volatile(\"found_rows\") | filter refused RefusedCalls([\"FOUND_ROWS\"]) | read-only filter refused RefusedCalls([\"FOUND_ROWS\"])",
                ),
                (
                    MariaDb,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run Volatile(\"found_rows\") | filter refused RefusedCalls([\"FOUND_ROWS\"]) | read-only filter refused RefusedCalls([\"FOUND_ROWS\"])",
                ),
            ],
        },
        Case {
            name: "last_insert_id",
            input: "LAST_INSERT_ID()",
            expect: &[
                (
                    Pg,
                    "run None | read-only UnknownFunction(\"last_insert_id\") | read-only allowing my_function UnknownFunction(\"last_insert_id\") | hint Some(NotAllowed) | re-run UserFunction | filter NeedsConfirm([Calls([\"last_insert_id\"])]) | read-only filter refused UncheckedCalls([\"last_insert_id\"])",
                ),
                (
                    MySql,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run Volatile(\"last_insert_id\") | filter NeedsConfirm([Calls([\"LAST_INSERT_ID\"])]) | read-only filter refused UncheckedCalls([\"LAST_INSERT_ID\"])",
                ),
                (
                    MariaDb,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run Volatile(\"last_insert_id\") | filter NeedsConfirm([Calls([\"LAST_INSERT_ID\"])]) | read-only filter refused UncheckedCalls([\"LAST_INSERT_ID\"])",
                ),
            ],
        },
        Case {
            name: "now",
            input: "NOW()",
            expect: &[
                (
                    Pg,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run ok | filter Clear | read-only filter Clear",
                ),
                (
                    MySql,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run ok | filter Clear | read-only filter Clear",
                ),
                (
                    MariaDb,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run ok | filter Clear | read-only filter Clear",
                ),
            ],
        },
        Case {
            name: "upper",
            input: "UPPER(a)",
            expect: &[
                (
                    Pg,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run ok | filter Clear | read-only filter Clear",
                ),
                (
                    MySql,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run ok | filter Clear | read-only filter Clear",
                ),
                (
                    MariaDb,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run ok | filter Clear | read-only filter Clear",
                ),
            ],
        },
        Case {
            name: "rand",
            input: "RAND()",
            expect: &[
                (
                    Pg,
                    "run None | read-only UnknownFunction(\"rand\") | read-only allowing my_function UnknownFunction(\"rand\") | hint Some(NotAllowed) | re-run UserFunction | filter NeedsConfirm([Calls([\"rand\"])]) | read-only filter refused UncheckedCalls([\"rand\"])",
                ),
                (
                    MySql,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run Volatile(\"rand\") | filter NeedsConfirm([Calls([\"RAND\"])]) | read-only filter refused UncheckedCalls([\"RAND\"])",
                ),
                (
                    MariaDb,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run Volatile(\"rand\") | filter NeedsConfirm([Calls([\"RAND\"])]) | read-only filter refused UncheckedCalls([\"RAND\"])",
                ),
            ],
        },
        Case {
            name: "random",
            input: "random()",
            expect: &[
                (
                    Pg,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run Volatile(\"random\") | filter NeedsConfirm([Calls([\"random\"])]) | read-only filter refused UncheckedCalls([\"random\"])",
                ),
                (
                    MySql,
                    "run None | read-only UnknownFunction(\"random\") | read-only allowing my_function UnknownFunction(\"random\") | hint None | re-run UserFunction | filter NeedsConfirm([Calls([\"RANDOM\"])]) | read-only filter refused UncheckedCalls([\"RANDOM\"])",
                ),
                (
                    MariaDb,
                    "run None | read-only UnknownFunction(\"random\") | read-only allowing my_function UnknownFunction(\"random\") | hint None | re-run UserFunction | filter NeedsConfirm([Calls([\"RANDOM\"])]) | read-only filter refused UncheckedCalls([\"RANDOM\"])",
                ),
            ],
        },
        Case {
            name: "sleep",
            input: "SLEEP(1)",
            expect: &[
                (
                    Pg,
                    "run None | read-only UnknownFunction(\"sleep\") | read-only allowing my_function UnknownFunction(\"sleep\") | hint Some(NotAllowed) | re-run UserFunction | filter NeedsConfirm([Calls([\"sleep\"])]) | read-only filter refused UncheckedCalls([\"sleep\"])",
                ),
                (
                    MySql,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run Volatile(\"sleep\") | filter refused RefusedCalls([\"SLEEP\"]) | read-only filter refused RefusedCalls([\"SLEEP\"])",
                ),
                (
                    MariaDb,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run Volatile(\"sleep\") | filter refused RefusedCalls([\"SLEEP\"]) | read-only filter refused RefusedCalls([\"SLEEP\"])",
                ),
            ],
        },
        Case {
            name: "pg_sleep",
            input: "pg_sleep(1)",
            expect: &[
                (
                    Pg,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run Volatile(\"pg_sleep\") | filter refused RefusedCalls([\"pg_sleep\"]) | read-only filter refused RefusedCalls([\"pg_sleep\"])",
                ),
                (
                    MySql,
                    "run None | read-only UnknownFunction(\"pg_sleep\") | read-only allowing my_function UnknownFunction(\"pg_sleep\") | hint None | re-run UserFunction | filter NeedsConfirm([Calls([\"PG_SLEEP\"])]) | read-only filter refused UncheckedCalls([\"PG_SLEEP\"])",
                ),
                (
                    MariaDb,
                    "run None | read-only UnknownFunction(\"pg_sleep\") | read-only allowing my_function UnknownFunction(\"pg_sleep\") | hint None | re-run UserFunction | filter NeedsConfirm([Calls([\"PG_SLEEP\"])]) | read-only filter refused UncheckedCalls([\"PG_SLEEP\"])",
                ),
            ],
        },
        Case {
            name: "bin_to_uuid",
            input: "BIN_TO_UUID(a)",
            expect: &[
                (
                    Pg,
                    "run None | read-only UnknownFunction(\"bin_to_uuid\") | read-only allowing my_function UnknownFunction(\"bin_to_uuid\") | hint Some(NotAllowed) | re-run UserFunction | filter NeedsConfirm([Calls([\"bin_to_uuid\"])]) | read-only filter refused UncheckedCalls([\"bin_to_uuid\"])",
                ),
                (
                    MySql,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run ok | filter NeedsConfirm([Calls([\"BIN_TO_UUID\"])]) | read-only filter refused UncheckedCalls([\"BIN_TO_UUID\"])",
                ),
                (
                    MariaDb,
                    "run None | read-only UnknownFunction(\"BIN_TO_UUID\") | read-only allowing my_function UnknownFunction(\"BIN_TO_UUID\") | hint None | re-run UserFunction | filter NeedsConfirm([Calls([\"BIN_TO_UUID\"])]) | read-only filter refused UncheckedCalls([\"BIN_TO_UUID\"])",
                ),
            ],
        },
        Case {
            name: "nextval",
            input: "NEXTVAL(sq)",
            expect: &[
                (
                    Pg,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run Volatile(\"nextval\") | filter refused RefusedCalls([\"nextval\"]) | read-only filter refused RefusedCalls([\"nextval\"])",
                ),
                (
                    MySql,
                    "run None | read-only UnknownFunction(\"NEXTVAL\") | read-only allowing my_function UnknownFunction(\"NEXTVAL\") | hint None | re-run UserFunction | filter refused RefusedCalls([\"NEXTVAL\"]) | read-only filter refused RefusedCalls([\"NEXTVAL\"])",
                ),
                (
                    MariaDb,
                    "run None | read-only UnknownFunction(\"NEXTVAL\") | read-only allowing my_function UnknownFunction(\"NEXTVAL\") | hint None | re-run UserFunction | filter refused RefusedCalls([\"NEXTVAL\"]) | read-only filter refused RefusedCalls([\"NEXTVAL\"])",
                ),
            ],
        },
        Case {
            name: "pg_catalog.uuidv7",
            input: "pg_catalog.uuidv7()",
            expect: &[
                (
                    Pg,
                    "run None | read-only ok | read-only allowing my_function ok | hint None | re-run UserFunction | filter NeedsConfirm([Calls([\"pg_catalog.uuidv7\"])]) | read-only filter refused UncheckedCalls([\"pg_catalog.uuidv7\"])",
                ),
                (
                    MySql,
                    "run None | read-only UnknownFunction(\"pg_catalog.uuidv7\") | read-only allowing my_function UnknownFunction(\"pg_catalog.uuidv7\") | hint None | re-run UserFunction | filter NeedsConfirm([Calls([\"UUIDV7\"])]) | read-only filter refused UncheckedCalls([\"UUIDV7\"])",
                ),
                (
                    MariaDb,
                    "run None | read-only UnknownFunction(\"pg_catalog.uuidv7\") | read-only allowing my_function UnknownFunction(\"pg_catalog.uuidv7\") | hint None | re-run UserFunction | filter NeedsConfirm([Calls([\"UUIDV7\"])]) | read-only filter refused UncheckedCalls([\"UUIDV7\"])",
                ),
            ],
        },
        Case {
            name: "uuidv7",
            input: "uuidv7()",
            expect: &[
                (
                    Pg,
                    "run None | read-only UnknownFunction(\"uuidv7\") | read-only allowing my_function UnknownFunction(\"uuidv7\") | hint Some(NewerBuiltin) | re-run UserFunction | filter NeedsConfirm([Calls([\"uuidv7\"])]) | read-only filter refused UncheckedCalls([\"uuidv7\"])",
                ),
                (
                    MySql,
                    "run None | read-only UnknownFunction(\"uuidv7\") | read-only allowing my_function UnknownFunction(\"uuidv7\") | hint None | re-run UserFunction | filter NeedsConfirm([Calls([\"UUIDV7\"])]) | read-only filter refused UncheckedCalls([\"UUIDV7\"])",
                ),
                (
                    MariaDb,
                    "run None | read-only UnknownFunction(\"uuidv7\") | read-only allowing my_function UnknownFunction(\"uuidv7\") | hint None | re-run UserFunction | filter NeedsConfirm([Calls([\"UUIDV7\"])]) | read-only filter refused UncheckedCalls([\"UUIDV7\"])",
                ),
            ],
        },
        Case {
            name: "allowed function qualified",
            input: "public.my_function(a)",
            expect: &[
                (
                    Pg,
                    "run None | read-only UnknownFunction(\"public.my_function\") | read-only allowing my_function ok | hint Some(NotAllowed) | re-run UserFunction | filter NeedsConfirm([Calls([\"public.my_function\"])]) | read-only filter refused UncheckedCalls([\"public.my_function\"])",
                ),
                (
                    MySql,
                    "run None | read-only UnknownFunction(\"public.my_function\") | read-only allowing my_function UnknownFunction(\"public.my_function\") | hint None | re-run UserFunction | filter NeedsConfirm([Calls([\"MY_FUNCTION\"])]) | read-only filter refused UncheckedCalls([\"MY_FUNCTION\"])",
                ),
                (
                    MariaDb,
                    "run None | read-only UnknownFunction(\"public.my_function\") | read-only allowing my_function UnknownFunction(\"public.my_function\") | hint None | re-run UserFunction | filter NeedsConfirm([Calls([\"MY_FUNCTION\"])]) | read-only filter refused UncheckedCalls([\"MY_FUNCTION\"])",
                ),
            ],
        },
        Case {
            name: "user function",
            input: "my_function(a)",
            expect: &[
                (
                    Pg,
                    "run None | read-only UnknownFunction(\"my_function\") | read-only allowing my_function ok | hint Some(NotAllowed) | re-run UserFunction | filter NeedsConfirm([Calls([\"my_function\"])]) | read-only filter refused UncheckedCalls([\"my_function\"])",
                ),
                (
                    MySql,
                    "run None | read-only UnknownFunction(\"my_function\") | read-only allowing my_function UnknownFunction(\"my_function\") | hint None | re-run UserFunction | filter NeedsConfirm([Calls([\"MY_FUNCTION\"])]) | read-only filter refused UncheckedCalls([\"MY_FUNCTION\"])",
                ),
                (
                    MariaDb,
                    "run None | read-only UnknownFunction(\"my_function\") | read-only allowing my_function UnknownFunction(\"my_function\") | hint None | re-run UserFunction | filter NeedsConfirm([Calls([\"MY_FUNCTION\"])]) | read-only filter refused UncheckedCalls([\"MY_FUNCTION\"])",
                ),
            ],
        },
    ]
}
