//! The checks filter text passes before any statement built from it runs ([`check_filter`]).
//!
//! 1. **Lexical**, with the dialect's lexer: no `;` (nor MySQL's `\g`), no comment of any form
//!    (`--`, `/* */`, MySQL's `#`; MySQL's executable comments `/*! */` and `/*M! */` named
//!    apart), no client command, every parenthesis closed and none closing one it did not open
//!    (the text would leave the parentheses it is put in), no string or quoted name left open.
//!    A text the server may read otherwise is read every way it may be: PostgreSQL's plain
//!    strings with a backslash as `standard_conforming_strings = off` reads them, MySQL's text
//!    in the other sql modes that read it differently. Another reading that leaves a string or
//!    a parenthesis open is a syntax error on the server and is dropped; one that is refused
//!    otherwise refuses the text, and one whose tokens end elsewhere is ambiguous.
//! 2. **Shape** of the compiled statement. PostgreSQL, on its parse tree: one `SELECT` of our
//!    table's `*`, no set operation, no `INTO` and no row lock anywhere, no clause but the
//!    `WHERE`. MySQL, on the text's tokens: no `INTO`, `LOCK`, `PROCEDURE` or `FOR UPDATE`/`FOR
//!    SHARE` anywhere, no `UNION`, `INTERSECT`, `EXCEPT`, `FOR`, `LIMIT`, `ORDER`, `GROUP`,
//!    `HAVING` or `WINDOW` outside the text's own parentheses, no assignment (`:=`), and no
//!    `NEXT VALUE FOR` nor `s.nextval` (MariaDB's, which advance a sequence for each row read).
//! 3. **Classifier**: the whole statement is a plain read with no danger
//!    ([`crate::sql::risk`]); a text it cannot read or that is too complex is refused.
//! 4. **Function calls**: only the built-ins of [`PG_FILTER_CALLS`] or [`MYSQL_FILTER_CALLS`]
//!    pass without a word (by a bare name, or PostgreSQL's `pg_catalog.` one). A built-in of
//!    [`PG_REFUSED_CALLS`] or [`MYSQL_REFUSED_CALLS`] (it waits, locks, changes the session,
//!    calls out, reads or writes the server's files or large objects, or reads the outcome of
//!    the statement before it) is refused. Any other call asks the user
//!    ([`FilterVerdict::NeedsConfirm`]), and is refused on a read-only profile: a function may do
//!    on the server what a read-only transaction does not stop.
//!
//! A refused built-in is refused in attribute notation too (`(oid).lo_get`, `t.lo_unlink`),
//! where any later name of a column reference or an indirection counts, even a column's.
//! What is not seen, as for the editor's classifier: a function of the user's called in
//! attribute notation (`t.f`), a user-defined operator, and code a view or a policy runs.

use super::{Filter, Query, Source, TableName, Target, compile};
use crate::sql::dialect::{Dialect, MySqlMode};
use crate::sql::lexer::{Tok, Token, lex_backslash_strings, lex_in};
use crate::sql::risk::{self, Class, Classifier, Danger, ReadOnlyBlock};
use serde_json::Value;
use std::ops::Range;

/// What the user must say before a filter runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilterVerdict {
    /// It may run.
    Clear,
    /// It may run once the user confirms each of these.
    NeedsConfirm(Vec<Confirm>),
}

/// Something a filter does that the user confirms before it runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirm {
    /// It calls functions whose effects are not known, as written: they run on the server for
    /// every row.
    Calls(Vec<String>),
}

/// Why the filter bar refuses a text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BarRefusal {
    /// There is no condition, only blanks.
    Empty,
    /// A token the filter may not hold, at a byte range of the text.
    Lexical { problem: LexProblem, at: Range<usize> },
    /// The statement is not one read of the table with that filter; with a byte range of the
    /// text when known.
    Shape { problem: ShapeProblem, at: Option<Range<usize>> },
    /// The classifier does not read the statement as a plain read: its class and danger (a text
    /// it cannot read is [`Danger::Unparsed`], or [`Danger::Unrecognized`] on MySQL).
    NotRead { class: Class, danger: Option<Danger> },
    /// It calls built-ins that wait, lock, change the session or call out (as written).
    RefusedCalls(Vec<String>),
    /// On a read-only profile: it calls functions whose effects are not known (as written).
    UncheckedCalls(Vec<String>),
    /// A read-only profile refuses the statement.
    ReadOnly(ReadOnlyBlock),
}

