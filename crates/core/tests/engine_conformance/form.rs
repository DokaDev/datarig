//! What each engine fixes before a session answered: the language a session starts in before it
//! says its mode (`EngineSpec::language`) and the engine-only fields of the profile form
//! (`EngineSpec::form`). No answer depends on the server's version.

use crate::Case;
use crate::Engine::{self, MariaDb, MySql, Pg};

/// `EngineSpec::language` of engine `e`.
pub fn language(e: Engine, _: &()) -> String {
    format!("{:?}", e.dialect().engine().spec().language)
}

/// `EngineSpec::form` of engine `e`.
pub fn form(e: Engine, _: &()) -> String {
    format!("{:?}", e.dialect().engine().spec().form)
}

pub fn language_cases() -> Vec<Case<()>> {
    vec![Case {
        name: "a session's language before it says its mode",
        input: (),
        expect: &[
            (Pg, "Sql(Postgres)"),
            (
                MySql,
                "Sql(MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: NotRead, ignore_space: false }))",
            ),
            (
                MariaDb,
                "Sql(MariaDb(MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: NotRead, ignore_space: false }))",
            ),
        ],
    }]
}

pub fn form_cases() -> Vec<Case<()>> {
    vec![Case {
        name: "the profile form's engine-only fields",
        input: (),
        expect: &[
            (Pg, "FormFields { statement_cache: true, server_key: false }"),
            (MySql, "FormFields { statement_cache: false, server_key: true }"),
            (MariaDb, "FormFields { statement_cache: false, server_key: true }"),
        ],
    }]
}
