//! The risk classifier of SQLite text (`Dialect::Sqlite`): what a statement may do, read from
//! its keywords. An allowlist: a statement it does not know is `Class::Unknown`, which a
//! read-only policy refuses and a confirmation asks about.
//!
//! SQLite has no roles and no server to enforce anything: the engine's own read-only open
//! (`SQLITE_OPEN_READ_ONLY`) and the driver's authorizer (which refuses `ATTACH` and `DETACH`)
//! are the guarantee. This classifier only decides what the app asks before it runs.

use super::{Class, Danger, Explain, NoWhere, Risk};
use crate::sql::dialect::Dialect;
use crate::sql::lexer::{Tok, lex_in};
use crate::sql::split::split_in;

/// The PRAGMAs that only read: `PRAGMA name` and `PRAGMA name(arg)` with no `=`. Any other
/// PRAGMA may change a setting, so it is a session statement a read-only policy refuses.
const READ_PRAGMAS: &[&str] = &[
    "COMPILE_OPTIONS",
    "COLLATION_LIST",
    "DATABASE_LIST",
    "FOREIGN_KEY_LIST",
    "FUNCTION_LIST",
    "INDEX_INFO",
    "INDEX_LIST",
    "INDEX_XINFO",
    "INTEGRITY_CHECK",
    "MODULE_LIST",
    "PAGE_COUNT",
    "PAGE_SIZE",
    "PRAGMA_LIST",
    "QUICK_CHECK",
    "SCHEMA_VERSION",
    "TABLE_INFO",
    "TABLE_LIST",
    "TABLE_XINFO",
    "USER_VERSION",
];

/// The PRAGMA that lets a statement change the schema file itself.
const DANGEROUS_PRAGMA: &str = "WRITABLE_SCHEMA";

/// The risk of `sql`: every statement of it, the worst one counts.
pub fn classify(sql: &str) -> Risk {
    split_in(sql, Dialect::Sqlite)
        .iter()
        .map(|statement| statement_risk(statement.body(sql)))
        .reduce(Risk::merge)
        .unwrap_or_else(|| risk_of(Class::Unknown))
}

/// The risk of one statement, read from its first keyword.
fn statement_risk(body: &str) -> Risk {
    let (words, has_eq) = keywords(body);
    let Some(first) = words.first().map(String::as_str) else {
        return risk_of(Class::Unknown);
    };
    match first {
        "SELECT" | "VALUES" | "EXPLAIN" => risk_of(Class::Read),
        "WITH" if words.iter().any(|w| matches!(w.as_str(), "INSERT" | "UPDATE" | "DELETE" | "REPLACE")) => {
            write_risk(&words)
        }
        "WITH" => risk_of(Class::Read),
        "PRAGMA" => pragma_risk(&words, has_eq),
        "INSERT" | "REPLACE" => write_risk(&words),
        "UPDATE" | "DELETE" => write_risk(&words),
        "CREATE" => risk_of(Class::Ddl),
        "ALTER" => alter_risk(&words),
        "DROP" => Risk { danger: Some(Danger::Drop), ..risk_of(Class::Ddl) },
        "VACUUM" | "REINDEX" | "ANALYZE" => risk_of(Class::Maintenance),
        "BEGIN" | "COMMIT" | "END" | "ROLLBACK" | "SAVEPOINT" | "RELEASE" => risk_of(Class::Tx),
        "ATTACH" | "DETACH" => Risk { danger: Some(Danger::FileAccess), ..risk_of(Class::Unknown) },
        _ => risk_of(Class::Unknown),
    }
}

/// `INSERT`, `REPLACE`, `UPDATE` and `DELETE`: they change rows. An `UPDATE` or `DELETE`
/// without a `WHERE` touches every row, which the confirmation names.
fn write_risk(words: &[String]) -> Risk {
    let has_where = words.iter().any(|w| w == "WHERE");
    let first = words.first().map(String::as_str);
    let (danger, no_where) = match (first, has_where) {
        (Some("DELETE"), false) => (Some(Danger::DeleteAll), Some(NoWhere::Missing)),
        (Some("UPDATE"), false) => (Some(Danger::UpdateAll), Some(NoWhere::Missing)),
        _ => (None, None),
    };
    Risk { writes: true, danger, no_where, ..risk_of(Class::Write) }
}

/// `ALTER TABLE`: a `DROP COLUMN` removes data; the other forms only rename or add.
fn alter_risk(words: &[String]) -> Risk {
    let drops = words.iter().any(|w| w == "DROP");
    let danger = drops.then_some(Danger::DropColumn);
    Risk { danger, ..risk_of(Class::Ddl) }
}

/// A PRAGMA: a read of a value when it is one of [`READ_PRAGMAS`] without `=`, else a session
/// statement that may change a setting (refused by a read-only policy).
fn pragma_risk(words: &[String], has_eq: bool) -> Risk {
    // `PRAGMA [schema.]name[(arg)|= value]`: the name is the last word before `(` or `=`.
    let name = words[1..].iter().take_while(|w| *w != "(" && *w != "=").filter(|w| *w != ".").last();
    let name = name.map(String::as_str).unwrap_or("");
    if name == DANGEROUS_PRAGMA {
        return Risk { danger: Some(Danger::Setting), ..risk_of(Class::Session) };
    }
    if !has_eq && READ_PRAGMAS.contains(&name) {
        return risk_of(Class::Read);
    }
    risk_of(Class::Session)
}

/// The keywords and names of a statement, upper case, in order (with `(`, `.` and `=` where they
/// stand), and whether it has an `=` (outside strings and comments). Quoted names, strings and
/// comments are not keywords.
fn keywords(body: &str) -> (Vec<String>, bool) {
    let mut words = Vec::new();
    let mut has_eq = false;
    for token in lex_in(body, Dialect::Sqlite) {
        let text = token.text(body);
        match token.kind {
            Tok::Keyword | Tok::Ident => words.push(text.to_ascii_uppercase()),
            Tok::Op if text == "=" => {
                has_eq = true;
                words.push("=".to_string());
            }
            Tok::Dot => words.push(".".to_string()),
            Tok::LParen => words.push("(".to_string()),
            _ => {}
        }
    }
    (words, has_eq)
}

/// A risk of class `class` with nothing else known.
fn risk_of(class: Class) -> Risk {
    Risk {
        class,
        explain: Explain::No,
        writes: false,
        no_where: None,
        danger: None,
        target: None,
        safe_setting: false,
        read_write: false,
        stdio: false,
        runs_code: false,
        session_path: false,
        implicit_commit: false,
        unchecked_call: false,
        no_tx_block: false,
        adds_enum_value: false,
    }
}

#[cfg(test)]
mod tests;