/// A token of the text the filter bar refuses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LexProblem {
    /// A statement terminator (`;`, MySQL's `\g`).
    Semicolon,
    /// A comment, which may hide text from the line the user sees.
    Comment,
    /// MySQL: `/*! … */` or `/*M! … */`, whose content the server runs.
    ExecutableComment,
    /// MySQL: a client command (`DELIMITER`).
    ClientCommand,
    /// A parenthesis that does not close.
    Unclosed,
    /// A parenthesis that closes one the text did not open: it would leave the parentheses the
    /// text is put in.
    ClosesOutside,
    /// A string or a quoted name that does not end.
    Unterminated,
    /// The text reads as other tokens under a server setting (a backslash in a string):
    /// what runs depends on it.
    Ambiguous,
}

/// How the compiled statement is not one read of the table with that filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeProblem {
    /// Not exactly one `SELECT`.
    NotOneSelect,
    /// It reads another table or other columns than the table's `*`.
    Source,
    /// A `UNION`, `INTERSECT` or `EXCEPT`.
    SetOperation,
    /// A clause besides the `WHERE` (`ORDER BY`, `LIMIT`, `GROUP BY`, `HAVING`, `WINDOW`, …).
    Clause,
    /// A row lock (`FOR UPDATE`, `FOR SHARE`, `LOCK IN SHARE MODE`), at any depth.
    Locking,
    /// `INTO`, at any depth.
    Into,
    /// MySQL: a variable assignment (`:=`).
    Assignment,
    /// MariaDB: `NEXT VALUE FOR` or `s.nextval` (the ORACLE sql mode's spelling), which
    /// advance a sequence for each row read. Refused whatever the session's sql mode: a name
    /// `nextval` after a `.` counts, even a column's.
    Sequence,
}

/// PostgreSQL built-ins a filter may call without asking, by name: they read their arguments
/// and nothing else. Sorted.
pub const PG_FILTER_CALLS: &[&str] = &[
    "abs",
    "age",
    "array_length",
    "btrim",
    "cardinality",
    "ceil",
    "ceiling",
    "char_length",
    "character_length",
    "concat",
    "concat_ws",
    "date_part",
    "date_trunc",
    "extract",
    "floor",
    "initcap",
    "isfinite",
    "json_array_length",
    "json_extract_path",
    "json_extract_path_text",
    "json_typeof",
    "jsonb_array_length",
    "jsonb_extract_path",
    "jsonb_extract_path_text",
    "jsonb_typeof",
    "left",
    "length",
    "like_escape",
    "lower",
    "lpad",
    "ltrim",
    "make_date",
    "md5",
    "mod",
    "now",
    "octet_length",
    "overlay",
    "position",
    "regexp_replace",
    "replace",
    "reverse",
    "right",
    "round",
    "rpad",
    "rtrim",
    "sign",
    "split_part",
    "starts_with",
    "strpos",
    "substr",
    "substring",
    "timezone",
    "to_char",
    "to_date",
    "to_number",
    "to_timestamp",
    "trunc",
    "upper",
];

/// PostgreSQL built-ins a filter may not call, by name however qualified: they wait, take
/// locks, change the session or a sequence, notify, reach another server, or read, list or write
/// the server's files or its large objects (a filter is one boolean expression with no effect).
/// Sorted.
pub const PG_REFUSED_CALLS: &[&str] = &[
    "dblink",
    "dblink_connect",
    "dblink_connect_u",
    "dblink_exec",
    "dblink_send_query",
    "lo_close",
    "lo_creat",
    "lo_create",
    "lo_export",
    "lo_from_bytea",
    "lo_get",
    "lo_import",
    "lo_lseek",
    "lo_lseek64",
    "lo_open",
    "lo_put",
    "lo_tell",
    "lo_tell64",
    "lo_truncate",
    "lo_truncate64",
    "lo_unlink",
    "loread",
    "lowrite",
    "nextval",
    "pg_advisory_lock",
    "pg_advisory_lock_shared",
    "pg_advisory_unlock",
    "pg_advisory_unlock_all",
    "pg_advisory_unlock_shared",
    "pg_advisory_xact_lock",
    "pg_advisory_xact_lock_shared",
    "pg_ls_archive_statusdir",
    "pg_ls_dir",
    "pg_ls_logdir",
    "pg_ls_logicalmapdir",
    "pg_ls_logicalsnapdir",
    "pg_ls_replslotdir",
    "pg_ls_summariesdir",
    "pg_ls_tmpdir",
    "pg_ls_waldir",
    "pg_notify",
    "pg_read_binary_file",
    "pg_read_file",
    "pg_sleep",
    "pg_sleep_for",
    "pg_sleep_until",
    "pg_stat_file",
    "pg_try_advisory_lock",
    "pg_try_advisory_lock_shared",
    "pg_try_advisory_xact_lock",
    "pg_try_advisory_xact_lock_shared",
    "set_config",
    "setseed",
    "setval",
];

/// MySQL built-ins a filter may call without asking, upper case: they read their arguments and
/// nothing else. Sorted.
pub const MYSQL_FILTER_CALLS: &[&str] = &[
    "ABS",
    "CAST",
    "CEIL",
    "CEILING",
    "CHARACTER_LENGTH",
    "CHAR_LENGTH",
    "COALESCE",
    "CONCAT",
    "CONCAT_WS",
    "CONVERT",
    "CONVERT_TZ",
    "CURDATE",
    "CURRENT_DATE",
    "CURRENT_TIMESTAMP",
    "CURTIME",
    "DATE",
    "DATEDIFF",
    "DATE_ADD",
    "DATE_FORMAT",
    "DATE_SUB",
    "DAY",
    "DAYOFMONTH",
    "DAYOFWEEK",
    "EXTRACT",
    "FIND_IN_SET",
    "FLOOR",
    "GREATEST",
    "HEX",
    "HOUR",
    "IF",
    "IFNULL",
    "INSTR",
    "JSON_CONTAINS",
    "JSON_EXTRACT",
    "JSON_LENGTH",
    "JSON_TYPE",
    "JSON_UNQUOTE",
    "LCASE",
    "LEAST",
    "LEFT",
    "LENGTH",
    "LOCATE",
    "LOWER",
    "LPAD",
    "LTRIM",
    "MD5",
    "MID",
    "MINUTE",
    "MOD",
    "MONTH",
    "NOW",
    "NULLIF",
    "OCTET_LENGTH",
    "POSITION",
    "REPLACE",
    "REVERSE",
    "RIGHT",
    "ROUND",
    "RPAD",
    "RTRIM",
    "SECOND",
    "SIGN",
    "STR_TO_DATE",
    "SUBSTR",
    "SUBSTRING",
    "TIME",
    "TIMESTAMPDIFF",
    "TRIM",
    "TRUNCATE",
    "UCASE",
    "UNHEX",
    "UPPER",
    "UTC_TIMESTAMP",
    "YEAR",
];

/// MySQL built-ins a filter may not call: they wait, take or test user locks, read a file of the
/// server, or advance or set a sequence (MariaDB's `NEXTVAL`, `SETVAL`); or they read the
/// outcome of the statement before (`FOUND_ROWS`, `ROW_COUNT`), which for a table's statement
/// is one the app sent. Sorted.
pub const MYSQL_REFUSED_CALLS: &[&str] = &[
    "BENCHMARK",
    "FOUND_ROWS",
    "GET_LOCK",
    "IS_FREE_LOCK",
    "IS_USED_LOCK",
    "LOAD_FILE",
    "MASTER_POS_WAIT",
    "NEXTVAL",
    "RELEASE_ALL_LOCKS",
    "RELEASE_LOCK",
    "ROW_COUNT",
    "SETVAL",
    "SLEEP",
    "SOURCE_POS_WAIT",
    "WAIT_FOR_EXECUTED_GTID_SET",
    "WAIT_UNTIL_SQL_THREAD_AFTER_GTIDS",
];

/// Check filter `text` for table `table` of `target` (see the module docs): what the user must
/// confirm before it runs, or why it may not. `classifier` is the session's (its dialect should
/// be the target's); `read_only`: the profile is read-only.
pub fn check_filter(
    text: &str,
    table: &TableName,
    target: &Target,
    classifier: &Classifier,
    read_only: bool,
) -> Result<FilterVerdict, BarRefusal> {
    let readings = readings(text, target.dialect)?;
    let query =
        Query { filter: Some(Filter { text: text.to_string(), recognised: None }), ..Query::table(table.clone()) };
    let sql = compile(&query, target).sql;
    let risk = classifier.clone().classify(&sql);
    // PostgreSQL: a row lock written in the text is named as one, where it is, also when it
    // leaves the statement unparsed (`x = 1 FOR UPDATE` inside the parentheses).
    let pg_lock = || {
        let at = readings.first().and_then(|r| pg_lock_at(&r.tokens, text));
        BarRefusal::Shape { problem: ShapeProblem::Locking, at }
    };
    if matches!(risk.danger, Some(Danger::Unparsed | Danger::TooComplex)) {
        if target.dialect == Dialect::Postgres
            && readings.first().is_some_and(|r| pg_lock_at(&r.tokens, text).is_some())
        {
            return Err(pg_lock());
        }
        return Err(BarRefusal::NotRead { class: risk.class, danger: risk.danger });
    }
    let unread = || BarRefusal::NotRead { class: Class::Unknown, danger: Some(Danger::Unparsed) };
    let calls: Vec<Call> = match target.dialect {
        Dialect::Postgres => {
            let Source::Table(t) = &query.source;
            match pg_tree(&sql, t) {
                Some(Ok(calls)) => calls,
                Some(Err(ShapeProblem::Locking)) => return Err(pg_lock()),
                Some(Err(problem)) => return Err(BarRefusal::Shape { problem, at: None }),
                None if readings.first().is_some_and(|r| pg_lock_at(&r.tokens, text).is_some()) => {
                    return Err(pg_lock());
                }
                None => return Err(unread()),
            }
        }
        Dialect::MySql(_) => {
            let mut calls = Vec::new();
            for r in &readings {
                mysql_shape(&r.tokens, text).map_err(|(problem, at)| BarRefusal::Shape { problem, at: Some(at) })?;
                for written in mysql_sequence_reads(&r.tokens, text) {
                    let call = Call { written, allowed: false, refused: false };
                    if !calls.contains(&call) {
                        calls.push(call);
                    }
                }
                let Dialect::MySql(mode) = r.dialect else { continue };
                let unread_here = || BarRefusal::NotRead { class: risk.class, danger: Some(Danger::Unrecognized) };
                for (name, qualified) in risk::mysql::called(&sql, mode).ok_or_else(unread_here)? {
                    let call = Call {
                        allowed: !qualified && MYSQL_FILTER_CALLS.binary_search(&name.as_str()).is_ok(),
                        refused: MYSQL_REFUSED_CALLS.binary_search(&name.as_str()).is_ok(),
                        written: name,
                    };
                    if !calls.contains(&call) {
                        calls.push(call);
                    }
                }
            }
            calls
        }
    };
    if risk.class != Class::Read || risk.danger.is_some() || risk.read_write {
        return Err(BarRefusal::NotRead { class: risk.class, danger: risk.danger });
    }
    let written = |keep: fn(&Call) -> bool| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for c in calls.iter().filter(|c| keep(c)) {
            if !out.contains(&c.written) {
                out.push(c.written.clone());
            }
        }
        out
    };
    let refused = written(|c| c.refused);
    if !refused.is_empty() {
        return Err(BarRefusal::RefusedCalls(refused));
    }
    let unchecked = written(|c| !c.allowed);
    if read_only {
        if !unchecked.is_empty() {
            return Err(BarRefusal::UncheckedCalls(unchecked));
        }
        risk.read_only().map_err(BarRefusal::ReadOnly)?;
    }
    Ok(if unchecked.is_empty() {
        FilterVerdict::Clear
    } else {
        FilterVerdict::NeedsConfirm(vec![Confirm::Calls(unchecked)])
    })
}

/// A function call of the statement: its name as written (PostgreSQL: with its schema, if
/// written; MySQL: upper case, without its database), whether it is on the filter allowlist
/// (by a bare name, or PostgreSQL's `pg_catalog.` one) and whether the filter refuses it
/// (however qualified).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Call {
    written: String,
    allowed: bool,
    refused: bool,
}

/// One way the server may read the text: its tokens (of the text and a blank after it) and the
/// dialect it was read in.
struct Reading {
    tokens: Vec<Token>,
    dialect: Dialect,
}

/// The lexical check alone (step 1 of the module docs).
pub(super) fn lexical(text: &str, d: Dialect) -> Result<(), BarRefusal> {
    readings(text, d).map(|_| ())
}

/// The readings of `text` that the server may run, after the lexical check of each.
fn readings(text: &str, d: Dialect) -> Result<Vec<Reading>, BarRefusal> {
    if text.trim().is_empty() {
        return Err(BarRefusal::Empty);
    }
    // A blank after the text: a string or a name left open runs over it.
    let ext = format!("{text} ");
    let mut all = vec![Reading { tokens: lex_in(&ext, d), dialect: d }];
    match d {
        Dialect::Postgres if text.contains('\\') => {
            all.push(Reading { tokens: lex_backslash_strings(&ext), dialect: d });
        }
        Dialect::Postgres => {}
        Dialect::MySql(mode) => {
            for m in other_modes(text, mode) {
                all.push(Reading { tokens: lex_in(&ext, Dialect::MySql(m)), dialect: Dialect::MySql(m) });
            }
        }
    }
    let mut out: Vec<Reading> = Vec::new();
    for (i, r) in all.into_iter().enumerate() {
        match problems(&r.tokens, text, r.dialect) {
            Ok(()) => {}
            Err((problem, at, _)) if i == 0 => return Err(BarRefusal::Lexical { problem, at }),
            // A syntax error on the server: that reading does not run.
            Err((_, _, true)) => continue,
            Err((problem, at, false)) => return Err(BarRefusal::Lexical { problem, at }),
        }
        if let Some(own) = out.first() {
            let n = own.tokens.len().min(r.tokens.len());
            let same = |k: usize| own.tokens[k].start == r.tokens[k].start && own.tokens[k].end == r.tokens[k].end;
            let differ = (0..n).find(|&k| !same(k)).or((own.tokens.len() != r.tokens.len()).then_some(n));
            if let Some(k) = differ {
                let t = &own.tokens[k.min(own.tokens.len() - 1)];
                return Err(BarRefusal::Lexical { problem: LexProblem::Ambiguous, at: clamp(t, text) });
            }
        }
        out.push(r);
    }
    Ok(out)
}

/// The other sql modes that read MySQL text `text` differently from `mode`: with a backslash,
/// the other `NO_BACKSLASH_ESCAPES`; with a `"`, the other `ANSI_QUOTES`; with both, both.
fn other_modes(text: &str, mode: MySqlMode) -> Vec<MySqlMode> {
    let backslash = text.contains('\\');
    let quote = text.contains('"');
    let nbe = MySqlMode { no_backslash_escapes: !mode.no_backslash_escapes, ..mode };
    let ansi = MySqlMode { ansi_quotes: !mode.ansi_quotes, ..mode };
    let both = MySqlMode { ansi_quotes: !mode.ansi_quotes, ..nbe };
    [(backslash, nbe), (quote, ansi), (backslash && quote, both)]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, m)| m)
        .collect()
}

/// Token `t`'s byte range within `text` (not the blank after it).
fn clamp(t: &Token, text: &str) -> Range<usize> {
    t.start.min(text.len())..t.end.min(text.len())
}

/// The first token of one reading the filter may not hold: the problem, where, and whether the
/// server would refuse the statement for it as a syntax error (a string or a parenthesis left
/// open).
fn problems(tokens: &[Token], text: &str, d: Dialect) -> Result<(), (LexProblem, Range<usize>, bool)> {
    let mut open: Vec<Range<usize>> = Vec::new();
    for t in tokens.iter().filter(|t| t.start < text.len()) {
        let at = clamp(t, text);
        let problem = match t.kind {
            Tok::Semi => LexProblem::Semicolon,
            Tok::LineComment => LexProblem::Comment,
            Tok::BlockComment if matches!(d, Dialect::MySql(_)) && text[at.clone()].starts_with("/*M!") => {
                LexProblem::ExecutableComment
            }
            Tok::BlockComment => LexProblem::Comment,
            Tok::ExecComment => LexProblem::ExecutableComment,
            Tok::Directive => LexProblem::ClientCommand,
            // Only a string runs to the end of the statement; a quoted name could close on a
            // quote the app writes after the text.
            Tok::Str | Tok::Dollar if t.end > text.len() => return Err((LexProblem::Unterminated, at, true)),
            Tok::QuotedIdent if t.end > text.len() => LexProblem::Unterminated,
            Tok::LParen => {
                open.push(at);
                continue;
            }
            Tok::RParen if open.pop().is_none() => LexProblem::ClosesOutside,
            _ => continue,
        };
        return Err((problem, at, false));
    }
    match open.pop() {
        Some(at) => Err((LexProblem::Unclosed, at, true)),
        None => Ok(()),
    }
}

/// The MySQL token rules of the shape check, on one reading's tokens of the text.
fn mysql_shape(tokens: &[Token], text: &str) -> Result<(), (ShapeProblem, Range<usize>)> {
    let ws: Vec<&Token> = tokens.iter().filter(|t| !t.is_trivia() && t.start < text.len()).collect();
    let mut depth = 0usize;
    for (i, t) in ws.iter().enumerate() {
        let at = clamp(t, text);
        let up = if t.is_word() { t.text(text).to_ascii_uppercase() } else { String::new() };
        let next_is = |words: &[&str]| {
            ws.get(i + 1).is_some_and(|n| n.is_word() && words.iter().any(|w| n.text(text).eq_ignore_ascii_case(w)))
        };
        let problem = match (t.kind, up.as_str()) {
            (Tok::LParen, _) => {
                depth += 1;
                continue;
            }
            (Tok::RParen, _) => {
                depth = depth.saturating_sub(1);
                continue;
            }
            (Tok::Op, _)
                if t.text(text) == ":" && ws.get(i + 1).is_some_and(|n| n.start == t.end && n.text(text) == "=") =>
            {
                ShapeProblem::Assignment
            }
            (_, "INTO") => ShapeProblem::Into,
            (_, "NEXT") if next_is(&["VALUE"]) => ShapeProblem::Sequence,
            _ if after_dot(&ws, i) && unquoted(t.text(text)).eq_ignore_ascii_case("NEXTVAL") => ShapeProblem::Sequence,
            (_, "LOCK") => ShapeProblem::Locking,
            (_, "FOR") if next_is(&["UPDATE", "SHARE"]) => ShapeProblem::Locking,
            (_, "PROCEDURE") => ShapeProblem::Clause,
            (_, "UNION" | "INTERSECT" | "EXCEPT") if depth == 0 => ShapeProblem::SetOperation,
            (_, "FOR" | "LIMIT" | "ORDER" | "GROUP" | "HAVING" | "WINDOW") if depth == 0 => ShapeProblem::Clause,
            _ => continue,
        };
        return Err((problem, at));
    }
    Ok(())
}

/// Whether word `i` of `ws` follows a `.`: a name in attribute notation (`s.nextval`).
fn after_dot(ws: &[&Token], i: usize) -> bool {
    (ws[i].is_word() || ws[i].kind == Tok::QuotedIdent) && i > 0 && ws[i - 1].kind == Tok::Dot
}

/// A name without its quotes (`` `a` ``, `"a"`), as the server reads it when no quote is doubled.
fn unquoted(name: &str) -> &str {
    let quoted = name.len() >= 2
        && ((name.starts_with('`') && name.ends_with('`')) || (name.starts_with('"') && name.ends_with('"')));
    if quoted { &name[1..name.len() - 1] } else { name }
}

/// MariaDB's reads of a sequence's last value that are not written as a call: `s.currval` (a
/// session in the ORACLE sql mode) and `PREVIOUS VALUE FOR s`, named as `CURRVAL` and `PREVIOUS
/// VALUE FOR`. They change nothing but answer from what the session did before, as `LASTVAL(s)`
/// does, so they are asked about as a call whose effects are not known.
fn mysql_sequence_reads(tokens: &[Token], text: &str) -> Vec<String> {
    let ws: Vec<&Token> = tokens.iter().filter(|t| !t.is_trivia() && t.start < text.len()).collect();
    let word = |i: usize, w: &str| ws.get(i).is_some_and(|t| t.is_word() && t.text(text).eq_ignore_ascii_case(w));
    let mut out = Vec::new();
    for i in 0..ws.len() {
        let name = if after_dot(&ws, i) && unquoted(ws[i].text(text)).eq_ignore_ascii_case("CURRVAL") {
            "CURRVAL"
        } else if word(i, "PREVIOUS") && word(i + 1, "VALUE") && word(i + 2, "FOR") {
            "PREVIOUS VALUE FOR"
        } else {
            continue;
        };
        if !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// PostgreSQL: where text `text` (its tokens `tokens`) writes a row lock, `FOR` through its
/// last word (`FOR UPDATE`, `FOR NO KEY UPDATE`, `FOR SHARE`, `FOR KEY SHARE`); `None` when
/// it writes none.
fn pg_lock_at(tokens: &[Token], text: &str) -> Option<Range<usize>> {
    let ws: Vec<&Token> = tokens.iter().filter(|t| !t.is_trivia() && t.start < text.len()).collect();
    let word = |i: usize, w: &str| ws.get(i).is_some_and(|t| t.is_word() && t.text(text).eq_ignore_ascii_case(w));
    (0..ws.len()).filter(|&i| word(i, "FOR")).find_map(|i| {
        let last = if word(i + 1, "UPDATE") || word(i + 1, "SHARE") {
            i + 1
        } else if word(i + 1, "NO") && word(i + 2, "KEY") && word(i + 3, "UPDATE") {
            i + 3
        } else if word(i + 1, "KEY") && word(i + 2, "SHARE") {
            i + 2
        } else {
            return None;
        };
        Some(clamp(ws[i], text).start..clamp(ws[last], text).end)
    })
}

/// The PostgreSQL shape check of statement `sql` on its parse tree, read on the classifier's
/// thread: the calls it makes (each as written, `schema.f` with its schema), or what is wrong
/// with its shape. `None` when the parser rejects it.
fn pg_tree(sql: &str, table: &TableName) -> Option<Result<Vec<Call>, ShapeProblem>> {
    std::thread::scope(|scope| {
        let worker = std::thread::Builder::new().name(risk::THREAD.to_string()).stack_size(risk::STACK);
        worker.spawn_scoped(scope, || pg_tree_here(sql, table)).ok().and_then(|h| h.join().ok()).flatten()
    })
}

fn pg_tree_here(sql: &str, table: &TableName) -> Option<Result<Vec<Call>, ShapeProblem>> {
    let stmts = risk::parse(sql)?;
    Some(pg_shape(&stmts, table))
}

fn pg_shape(stmts: &[Value], table: &TableName) -> Result<Vec<Call>, ShapeProblem> {
    let empty = |v: &Value| v.as_array().is_none_or(Vec::is_empty);
    let [stmt] = stmts else { return Err(ShapeProblem::NotOneSelect) };
    let Some(("SelectStmt", b)) = risk::node(stmt) else { return Err(ShapeProblem::NotOneSelect) };
    let setop_none = pg_query::protobuf::SetOperation::SetopNone as i64;
    if b["op"].as_i64().is_some_and(|op| op != setop_none) || !b["larg"].is_null() || !b["rarg"].is_null() {
        return Err(ShapeProblem::SetOperation);
    }
    // Row locks and `INTO` at any depth.
    let (mut locking, mut into) = (false, false);
    let mut look = |o: &serde_json::Map<String, Value>| {
        locking |= o.get("locking_clause").is_some_and(|l| !empty(l));
        into |= o.get("into_clause").is_some_and(|i| !i.is_null());
    };
    if let Some(o) = b.as_object() {
        look(o);
    }
    risk::walk(b, &mut look);
    if locking {
        return Err(ShapeProblem::Locking);
    }
    if into {
        return Err(ShapeProblem::Into);
    }
    let ours = match b["from_clause"].as_array().map(Vec::as_slice) {
        Some([from]) => match risk::node(from) {
            Some(("RangeVar", r)) => {
                r["catalogname"].as_str().is_none_or(str::is_empty)
                    && r["schemaname"].as_str() == Some(table.schema.as_str())
                    && r["relname"].as_str() == Some(table.name.as_str())
                    && r["alias"].is_null()
                    && r["inh"] == Value::Bool(true)
            }
            _ => false,
        },
        _ => false,
    };
    let star = match b["target_list"].as_array().map(Vec::as_slice) {
        Some([t]) => match risk::node(t) {
            Some(("ResTarget", r)) => {
                r["name"].as_str().is_none_or(str::is_empty)
                    && empty(&r["indirection"])
                    && matches!(risk::node(&r["val"]), Some(("ColumnRef", c))
                        if matches!(c["fields"].as_array().map(Vec::as_slice), Some([f]) if matches!(risk::node(f), Some(("AStar", _)))))
            }
            _ => false,
        },
        _ => false,
    };
    if !ours || !star {
        return Err(ShapeProblem::Source);
    }
    let lists = ["distinct_clause", "group_clause", "window_clause", "values_lists", "sort_clause"];
    let nodes = ["having_clause", "limit_count", "limit_offset", "with_clause"];
    if b["where_clause"].is_null() || lists.iter().any(|k| !empty(&b[k])) || nodes.iter().any(|k| !b[k].is_null()) {
        return Err(ShapeProblem::Clause);
    }
    // Every call, however it is written: a `FuncCall`, or attribute notation (`(expr).f`,
    // `t.f`), which the server runs as `f(expr)` when there is no column or field `f`. A name in
    // attribute notation counts only when refused: any other is a column or a field as often as
    // a call, and is not asked about (see the module docs).
    let mut calls = Vec::new();
    risk::walk(b, &mut |o| {
        for found in risk::calls(o) {
            let call = match found {
                risk::Call::Func(names, _) => {
                    let allowed =
                        matches!(names.as_slice(), [f] | ["pg_catalog", f] if PG_FILTER_CALLS.binary_search(f).is_ok());
                    let refused = names.last().is_some_and(|f| PG_REFUSED_CALLS.binary_search(f).is_ok());
                    Call { written: names.join("."), allowed, refused }
                }
                risk::Call::Attribute(f, _) if PG_REFUSED_CALLS.binary_search(&f).is_ok() => {
                    Call { written: f.to_string(), allowed: false, refused: true }
                }
                risk::Call::Attribute(..) => continue,
            };
            if !calls.contains(&call) {
                calls.push(call);
            }
        }
    });
    Ok(calls)
}
