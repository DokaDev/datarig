//! What a MySQL statement may do to a database, read from **its tokens** as datarig's MySQL
//! lexer reads them ([`crate::sql::lexer`], held to MySQL 8.0, 8.4 and 9.x's own client and
//! server by its tests), not from a parse tree: no SQL parser written in Rust reads MySQL as
//! the server does, and a second reading of the text that disagrees with the server's is how
//! a write could pass for a read. Whether each verdict matches the server is held to MySQL
//! 8.0, 8.4 and 9.x by a test that runs the statements on them (`tests/mysql_classify.rs`).
//!
//! **Allowlist, not denylist.** A statement is a read only when it has a form this module
//! knows to be one, and nothing in it is known to act otherwise:
//!
//! * a query (`SELECT`, `TABLE`, `VALUES`, `WITH … SELECT`, one in parentheses) without
//!   `INTO` and without a locking clause (`FOR UPDATE`, `FOR SHARE`, `LOCK IN SHARE MODE`:
//!   those are [`Class::Write`]s that change no rows, as on PostgreSQL); `SELECT … INTO @x`
//!   only sets the session's variables ([`Class::Session`], allowed on a read-only profile);
//!   `INTO OUTFILE`/`DUMPFILE` writes a file on the server ([`Danger::FileAccess`]);
//! * `SHOW`, `DESCRIBE`/`DESC`, `HELP`, `XA RECOVER`, `GET DIAGNOSTICS`, `DO` of built-ins,
//!   and `EXPLAIN` of a query. `EXPLAIN` of `INSERT`/`UPDATE`/`DELETE`/`REPLACE` runs nothing
//!   either, but MySQL refuses it in a read-only transaction, so it is a [`Class::Write`] that
//!   changes no rows. `EXPLAIN ANALYZE` runs the statement: its risk is the statement's, with
//!   [`Explain::Analyze`].
//!
//! A locking clause anywhere (in a subquery of a `SET`, a `SHOW … WHERE`, a `DO`) makes the
//! statement a [`Class::Write`]: MySQL's read-only mode lets `FOR SHARE` take row locks.
//!
//! **The sql mode.** A backslash in a string and a `"` read differently under
//! `NO_BACKSLASH_ESCAPES` and `ANSI_QUOTES`. The session's mode is the driver's to tell, but a
//! server's may differ from what the app was told, so a text with either is read in every
//! mode that reads it differently and the worse reading counts, unless that reading is one the
//! server would refuse as a syntax error (an unterminated string: no way the text runs). The
//! driver must keep the session's client character set UTF-8: another one (`gbk`, `sjis`) can
//! read a byte of a character as a quote or a backslash, which is why `SET NAMES` of one asks.
//!
//! A text with an executable comment (`/*! … */`, `/*!80023 … */`, and MariaDB's `/*M! … */`)
//! is never read: the server runs what is inside depending on its version, which the text does
//! not tell ([`Danger::ExecutableComment`]: it asks, and a read-only policy refuses it).
//! Optimizer hints (`/*+ … */`) are comments, except that a `SET_VAR` of a setting of
//! [`RISKY_SETTINGS`] or of read-only in one asks ([`Danger::Setting`]). A text whose form is not one of those this
//! module knows, or that the server would read otherwise than the lexer (an unterminated
//! string or comment, a client command such as `DELIMITER` or `\g`, a `?`, a parenthesis that
//! does not close, a write's keyword inside a query, …) is [`Danger::Unrecognized`]: it asks, and
//! a read-only policy refuses it. A text over [`MAX_BYTES`] or nested deeper than [`MAX_DEPTH`]
//! parentheses is [`Danger::TooComplex`]. Nothing here recurses with the text's nesting, and
//! the work runs on a thread of its own ([`super::THREAD`], as PostgreSQL's does), where a panic
//! reads as [`Danger::Unrecognized`].
//!
//! **Several statements.** Every statement of the text is read and the worst counts (the danger
//! a read-only policy refuses by itself is kept over another). `CREATE PROCEDURE`, `FUNCTION`,
//! `TRIGGER` and `EVENT` hold `;` in their body: the statement ends where the body does (one
//! statement, or a `BEGIN … END` block followed through `IF`, `CASE`, `LOOP`, `REPEAT` and
//! `WHILE`), and when that cannot be followed the whole text is the one statement (the driver
//! sends each statement alone, with the server's multi-statement mode off, so a text the server
//! reads as more than one fails as a whole). An event's body runs later on its own: a
//! statement's danger is the event's, and a block asks ([`Danger::Procedural`]).
//!
//! **What the server's read-only mode does not stop**, refused before a statement is sent: a
//! call of a built-in that reads a file of the server ([`SERVER_FILES`]) or takes a lock that
//! outlives the statement ([`SERVER_ACTIONS`]), statements that act on the whole server (`KILL`,
//! `SET GLOBAL`, `FLUSH`, `RESET`, `PURGE`, replication, `INSTALL`, `SHUTDOWN`, `XA`, …:
//! [`Danger::ServerCommand`]), locks that block other sessions (`LOCK TABLES`, `HANDLER`, `LOCK
//! INSTANCE`, `FLUSH … WITH READ LOCK`: [`Danger::Locks`]), `SET SESSION TRANSACTION READ WRITE`
//! and `transaction_read_only` turned off (`read_write`), `PREPARE` (its text is not read:
//! [`Danger::DynamicSql`]) and `EXECUTE` ([`Danger::UnknownPrepared`]; prepared statements are
//! not remembered on MySQL), and a call of an unqualified name that is not a built-in
//! ([`Risk::unchecked_call`]): it may be a loadable function, code of the server's that a
//! read-only transaction does not stop. A stored function is called with its database
//! (`db.f()`) to be allowed; its writes are refused by the server's read-only mode.
//!
//! **Settings.** `SET` of a session variable on [`SAFE_SETTINGS`], of a user variable (`@x`),
//! `SET NAMES`/`CHARACTER SET` of a UTF-8 character set and `SET ROLE` are safe; `SET` of one on
//! [`RISKY_SETTINGS`] (turns a check of the server off, changes how the server reads the text
//! or writes the binary log) asks ([`Danger::Setting`]); `SET GLOBAL`/`PERSIST` asks
//! ([`Danger::ServerCommand`]); any other session variable is refused by a read-only policy.
//! `SET autocommit` is transaction control.
//!
//! **What a statement commits.** MySQL commits the open transaction before most statements
//! that change the schema or the server ([`Risk::implicit_commit`]: DDL other than of a
//! temporary table, accounts and privileges, `LOCK TABLES`, `START TRANSACTION`/`BEGIN`, `SET
//! autocommit = 1`, table maintenance, `FLUSH`, `RESET`, replication, plugins).
//!
//! **What the confirm is not.** As on PostgreSQL, a function called from a query cannot be seen
//! from the text (`SELECT db.delete_everything()` is a read here); nor are a view's query, a
//! trigger or a generated column. `SLEEP()` and `BENCHMARK()` only take time, as a long query
//! does: they are reads. The hard guarantee is a read-only profile: the driver keeps the
//! session's `transaction_read_only` on, so the server rejects any write, whatever runs it;
//! the server still lets a `LOCK TABLES … READ`, a `HANDLER`, a `SELECT … FOR SHARE`, `SET
//! GLOBAL`, `KILL` and `FLUSH` through, which is why they are refused here.
//!
//! "Effectively every row" ([`NoWhere`]) for `UPDATE` and `DELETE` (multi-table forms too): no
//! `WHERE` (a `LIMIT` alone does not count), a `WHERE` that is always true (`TRUE`, a number
//! other than 0, `NOT FALSE`, both sides of `=` or `<=>` the same, an `OR` with such an
//! operand, an `AND` of them; names compared whatever their quoting and case), a `WHERE` that
//! names no column, or, for a multi-table one, a `WHERE` and joins that name only other tables'
//! columns ([`NoWhere::OtherTables`]). `ALTER TABLE … ENGINE = BLACKHOLE` empties the table.

use super::repeat::NotRepeatable;
use super::{Class, Danger, Explain, MAX_BYTES, MAX_DEPTH, NoWhere, Risk, THREAD};
use crate::sql::dialect::{Dialect, MySqlMode};
use crate::sql::lexer::{Tok, is_space, lex_in};

/// The names of MySQL's built-in functions, upper case, one per line in byte order: every
/// function that MySQL 8.0, 8.4 or 9.x documents in its help tables (`mysql.help_topic` of the
/// function categories, not the loadable ones), without the operators. A test runs each on the
/// three servers: none is unknown to all of them.
pub const BUILTINS: &str = include_str!("mysql_builtins.txt");

/// Whether `name` (upper case) is a built-in function in [`BUILTINS`].
pub fn builtin(name: &str) -> bool {
    static SORTED: std::sync::OnceLock<Vec<&str>> = std::sync::OnceLock::new();
    SORTED.get_or_init(|| BUILTINS.lines().collect()).binary_search(&name).is_ok()
}

/// Built-ins that read a file of the server.
pub const SERVER_FILES: &[&str] = &["LOAD_FILE"];

/// Built-ins that take or drop a lock that outlives the statement and the transaction (a user
/// lock blocks another session's `GET_LOCK` of the name until it is released).
pub const SERVER_ACTIONS: &[&str] = &["GET_LOCK", "RELEASE_ALL_LOCKS", "RELEASE_LOCK"];

/// Built-ins whose result changes from call to call, or that act or wait when called: a query
/// that calls one is not run again for the next page. `NOW()` and the like are the same
/// throughout a statement and are allowed, as PostgreSQL's stable functions are.
pub const VOLATILE: &[&str] = &[
    "ANY_VALUE",
    "BENCHMARK",
    "FOUND_ROWS",
    "GET_LOCK",
    "IS_FREE_LOCK",
    "IS_USED_LOCK",
    "LAST_INSERT_ID",
    "LOAD_FILE",
    "MASTER_POS_WAIT",
    "RAND",
    "RANDOM_BYTES",
    "RELEASE_ALL_LOCKS",
    "RELEASE_LOCK",
    "ROW_COUNT",
    "SLEEP",
    "SOURCE_POS_WAIT",
    "SYSDATE",
    "UUID",
    "UUID_SHORT",
    "WAIT_FOR_EXECUTED_GTID_SET",
    "WAIT_UNTIL_SQL_THREAD_AFTER_GTIDS",
];

/// Session variables a read-only policy lets a session change: none of them can turn read-only
/// off, write anything, change how the server reads the text the app sends, or let a statement
/// take more of the server's memory, time or locks than its defaults (buffer and result sizes,
/// recursion and planning depth, join size, lock waits and idle timeouts are not here). Lower
/// case.
pub const SAFE_SETTINGS: &[&str] = &[
    "big_tables",
    "character_set_connection",
    "character_set_results",
    "collation_connection",
    "default_week_format",
    "div_precision_increment",
    "end_markers_in_json",
    "explain_format",
    "explain_json_format_version",
    "information_schema_stats_expiry",
    "lc_messages",
    "lc_time_names",
    "max_error_count",
    "max_execution_time",
    "net_read_timeout",
    "net_write_timeout",
    "optimizer_switch",
    "optimizer_trace",
    "optimizer_trace_features",
    "optimizer_trace_limit",
    "optimizer_trace_offset",
    "profiling",
    "profiling_history_size",
    "sql_buffer_result",
    "sql_notes",
    "sql_quote_show_create",
    "sql_select_limit",
    "sql_warnings",
    "time_zone",
    "transaction_isolation",
    "tx_isolation",
    "windowing_use_high_precision",
];

/// Session variables whose change asks ([`Danger::Setting`]): they turn a check of the server
/// off (`foreign_key_checks`, `unique_checks`, `sql_safe_updates`), change how the server reads
/// the text the app sends or what it returns (`sql_mode`, `character_set_client`,
/// `resultset_metadata`, the session trackers the driver reads), or what reaches the binary log
/// and the replicas (`sql_log_bin`, `binlog_format`, `gtid_next`, …). Lower case.
pub const RISKY_SETTINGS: &[&str] = &[
    "binlog_format",
    "binlog_row_image",
    "binlog_row_value_options",
    "character_set_client",
    "foreign_key_checks",
    "gtid_next",
    "insert_id",
    "last_insert_id",
    "pseudo_replica_mode",
    "pseudo_slave_mode",
    "pseudo_thread_id",
    "resultset_metadata",
    "session_track_gtids",
    "session_track_schema",
    "session_track_state_change",
    "session_track_system_variables",
    "session_track_transaction_info",
    "sql_log_bin",
    "sql_log_off",
    "sql_mode",
    "sql_safe_updates",
    "unique_checks",
];

/// The settings that make the session's transactions read-only.
const READ_ONLY_SETTINGS: &[&str] = &["transaction_read_only", "tx_read_only"];

/// The character sets a session may switch its client to: they read the bytes of the text the
/// app sends (UTF-8) as the lexer does. Another one (`gbk`, `sjis`, …) can read a byte of a
/// character as a quote or a backslash.
const CLIENT_CHARSETS: &[&str] = &["utf8", "utf8mb3", "utf8mb4"];

/// Words that open a statement's write, or other non-read, inside a text that is otherwise a
/// read: a query, `SHOW`, `SET`, … in which one appears is not one this module knows. Reserved
/// words only, so none is a bare name.
const WRITE_WORDS: &[&str] = &[
    "ALTER", "CALL", "CREATE", "DELETE", "DROP", "GRANT", "INSERT", "KILL", "LOAD", "LOCK", "OPTIMIZE", "PURGE",
    "RENAME", "REPLACE", "REVOKE", "UNLOCK", "UPDATE",
];

/// Reserved words written before `(` that do not call a function: syntax, and the types of
/// `CAST`, `CONVERT` and column definitions. A reserved word written bare is never a function's
/// name; a non-reserved one before `(` is read as a call unless its place says otherwise
/// ([`syntax_before_paren`]).
const NOT_CALLS: &[&str] = &[
    "ALL",
    "ANALYZE",
    "AND",
    "AS",
    "BETWEEN",
    "BIGINT",
    "BINARY",
    "BLOB",
    "BY",
    "CASE",
    "CHECK",
    "CONSTRAINT",
    "DEC",
    "DECIMAL",
    "DISTINCT",
    "DIV",
    "DOUBLE",
    "ELSE",
    "EXCEPT",
    "EXISTS",
    "FLOAT",
    "FOREIGN",
    "FROM",
    "FULLTEXT",
    "HAVING",
    "IN",
    "INDEX",
    "INT",
    "INTEGER",
    "INTERSECT",
    "INTO",
    "IS",
    "JOIN",
    "KEY",
    "KEYS",
    "LATERAL",
    "LIKE",
    "LIMIT",
    "LONGBLOB",
    "LONGTEXT",
    "MATCH",
    "MEDIUMBLOB",
    "MEDIUMINT",
    "MEDIUMTEXT",
    "NOT",
    "NUMERIC",
    "OF",
    "ON",
    "OR",
    "OVER",
    "PARTITION",
    "PRIMARY",
    "RANGE",
    "REAL",
    "REFERENCES",
    "REGEXP",
    "RETURN",
    "RLIKE",
    "ROW",
    "SELECT",
    "SET",
    "SMALLINT",
    "SPATIAL",
    "TABLE",
    "THEN",
    "TINYBLOB",
    "TINYINT",
    "TINYTEXT",
    "UNION",
    "UNIQUE",
    "UNSIGNED",
    "USING",
    "VALUES",
    "VARBINARY",
    "VARCHAR",
    "WHEN",
    "WHERE",
    "WINDOW",
    "WITH",
    "XOR",
];

/// Words after which a name followed by `(` is a table or an index, not a function: its column
/// list follows.
const BEFORE_COLUMN_LIST: &[&str] =
    &["AS", "IGNORE", "INDEX", "INSERT", "INTO", "KEY", "REFERENCES", "REPLACE", "TABLE", "VIEW"];

/// Words of a condition that are not columns: operators, constants, the units of `INTERVAL` and
/// the functions written without parentheses.
const NOT_COLUMNS: &[&str] = &[
    "ALL",
    "AND",
    "ANY",
    "AS",
    "BETWEEN",
    "BINARY",
    "CASE",
    "COLLATE",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURRENT_USER",
    "DISTINCT",
    "DIV",
    "DUAL",
    "ELSE",
    "END",
    "ESCAPE",
    "EXISTS",
    "FALSE",
    "FROM",
    "IN",
    "INTERVAL",
    "IS",
    "LIKE",
    "LOCALTIME",
    "LOCALTIMESTAMP",
    "MOD",
    "NOT",
    "NULL",
    "OR",
    "REGEXP",
    "RLIKE",
    "SELECT",
    "SOME",
    "SOUNDS",
    "THEN",
    "TRUE",
    "UNKNOWN",
    "UNION",
    "UTC_DATE",
    "UTC_TIME",
    "UTC_TIMESTAMP",
    "WHEN",
    "WHERE",
    "XOR",
];

/// The units of `INTERVAL`, which are not columns after one.
const UNITS: &[&str] = &[
    "DAY",
    "DAY_HOUR",
    "DAY_MICROSECOND",
    "DAY_MINUTE",
    "DAY_SECOND",
    "HOUR",
    "HOUR_MICROSECOND",
    "HOUR_MINUTE",
    "HOUR_SECOND",
    "MICROSECOND",
    "MINUTE",
    "MINUTE_MICROSECOND",
    "MINUTE_SECOND",
    "MONTH",
    "QUARTER",
    "SECOND",
    "SECOND_MICROSECOND",
    "WEEK",
    "YEAR",
    "YEAR_MONTH",
];

/// A token of a statement that is not a blank or a comment.
#[derive(Clone, Debug)]
struct W<'a> {
    kind: Tok,
    text: &'a str,
    /// A word (a name or a keyword, not quoted) in upper case; empty for any other token.
    up: String,
    /// The parentheses open around it (a parenthesis is at the depth outside it).
    depth: usize,
    /// A parenthesis: how far its mate is (`+` for an opening one, `-` for a closing one); 0
    /// for any other token.
    mate: isize,
}

impl W<'_> {
    fn is(&self, word: &str) -> bool {
        self.up == word
    }

    fn is_word(&self) -> bool {
        !self.up.is_empty()
    }

    /// A name: a word or a quoted name.
    fn is_name(&self) -> bool {
        self.is_word() || self.kind == Tok::QuotedIdent
    }

    fn is_op(&self, op: &str) -> bool {
        self.kind == Tok::Op && self.text == op
    }
}

/// A call of a function in the text.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Call {
    /// Its name, upper case, without quotes.
    name: String,
    /// Written with its database (`db.f()`): a stored function.
    qualified: bool,
}

impl Call {
    fn builtin(&self) -> bool {
        !self.qualified && builtin(&self.name)
    }
}

/// Why a text is not read into statements.
enum Unread {
    /// The server would refuse it as a syntax error before running anything: an unterminated
    /// string, quoted name or comment, a parenthesis that does not close, a `;` inside one.
    Syntax,
    /// Its risk is known without reading its statements (an executable comment, a client
    /// command, a text over the caps, …).
    Risk(Risk),
}

/// The risk of MySQL text `sql`, read in sql mode `mode`: one statement, or several separated
/// by `;` (their worst). A text with a backslash or a `"` is also read as a session in the other
/// modes would read it ([`readings`]), and the worse reading counts, unless that reading is
/// one the server would refuse as a syntax error. It runs on a thread of its own ([`THREAD`]),
/// as PostgreSQL's does: a panic there reads as [`Danger::Unrecognized`].
pub fn classify(sql: &str, mode: MySqlMode) -> Risk {
    guarded(unrecognized, || {
        let mut modes = readings(sql, mode).into_iter();
        let first = modes.next().and_then(|m| classify_as(sql, m)).unwrap_or_else(unrecognized);
        modes.filter_map(|m| classify_as(sql, m)).fold(first, worse)
    })
}

/// The sql modes to read `sql` in: `mode` first, then the others that read it differently (a
/// backslash in it reads otherwise under `NO_BACKSLASH_ESCAPES`, a `"` under `ANSI_QUOTES`).
/// The session's mode is the driver's to keep current, and a server's default may differ from
/// what the app was told.
fn readings(sql: &str, mode: MySqlMode) -> Vec<MySqlMode> {
    let backslash = sql.contains('\\');
    let quote = sql.contains('"');
    let mut out = vec![mode];
    if backslash {
        out.push(MySqlMode { no_backslash_escapes: !mode.no_backslash_escapes, ..mode });
    }
    if quote {
        out.push(MySqlMode { ansi_quotes: !mode.ansi_quotes, ..mode });
    }
    if backslash && quote {
        out.push(MySqlMode {
            ansi_quotes: !mode.ansi_quotes,
            no_backslash_escapes: !mode.no_backslash_escapes,
            ..mode
        });
    }
    out
}

/// `f` on a thread of its own named [`THREAD`]; `fallback` when it panics or the thread cannot
/// start.
fn guarded<T: Send>(fallback: impl FnOnce() -> T, f: impl FnOnce() -> T + Send) -> T {
    let run = std::thread::scope(|scope| {
        let worker = std::thread::Builder::new().name(THREAD.to_string()).stack_size(8 << 20);
        worker.spawn_scoped(scope, f).ok().and_then(|h| h.join().ok())
    });
    run.unwrap_or_else(fallback)
}

/// [`classify`] in one sql mode; `None` when the server in that mode would refuse the text as a
/// syntax error ([`Unread::Syntax`]: no way it runs so).
fn classify_as(sql: &str, mode: MySqlMode) -> Option<Risk> {
    match statements(sql, mode) {
        Ok((stmts, hint)) => {
            let risk = stmts.iter().map(|ws| statement(ws)).reduce(worse).unwrap_or_else(|| Risk::of(Class::Unknown));
            Some(if hint { worse(risk, Risk::danger(Class::Read, Danger::Setting)) } else { risk })
        }
        Err(Unread::Syntax) => None,
        Err(Unread::Risk(risk)) => Some(risk),
    }
}

/// The statements of `sql` (their tokens), and whether an optimizer hint of it sets a risky
/// setting ([`words`]); or why the text is not read.
fn statements(sql: &str, mode: MySqlMode) -> Result<(Vec<Vec<W<'_>>>, bool), Unread> {
    let (mut ws, hint) = words(sql, mode)?;
    let mut out = Vec::new();
    if starts_compound(&ws) {
        match routine_end(&ws) {
            RoutineEnd::At(end) => {
                let rest = ws.split_off(end);
                out.push(ws);
                ws = rest;
            }
            RoutineEnd::Whole => return Ok((vec![ws], hint)),
            // What follows its body may be other statements, which are not read.
            RoutineEnd::Lost => return Err(Unread::Risk(ddl(Some(Danger::Unrecognized)))),
        }
    }
    let mut cur = Vec::new();
    for w in ws {
        if w.kind == Tok::Semi {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(w);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    Ok((out, hint))
}

/// The worse of two risks ([`Risk::merge`]), keeping the danger a read-only policy refuses by
/// itself when only one of them has such a danger (the merge keeps the first one).
fn worse(a: Risk, b: Risk) -> Risk {
    let blocks = |d: Danger| Risk::danger(Class::Read, d).read_only().is_err();
    let danger = match (a.danger, b.danger) {
        (Some(x), Some(y)) if !blocks(x) && blocks(y) => Some(y),
        (x, y) => x.or(y),
    };
    Risk { danger, ..a.merge(b) }
}

fn unrecognized() -> Risk {
    Risk::danger(Class::Unknown, Danger::Unrecognized)
}

/// The tokens of `sql` that are not blanks or comments, and whether an optimizer hint sets a
/// setting of [`RISKY_SETTINGS`] or turns read-only off for its statement (`/*+
/// SET_VAR(foreign_key_checks = 0) */`); or the risk of a text that is not read (see the module
/// docs).
fn words(sql: &str, mode: MySqlMode) -> Result<(Vec<W<'_>>, bool), Unread> {
    let not_read = |risk: Risk| Err(Unread::Risk(risk));
    if sql.len() > MAX_BYTES {
        return not_read(Risk::danger(Class::Unknown, Danger::TooComplex));
    }
    let backslash = !mode.no_backslash_escapes;
    let mut out: Vec<W> = Vec::new();
    let mut depth = 0usize;
    let mut open: Vec<usize> = Vec::new();
    let mut hint = false;
    for t in lex_in(sql, Dialect::MySql(mode)) {
        let text = t.text(sql);
        match t.kind {
            Tok::ExecComment => return not_read(Risk::danger(Class::Unknown, Danger::ExecutableComment)),
            Tok::BlockComment if text.starts_with("/*M!") => {
                return not_read(Risk::danger(Class::Unknown, Danger::ExecutableComment));
            }
            Tok::BlockComment if text.len() < 4 || !text.ends_with("*/") => return Err(Unread::Syntax),
            Tok::Directive | Tok::Param => return not_read(unrecognized()),
            // The server reads no escape in a quoted name, nor in a hex or bit string, where the
            // lexer (as the client) reads one: where such a token ends is not known.
            Tok::QuotedIdent | Tok::Str
                if backslash
                    && text.contains('\\')
                    && ((t.kind == Tok::QuotedIdent && text.starts_with('"'))
                        || (t.kind == Tok::Str && text.starts_with(['x', 'X', 'b', 'B']))) =>
            {
                return not_read(unrecognized());
            }
            Tok::Str | Tok::QuotedIdent | Tok::Variable if !closed(text, t.kind, backslash) => {
                return Err(Unread::Syntax);
            }
            Tok::BlockComment => {
                hint |= text.starts_with("/*+") && sets_risky(text);
                continue;
            }
            Tok::LineComment => continue,
            // A backslash outside strings: the server reads only `\N` (NULL); a client command
            // (`\g`, `\G`) or a backslash at a line's end is a syntax error there.
            _ if text.starts_with('\\') && text != "\\N" => return Err(Unread::Syntax),
            _ if text == "\\N" || text.chars().any(|c| c.is_control() && !is_space(c)) => {
                // `\N`, or a control character: not read.
                if t.kind != Tok::Str && t.kind != Tok::QuotedIdent {
                    return not_read(unrecognized());
                }
            }
            _ => {}
        }
        if t.kind == Tok::Whitespace {
            continue;
        }
        if t.kind == Tok::Variable && text.contains('\\') {
            return not_read(unrecognized());
        }
        let at = match t.kind {
            Tok::LParen => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return not_read(Risk::danger(Class::Unknown, Danger::TooComplex));
                }
                depth - 1
            }
            Tok::RParen => {
                depth = depth.checked_sub(1).ok_or(Unread::Syntax)?;
                depth
            }
            Tok::Semi if depth > 0 => return Err(Unread::Syntax),
            _ => depth,
        };
        let up = if matches!(t.kind, Tok::Ident | Tok::Keyword) { text.to_ascii_uppercase() } else { String::new() };
        let here = out.len();
        let mut mate = 0;
        if t.kind == Tok::LParen {
            open.push(here);
        } else if t.kind == Tok::RParen {
            let o = open.pop().ok_or(Unread::Syntax)?;
            mate = o as isize - here as isize;
            out[o].mate = -mate;
        }
        out.push(W { kind: t.kind, text, up, depth: at, mate });
    }
    if depth != 0 {
        return Err(Unread::Syntax);
    }
    Ok((out, hint))
}

/// Whether optimizer hint `text` has a `SET_VAR` of a setting of [`RISKY_SETTINGS`] or
/// [`READ_ONLY_SETTINGS`].
fn sets_risky(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.match_indices("set_var").any(|(at, _)| {
        let rest = lower[at + "set_var".len()..].trim_start();
        let Some(rest) = rest.strip_prefix('(') else { return false };
        let name: String = rest.trim_start().chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        RISKY_SETTINGS.contains(&name.as_str()) || READ_ONLY_SETTINGS.contains(&name.as_str())
    })
}

/// Whether quoted token `text` (a string, a quoted name, a quoted variable name) ends with its
/// closing quote, as the lexer reads it (an unterminated one runs to the end of the text).
fn closed(text: &str, kind: Tok, backslash: bool) -> bool {
    let body = match kind {
        Tok::Variable => text.trim_start_matches('@'),
        Tok::Str if text.starts_with('$') => {
            let Some(end) = text[1..].find('$') else { return false };
            let tag = &text[..end + 2];
            return text.len() >= 2 * tag.len() && text.ends_with(tag);
        }
        Tok::Str => &text[text.find(['\'', '"']).unwrap_or(0)..],
        _ => text,
    };
    let Some(q @ ('\'' | '"' | '`')) = body.chars().next() else { return kind != Tok::Str && kind != Tok::QuotedIdent };
    let escapes = backslash && q != '`';
    let mut chars = body[1..].char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if escapes && c == '\\' {
            if chars.next().is_none() {
                return false;
            }
        } else if c == q {
            if chars.peek().is_some_and(|&(_, n)| n == q) {
                chars.next();
            } else {
                return 1 + i + 1 == body.len();
            }
        }
    }
    false
}

/// The kinds of object `CREATE` and `ALTER` name.
const OBJECTS: &[&str] = &[
    "DATABASE",
    "EVENT",
    "FUNCTION",
    "INDEX",
    "INSTANCE",
    "LOGFILE",
    "PROCEDURE",
    "RESOURCE",
    "ROLE",
    "SCHEMA",
    "SERVER",
    "SPATIAL",
    "TABLE",
    "TABLESPACE",
    "TRIGGER",
    "UNDO",
    "USER",
    "VIEW",
];

/// The index of the kind of object a `CREATE` or `ALTER` names: the first word of
/// [`OBJECTS`] after its options (`OR REPLACE`, `TEMPORARY`, `DEFINER = …`, `ALGORITHM = …`,
/// `SQL SECURITY …`, `UNIQUE`, `AGGREGATE`, …), outside parentheses.
fn object(ws: &[W]) -> Option<usize> {
    if !ws.first().is_some_and(|w| w.is("CREATE") || w.is("ALTER")) {
        return None;
    }
    (1..ws.len()).find(|&i| ws[i].depth == 0 && OBJECTS.contains(&ws[i].up.as_str()))
}

/// Whether the text starts with `CREATE` of a stored routine, a trigger or an event, whose
/// body may hold `;`.
fn starts_compound(ws: &[W]) -> bool {
    let first = ws.iter().position(|w| w.kind == Tok::Semi).map_or(ws, |end| &ws[..end]);
    ws.first().is_some_and(|w| w.is("CREATE"))
        && object(first).is_some_and(|i| matches!(first[i].up.as_str(), "PROCEDURE" | "FUNCTION" | "TRIGGER" | "EVENT"))
}

/// Where the `CREATE` of a routine, a trigger or an event at the start of `ws` ends: the `;` after
/// its body, which is one statement or a block (`BEGIN … END`, also nested, with `IF`, `CASE`,
/// `LOOP`, `REPEAT` and `WHILE` inside); the end of the text; or nowhere it can tell (an `END`
/// that closes no block, a block not closed).
fn routine_end(ws: &[W]) -> RoutineEnd {
    let mut blocks = 0usize;
    let mut start = false;
    let mut i = 0;
    while i < ws.len() {
        let w = &ws[i];
        if w.kind == Tok::Semi {
            if blocks == 0 {
                return RoutineEnd::At(i);
            }
            start = true;
            i += 1;
            continue;
        }
        let next = ws.get(i + 1);
        if w.is("END") {
            let Some(left) = blocks.checked_sub(1) else { return RoutineEnd::Lost };
            blocks = left;
            if next.is_some_and(|n| ["IF", "CASE", "LOOP", "REPEAT", "WHILE"].iter().any(|k| n.is(k))) {
                i += 1;
            }
            start = false;
            i += 1;
            continue;
        }
        let opens = match w.up.as_str() {
            "BEGIN" | "CASE" => true,
            "IF" | "WHILE" | "LOOP" => start,
            "REPEAT" => !next.is_some_and(|n| n.kind == Tok::LParen),
            _ => false,
        };
        if opens {
            blocks += 1;
        }
        let label = w.is_op(":") && i > 0 && ws[i - 1].is_name();
        start = label || ["BEGIN", "THEN", "ELSE", "DO", "LOOP", "REPEAT"].iter().any(|k| w.is(k));
        i += 1;
    }
    if blocks == 0 { RoutineEnd::Whole } else { RoutineEnd::Lost }
}

/// Where a routine's `CREATE` ends ([`routine_end`]).
enum RoutineEnd {
    /// At the `;` at this index; statements follow.
    At(usize),
    /// At the end of the text.
    Whole,
    /// Its blocks cannot be followed.
    Lost,
}

/// The index of the parenthesis that closes the one at `i` (or opens the one at `i`), when both
/// are in `ws`.
fn mate(ws: &[W], i: usize) -> Option<usize> {
    let w = ws.get(i).filter(|w| w.mate != 0)?;
    let j = usize::try_from(i as isize + w.mate).ok()?;
    (j < ws.len()).then_some(j)
}

/// A name (possibly qualified) starting at `i`, as written, and the index after it.
fn name_at(ws: &[W], i: usize) -> Option<(String, usize)> {
    let first = ws.get(i).filter(|w| w.is_name())?;
    let mut name = first.text.to_string();
    let mut j = i + 1;
    while ws.get(j).is_some_and(|w| w.kind == Tok::Dot) && ws.get(j + 1).is_some_and(|w| w.is_name() || w.is_op("*")) {
        name.push('.');
        name.push_str(ws[j + 1].text);
        j += 2;
    }
    Some((name, j))
}

/// The index of the first word of `ws` from `from` on (at depth 0) that is one of `words`.
fn find_top(ws: &[W], from: usize, words: &[&str]) -> Option<usize> {
    (from..ws.len()).find(|&i| ws[i].depth == 0 && words.contains(&ws[i].up.as_str()))
}

/// The function calls of `ws`.
fn calls(ws: &[W]) -> Vec<Call> {
    let mut out = Vec::new();
    for i in 0..ws.len().saturating_sub(1) {
        let w = &ws[i];
        // The statement's own first word, and the procedure of a `CALL`, are no function.
        if i == 0 || ws[i + 1].kind != Tok::LParen || !w.is_name() || (i == 1 && ws[0].is("CALL")) {
            continue;
        }
        let name = if w.kind == Tok::QuotedIdent { unquote(w.text).to_ascii_uppercase() } else { w.up.clone() };
        let prev = i.checked_sub(1).map(|p| &ws[p]);
        let qualified = prev.is_some_and(|p| p.kind == Tok::Dot);
        if !qualified
            && w.kind != Tok::QuotedIdent
            && (NOT_CALLS.contains(&name.as_str()) || syntax_before_paren(ws, i))
        {
            continue;
        }
        if !qualified && prev.is_some_and(|p| BEFORE_COLUMN_LIST.iter().any(|b| p.is(b))) {
            continue;
        }
        // A common table expression's column list: `name (a, b) AS (…)`.
        if let Some(close) = mate(ws, i + 1)
            && ws.get(close + 1).is_some_and(|w| w.is("AS"))
            && ws.get(close + 2).is_some_and(|w| w.kind == Tok::LParen)
        {
            continue;
        }
        out.push(Call { name, qualified });
    }
    out
}

/// Whether the non-reserved word at `i`, before `(`, is syntax in its place: `= ANY (…)`,
/// `> SOME (…)`, `MATCH (…) AGAINST (…)`, `'$' COLUMNS (…)` of `JSON_TABLE`, `LIKE 'x' ESCAPE
/// (…)`.
fn syntax_before_paren(ws: &[W], i: usize) -> bool {
    let prev = i.checked_sub(1).map(|p| &ws[p]);
    // A derived table's column list: `(SELECT …) AS d (a, b)` without the `AS`.
    if prev.is_some_and(|p| p.kind == Tok::RParen) {
        return true;
    }
    match ws[i].up.as_str() {
        "ANY" | "SOME" => prev.is_some_and(|p| p.kind == Tok::Op),
        "AGAINST" => prev.is_some_and(|p| p.kind == Tok::RParen),
        "COLUMNS" | "ESCAPE" => prev.is_some_and(|p| p.kind == Tok::Str),
        // The type of `CONVERT(x, type)` and of `… RETURNING type` (`JSON_VALUE`).
        t if CAST_TYPES.contains(&t) => {
            prev.is_some_and(|p| p.is("RETURNING"))
                || (prev.is_some_and(|p| p.kind == Tok::Comma) && opener(ws, i).is_some_and(|o| o.is("CONVERT")))
        }
        _ => false,
    }
}

/// The types `CAST`, `CONVERT` and `RETURNING` take, with a precision in parentheses.
const CAST_TYPES: &[&str] = &["BINARY", "CHAR", "DATETIME", "DECIMAL", "DOUBLE", "FLOAT", "NCHAR", "TIME"];

/// The word before the parenthesis that encloses the token at `i`.
fn opener<'c, 'a>(ws: &'c [W<'a>], i: usize) -> Option<&'c W<'a>> {
    let depth = ws[i].depth.checked_sub(1)?;
    let open = (0..i).rev().find(|&j| ws[j].kind == Tok::LParen && ws[j].depth == depth)?;
    open.checked_sub(1).map(|j| &ws[j])
}

/// A quoted name's text without its quotes.
fn unquote(text: &str) -> String {
    let q = text.chars().next().unwrap_or('`');
    let inner = text.get(1..text.len().saturating_sub(1)).unwrap_or("");
    inner.replace(&format!("{q}{q}"), &q.to_string())
}

/// What the function calls of `ws` add to `risk`. In a statement that changes the schema, only
/// the calls that act on the server count (a column definition or an index's key part looks
/// like a call too, and such a statement is no read anyway).
fn with_calls(mut risk: Risk, ws: &[W]) -> Risk {
    let schema = risk.class == Class::Ddl;
    for c in calls(ws) {
        if c.builtin() {
            let danger = if SERVER_FILES.contains(&c.name.as_str()) {
                Some(Danger::ServerFile)
            } else if SERVER_ACTIONS.contains(&c.name.as_str()) {
                Some(Danger::ServerAction)
            } else {
                None
            };
            risk.danger = risk.danger.or(danger);
        } else if !schema {
            risk.runs_code = true;
            risk.unchecked_call |= !c.qualified;
        }
    }
    risk
}

/// The risk of one statement (its tokens).
fn statement(ws: &[W]) -> Risk {
    let mut risk = kind(ws);
    // A locking read anywhere (a subquery of a `SET`, a `SHOW … WHERE`, …) takes row locks; what
    // `EXPLAIN` only plans takes none.
    if risk.class < Class::Write && risk.explain != Explain::Plan && locks(ws) && !routine_body(ws) {
        risk.class = Class::Write;
    }
    let risk = if routine_body(ws) { risk } else { with_calls(risk, ws) };
    let harmless = matches!(risk.class, Class::Read | Class::Session | Class::Tx);
    if harmless && has_write_word(ws) { unrecognized() } else { risk }
}

/// Whether `ws` creates or changes a routine, a trigger, an event or a view: the body is stored,
/// not run.
fn routine_body(ws: &[W]) -> bool {
    object(ws).is_some_and(|i| matches!(ws[i].up.as_str(), "PROCEDURE" | "FUNCTION" | "TRIGGER" | "EVENT" | "VIEW"))
}

/// Whether a word of [`WRITE_WORDS`] appears in `ws` past its first word, other than where a
/// read uses it (`FOR UPDATE`, `LOCK IN SHARE MODE`, the functions `INSERT()` and `REPLACE()`,
/// `SHOW CREATE …`) or as a name after a `.`.
fn has_write_word(ws: &[W]) -> bool {
    (1..ws.len()).any(|i| {
        let w = &ws[i];
        if !WRITE_WORDS.contains(&w.up.as_str()) || ws[i - 1].kind == Tok::Dot {
            return false;
        }
        let next = ws.get(i + 1);
        let allowed = (w.is("UPDATE") && ws[i - 1].is("FOR"))
            || ((w.is("INSERT") || w.is("REPLACE")) && next.is_some_and(|n| n.kind == Tok::LParen))
            || (w.is("LOCK") && next.is_some_and(|n| n.is("IN")))
            || (w.is("CREATE") && ws[i - 1].is("SHOW"));
        !allowed
    })
}

/// The risk of one statement by its form, before its calls.
fn kind(ws: &[W]) -> Risk {
    let Some(first) = ws.first() else { return Risk::of(Class::Unknown) };
    let word = |i: usize| ws.get(i).map_or("", |w| w.up.as_str());
    if first.kind == Tok::LParen {
        return query(ws);
    }
    match first.up.as_str() {
        "SELECT" | "TABLE" | "VALUES" => query(ws),
        "WITH" => with(ws),
        "SHOW" | "HELP" => Risk::of(Class::Read),
        "EXPLAIN" | "DESCRIBE" | "DESC" => explain(ws),
        "DO" => {
            let code = calls(ws).iter().any(|c| !c.builtin());
            if code {
                Risk { runs_code: true, ..Risk::danger(Class::Procedural, Danger::Procedural) }
            } else {
                query(ws)
            }
        }
        "USE" => Risk::session(true),
        "SET" => set(ws),
        "INSERT" | "REPLACE" => insert(ws),
        "UPDATE" => update(ws, 0),
        "DELETE" => delete(ws, 0),
        "CREATE" => create(ws),
        "ALTER" => alter(ws),
        "DROP" => drop(ws),
        "RENAME" => {
            let mut r = ddl(Some(Danger::Rename));
            r.target = name_at(ws, 2).map(|(n, _)| n);
            r
        }
        "TRUNCATE" => {
            let at = if word(1) == "TABLE" { 2 } else { 1 };
            let mut r = ddl(Some(Danger::Truncate));
            r.writes = true;
            r.target = name_at(ws, at).map(|(n, _)| n);
            r
        }
        "GRANT" | "REVOKE" => ddl(Some(Danger::Privileges)),
        "START" if word(1) == "TRANSACTION" => {
            let read_write = (2..ws.len().saturating_sub(1)).any(|i| ws[i].is("READ") && ws[i + 1].is("WRITE"));
            Risk { read_write, implicit_commit: true, ..Risk::of(Class::Tx) }
        }
        "BEGIN" if ws.len() == 1 || (ws.len() == 2 && word(1) == "WORK") => {
            Risk { implicit_commit: true, ..Risk::of(Class::Tx) }
        }
        "COMMIT" | "ROLLBACK" | "SAVEPOINT" => Risk::of(Class::Tx),
        "RELEASE" if word(1) == "SAVEPOINT" => Risk::of(Class::Tx),
        "UNLOCK" if matches!(word(1), "TABLE" | "TABLES" | "INSTANCE") => Risk::of(Class::Tx),
        "LOCK" if matches!(word(1), "TABLE" | "TABLES") => {
            Risk { implicit_commit: true, ..Risk::danger(Class::Write, Danger::Locks) }
        }
        "LOCK" if word(1) == "INSTANCE" => Risk::danger(Class::Maintenance, Danger::Locks),
        "HANDLER" => Risk::danger(Class::Write, Danger::Locks),
        "XA" if word(1) == "RECOVER" => Risk::of(Class::Read),
        "XA" => Risk::danger(Class::Tx, Danger::ServerCommand),
        "CALL" => Risk { runs_code: true, ..Risk::danger(Class::Procedural, Danger::Procedural) },
        "PREPARE" => Risk::danger(Class::Session, Danger::DynamicSql),
        "EXECUTE" => Risk::danger(Class::Unknown, Danger::UnknownPrepared),
        "DEALLOCATE" if word(1) == "PREPARE" => Risk::session(true),
        "LOAD" if matches!(word(1), "DATA" | "XML") => {
            let target = find_top(ws, 1, &["TABLE"]).and_then(|i| name_at(ws, i + 1)).map(|(n, _)| n);
            Risk { danger: Some(Danger::FileAccess), ..Risk::writing(Class::Write, target) }
        }
        "LOAD" if word(1) == "INDEX" => maintenance(None),
        "ANALYZE" | "OPTIMIZE" | "CHECK" | "CHECKSUM" | "REPAIR" | "CACHE" => maintenance(None),
        "FLUSH" => {
            let locks = ws.windows(3).any(|w| w[0].is("WITH") && w[1].is("READ") && w[2].is("LOCK"))
                || ws.windows(2).any(|w| w[0].is("FOR") && w[1].is("EXPORT"));
            maintenance(Some(if locks { Danger::Locks } else { Danger::ServerCommand }))
        }
        "KILL" | "SHUTDOWN" | "RESTART" | "INSTALL" | "UNINSTALL" | "CLONE" | "RESET" | "PURGE" | "CHANGE" | "STOP"
        | "START" => maintenance(Some(Danger::ServerCommand)),
        "BINLOG" => Risk::danger(Class::Unknown, Danger::ServerCommand),
        "IMPORT" if word(1) == "TABLE" => ddl(None),
        "GET" if matches!(word(1), "DIAGNOSTICS" | "CURRENT" | "STACKED") => Risk::of(Class::Read),
        "SIGNAL" | "RESIGNAL" => Risk::of(Class::Read),
        _ => unrecognized(),
    }
}

/// A statement that changes the schema, committing the open transaction first.
fn ddl(danger: Option<Danger>) -> Risk {
    Risk { danger, implicit_commit: true, ..Risk::of(Class::Ddl) }
}

/// A maintenance statement, committing the open transaction first.
fn maintenance(danger: Option<Danger>) -> Risk {
    Risk { danger, implicit_commit: true, ..Risk::of(Class::Maintenance) }
}

/// Whether `ws` has a locking clause: `FOR UPDATE`, `FOR SHARE`, `LOCK IN SHARE MODE`.
fn locks(ws: &[W]) -> bool {
    ws.windows(2).any(|w| w[0].is("FOR") && (w[1].is("UPDATE") || w[1].is("SHARE")))
        || ws.windows(4).any(|w| w[0].is("LOCK") && w[1].is("IN") && w[2].is("SHARE") && w[3].is("MODE"))
}

/// A query (`SELECT`, `TABLE`, `VALUES`, in parentheses or not, with `WITH` before it).
fn query(ws: &[W]) -> Risk {
    let locks = locks(ws);
    let class = if locks { Class::Write } else { Class::Read };
    let intos: Vec<usize> = (0..ws.len()).filter(|&i| ws[i].is("INTO")).collect();
    if intos.iter().any(|&i| ws.get(i + 1).is_some_and(|w| w.is("OUTFILE") || w.is("DUMPFILE"))) {
        return Risk::danger(Class::Write, Danger::FileAccess);
    }
    let into = match intos.as_slice() {
        [] => return Risk::of(class),
        [into] => *into,
        _ => return unrecognized(),
    };
    // `INTO @a, @b`: only the session's user variables.
    let mut i = into + 1;
    loop {
        if !ws.get(i).is_some_and(|w| w.kind == Tok::Variable && !w.text.starts_with("@@")) {
            return unrecognized();
        }
        if ws.get(i + 1).is_some_and(|w| w.kind == Tok::Comma) {
            i += 2;
        } else {
            break;
        }
    }
    if locks { Risk::of(Class::Write) } else { Risk::session(true) }
}

/// The index of the statement a `WITH …` is for, after its common table expressions.
fn with_main(ws: &[W]) -> Option<usize> {
    let mut i = 1;
    if ws.get(i).is_some_and(|w| w.is("RECURSIVE")) {
        i += 1;
    }
    loop {
        if !ws.get(i).is_some_and(|w| w.is_name()) {
            return None;
        }
        i += 1;
        if ws.get(i).is_some_and(|w| w.kind == Tok::LParen) {
            i = mate(ws, i)? + 1;
        }
        if !ws.get(i).is_some_and(|w| w.is("AS")) || !ws.get(i + 1).is_some_and(|w| w.kind == Tok::LParen) {
            return None;
        }
        i = mate(ws, i + 1)? + 1;
        if ws.get(i).is_some_and(|w| w.kind == Tok::Comma) {
            i += 1;
        } else {
            return Some(i);
        }
    }
}

/// Whether the token at `i` starts a query: `SELECT`, `TABLE`, `VALUES`, `(`.
fn starts_query(ws: &[W], i: usize) -> bool {
    ws.get(i).is_some_and(|w| w.kind == Tok::LParen || w.is("SELECT") || w.is("TABLE") || w.is("VALUES"))
}

/// `WITH …` and the statement it is for (a query, `UPDATE` or `DELETE`).
fn with(ws: &[W]) -> Risk {
    match with_main(ws) {
        Some(i) if starts_query(ws, i) => query(ws),
        Some(i) if ws.get(i).is_some_and(|w| w.is("UPDATE")) => update(ws, i),
        Some(i) if ws.get(i).is_some_and(|w| w.is("DELETE")) => delete(ws, i),
        _ => unrecognized(),
    }
}

/// `EXPLAIN`, `DESCRIBE`, `DESC`: of a statement, of a table, or of another connection.
fn explain(ws: &[W]) -> Risk {
    let mut i = 1;
    let mut analyze = false;
    let mut into = false;
    loop {
        match ws.get(i) {
            Some(w) if w.is("ANALYZE") => {
                analyze = true;
                i += 1;
            }
            Some(w) if w.is("EXTENDED") || w.is("PARTITIONS") => i += 1,
            Some(w) if w.is("FORMAT") && ws.get(i + 1).is_some_and(|w| w.is_op("=")) => i += 3,
            Some(w) if w.is("INTO") && ws.get(i + 1).is_some_and(|w| w.kind == Tok::Variable) => {
                into = true;
                i += 2;
            }
            Some(w) if w.is("FOR") && ws.get(i + 1).is_some_and(|w| w.is("SCHEMA") || w.is("DATABASE")) => i += 3,
            _ => break,
        }
    }
    let Some(next) = ws.get(i) else { return unrecognized() };
    if next.is("FOR") && ws.get(i + 1).is_some_and(|w| w.is("CONNECTION")) {
        return Risk::of(Class::Read);
    }
    let rest = &ws[i..];
    let statement = next.kind == Tok::LParen
        || ["SELECT", "TABLE", "VALUES", "WITH", "INSERT", "REPLACE", "UPDATE", "DELETE"].iter().any(|k| next.is(k));
    if !statement {
        // A table, and a column of it or a pattern of its columns' names.
        let Some((_, after)) = name_at(ws, i).filter(|_| !analyze && !into) else { return unrecognized() };
        return match &ws[after..] {
            [] => Risk::of(Class::Read),
            [w] if w.is_name() || w.kind == Tok::Str => Risk::of(Class::Read),
            _ => unrecognized(),
        };
    }
    let inner = kind(rest);
    let mut risk = if analyze {
        Risk { explain: Explain::Analyze, ..inner }
    } else {
        // Only planned: a query's locks are not taken. MySQL refuses the plan of a write in a
        // read-only transaction, so that is one that changes no rows.
        let dml = ["INSERT", "REPLACE", "UPDATE", "DELETE"].iter().any(|k| next.is(k))
            || (next.is("WITH") && with_main(rest).is_some_and(|m| !starts_query(rest, m)));
        match inner.danger {
            Some(Danger::Unrecognized | Danger::TooComplex | Danger::ExecutableComment | Danger::FileAccess) => {
                Risk { explain: Explain::Plan, ..inner }
            }
            _ => Risk { explain: Explain::Plan, ..Risk::of(if dml { Class::Write } else { Class::Read }) },
        }
    };
    if into && risk.class == Class::Read {
        // The plan goes into a user variable.
        risk = Risk { class: Class::Session, safe_setting: true, ..risk };
    } else if into {
        risk = worse(risk, Risk::session(false));
    }
    risk
}

/// `INSERT` or `REPLACE`.
fn insert(ws: &[W]) -> Risk {
    let mut i = 1;
    while ws
        .get(i)
        .is_some_and(|w| ["LOW_PRIORITY", "DELAYED", "HIGH_PRIORITY", "IGNORE", "INTO"].iter().any(|k| w.is(k)))
    {
        i += 1;
    }
    Risk::writing(Class::Write, name_at(ws, i).map(|(n, _)| n))
}

/// `UPDATE` (its keyword at `at`).
fn update(ws: &[W], at: usize) -> Risk {
    let mut i = at + 1;
    while ws.get(i).is_some_and(|w| w.is("LOW_PRIORITY") || w.is("IGNORE")) {
        i += 1;
    }
    let target = name_at(ws, i).map(|(n, _)| n);
    let Some(set) = find_top(ws, i, &["SET"]) else { return unrecognized() };
    let tables = &ws[i..set];
    let multi = tables.iter().any(|w| w.depth == 0 && (w.kind == Tok::Comma || w.is("JOIN")));
    let targets = if multi { set_targets(ws, set) } else { None };
    let no_where = filter(ws, set).or_else(|| targets.and_then(|t| other_tables(&t, tables, ws, set)));
    let danger = no_where.map(|_| Danger::UpdateAll);
    Risk { no_where, danger, ..Risk::writing(Class::Write, target) }
}

/// `DELETE` (its keyword at `at`), single- or multi-table.
fn delete(ws: &[W], at: usize) -> Risk {
    let mut i = at + 1;
    while ws.get(i).is_some_and(|w| ["LOW_PRIORITY", "QUICK", "IGNORE"].iter().any(|k| w.is(k))) {
        i += 1;
    }
    let from = ws.get(i).is_some_and(|w| w.is("FROM"));
    if from {
        i += 1;
    }
    let target = name_at(ws, i).map(|(n, _)| n);
    // `DELETE t1, t2 FROM …` or `DELETE FROM t1, t2 USING …`: the tables it deletes from, and
    // where the tables it reads are named.
    let using = find_top(ws, i, &["USING"]).filter(|&u| ws.get(u + 1).is_some_and(|w| w.kind != Tok::LParen));
    let multi = match (from, using) {
        (true, Some(u)) => Some((name_list(&ws[i..u]), u + 1)),
        (false, _) => find_top(ws, i, &["FROM"]).map(|f| (name_list(&ws[i..f]), f + 1)),
        (true, None) => None,
    };
    let no_where = filter(ws, i).or_else(|| {
        let (targets, tables) = multi?;
        let end = find_top(ws, tables, &["WHERE"]).unwrap_or(ws.len());
        other_tables(&targets?, &ws[tables..end], ws, tables)
    });
    let danger = no_where.map(|_| Danger::DeleteAll);
    Risk { no_where, danger, ..Risk::writing(Class::Write, target) }
}

/// The names of a list (`t1, db.t2, t3.*`), each the last part of the name in upper case and
/// without quotes; `None` when an item is not a name.
fn name_list(ws: &[W]) -> Option<Vec<String>> {
    split_commas(ws)
        .iter()
        .map(|item| {
            let item = match item {
                [rest @ .., dot, star] if dot.kind == Tok::Dot && star.is_op("*") => rest,
                _ => item,
            };
            let (_, next) = name_at(item, 0)?;
            (next == item.len()).then(|| key(&item[next - 1]))
        })
        .collect()
}

/// A name in upper case and without quotes.
fn key(w: &W) -> String {
    if w.kind == Tok::QuotedIdent { unquote(w.text).to_ascii_uppercase() } else { w.up.clone() }
}

/// The tables a multi-table `UPDATE` changes: the qualifiers of the columns its `SET` (at
/// `set`) assigns; `None` when one is not qualified (it could be any table's).
fn set_targets(ws: &[W], set: usize) -> Option<Vec<String>> {
    let end = find_top(ws, set + 1, &["WHERE", "ORDER", "LIMIT"]).unwrap_or(ws.len());
    split_commas(&ws[set + 1..end])
        .iter()
        .map(|item| {
            let eq = item.iter().position(|w| w.is_op("="))?;
            match &item[..eq] {
                [.., q, dot, c] if dot.kind == Tok::Dot && q.is_name() && c.is_name() => Some(key(q)),
                _ => None,
            }
        })
        .collect()
}

/// Whether a multi-table statement whose `WHERE` names a column reads no column of the tables
/// it changes (`targets`) in its `WHERE` (looked for from `from` on) or in the joins of its
/// table list (`tables`): every column it names there is qualified with another table's name.
/// `None` when it reads one of them, or may (an unqualified column, a join `USING (…)`).
fn other_tables(targets: &[String], tables: &[W], ws: &[W], from: usize) -> Option<NoWhere> {
    let at = find_top(ws, from, &["WHERE"])?;
    let end = find_top(ws, at + 1, &["ORDER", "LIMIT"]).unwrap_or(ws.len());
    let cond = &ws[at + 1..end];
    if tables.windows(2).any(|w| w[0].is("USING") && w[1].kind == Tok::LParen) {
        return None;
    }
    let joins = tables.iter().position(|w| w.is("ON")).map_or(&tables[..0], |on| &tables[on..]);
    let mut qualifiers = Vec::new();
    for (in_where, part) in [(true, cond), (false, joins)] {
        for i in 0..part.len() {
            let w = &part[i];
            let dotted = |j: usize| part.get(j).is_some_and(|d| d.kind == Tok::Dot);
            if !w.is_name() || part.get(i + 1).is_some_and(|n| n.kind == Tok::LParen) {
                continue;
            }
            if dotted(i + 1) && part.get(i + 2).is_some_and(|n| n.is_name()) && !dotted(i + 3) {
                // `q.c`: the qualifier.
                qualifiers.push(key(w));
            } else if in_where && !dotted(i + 1) && (i == 0 || !dotted(i - 1)) && names_a_column(&part[i..=i]) {
                // An unqualified column: it may be a target's.
                return None;
            }
        }
    }
    (!qualifiers.iter().any(|q| targets.contains(q))).then_some(NoWhere::OtherTables)
}

/// Whether the `WHERE` of an `UPDATE` or `DELETE` (looked for from `from` on) may let every row
/// through.
fn filter(ws: &[W], from: usize) -> Option<NoWhere> {
    let Some(at) = find_top(ws, from, &["WHERE"]) else { return Some(NoWhere::Missing) };
    let end = find_top(ws, at + 1, &["ORDER", "LIMIT"]).unwrap_or(ws.len());
    let cond = &ws[at + 1..end];
    if always_true(cond, 0) {
        return Some(NoWhere::AlwaysTrue);
    }
    (!names_a_column(cond)).then_some(NoWhere::NoColumn)
}

/// Whether condition `c` names a column (or anything else that may be one).
fn names_a_column(c: &[W]) -> bool {
    let mut interval = false;
    (0..c.len()).any(|i| {
        let w = &c[i];
        interval |= w.is("INTERVAL");
        let call = c.get(i + 1).is_some_and(|n| n.kind == Tok::LParen);
        let after = |word: &str| i > 0 && c[i - 1].is(word);
        match w.kind {
            Tok::QuotedIdent => !call,
            Tok::Ident | Tok::Keyword => {
                let introducer = w.text.starts_with('_') && c.get(i + 1).is_some_and(|n| n.kind == Tok::Str);
                let unit = interval && UNITS.contains(&w.up.as_str());
                !call && !introducer && !unit && !after("COLLATE") && !NOT_COLUMNS.contains(&w.up.as_str())
            }
            _ => false,
        }
    })
}

/// `c` without the parentheses around all of it.
fn unwrap<'c, 'a>(mut c: &'c [W<'a>]) -> &'c [W<'a>] {
    while c.len() >= 2 && c[0].kind == Tok::LParen && mate(c, 0) == Some(c.len() - 1) {
        c = &c[1..c.len() - 1];
    }
    c
}

/// `c` split at its top-level `OR` (and `||`), or `AND` (and `&&`).
fn split_logic<'c, 'a>(c: &'c [W<'a>], or: bool) -> Vec<&'c [W<'a>]> {
    let depth = c.first().map_or(0, |w| w.depth);
    let (word, op) = if or { ("OR", "|") } else { ("AND", "&") };
    let mut parts = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < c.len() {
        let doubled = c[i].is_op(op) && c.get(i + 1).is_some_and(|n| n.is_op(op));
        if c[i].depth == depth && (c[i].is(word) || doubled) {
            parts.push(&c[start..i]);
            i += if doubled { 2 } else { 1 };
            start = i;
        } else {
            i += 1;
        }
    }
    parts.push(&c[start..]);
    parts
}

/// `c` split at its top-level commas.
fn split_commas<'c, 'a>(c: &'c [W<'a>]) -> Vec<&'c [W<'a>]> {
    let depth = c.first().map_or(0, |w| w.depth);
    c.split(|w| w.kind == Tok::Comma && w.depth == depth).collect()
}

/// Whether condition `c` is recognised as always true (see the module docs). Gives up past a
/// small nesting.
fn always_true(c: &[W], nesting: usize) -> bool {
    if nesting > 32 {
        return false;
    }
    let c = unwrap(c);
    let ors = split_logic(c, true);
    if ors.len() > 1 {
        return ors.iter().any(|p| always_true(p, nesting + 1));
    }
    let ands = split_logic(c, false);
    if ands.len() > 1 {
        return ands.iter().all(|p| always_true(p, nesting + 1));
    }
    match c {
        [w] if w.is("TRUE") => true,
        [w] if w.kind == Tok::Number => w.text.parse::<f64>().is_ok_and(|n| n != 0.0),
        [n, w] if n.is("NOT") => w.is("FALSE") || (w.kind == Tok::Number && w.text.parse::<f64>() == Ok(0.0)),
        _ => same_sides(c),
    }
}

/// Whether `c` is `x = x` or `x <=> x` (the same tokens on both sides).
fn same_sides(c: &[W]) -> bool {
    let depth = c.first().map_or(0, |w| w.depth);
    let Some(eq) = (0..c.len()).find(|&i| c[i].depth == depth && c[i].is_op("=")) else { return false };
    let (mut left, mut right) = (&c[..eq], &c[eq + 1..]);
    let before = left.last();
    if before.is_some_and(|w| w.is_op("<")) && right.first().is_some_and(|w| w.is_op(">")) {
        left = &left[..left.len() - 1];
        right = &right[1..];
    } else if before.is_some_and(|w| w.kind == Tok::Op) || right.first().is_some_and(|w| w.kind == Tok::Op) {
        return false;
    }
    // A name, quoted or not, in upper case; any other token as written.
    let same = |a: &W, b: &W| match (a.is_name(), b.is_name()) {
        (true, true) => key(a) == key(b),
        (false, false) => a.kind == b.kind && a.text == b.text,
        _ => false,
    };
    !left.is_empty() && left.len() == right.len() && left.iter().zip(right).all(|(a, b)| same(a, b))
}

/// `CREATE …`.
fn create(ws: &[W]) -> Risk {
    let Some(i) = object(ws) else { return unrecognized() };
    let temporary = ws[1..i].iter().any(|w| w.is("TEMPORARY"));
    let object = ws[i].up.as_str();
    if ws.iter().any(|w| w.is("SONAME")) {
        // A loadable function: the server loads a shared library.
        return maintenance(Some(Danger::ServerCommand));
    }
    match object {
        "USER" | "ROLE" => ddl(Some(Danger::Privileges)),
        "INSTANCE" => unrecognized(),
        "EVENT" => with_event_body(ddl(None), ws, i),
        "TABLE" => {
            let named = name_at(ws, i + 1 + if_not_exists(ws, i + 1));
            let after = named.as_ref().map_or(ws.len(), |(_, next)| *next);
            // `CREATE TABLE … [AS] SELECT` (or `TABLE`, `VALUES`, `WITH`, in parentheses or not)
            // copies rows.
            let copies = (after..ws.len()).any(|j| {
                ws[j].depth == 0 && (["SELECT", "WITH", "VALUES", "TABLE"].iter().any(|k| ws[j].is(k)))
                    || (ws[j].kind == Tok::LParen
                        && ws[j].depth == 0
                        && ws.get(j + 1).is_some_and(|w| w.is("SELECT") || w.is("WITH")))
            });
            Risk { writes: copies, target: named.map(|(n, _)| n), implicit_commit: !temporary, ..Risk::of(Class::Ddl) }
        }
        _ => ddl(None),
    }
}

/// An event's body (after its `DO`, from `from` on) runs later on its own, at once for one
/// scheduled `AT CURRENT_TIMESTAMP`: a statement's danger is the event's, and a block
/// (`BEGIN … END`) is code that is not read ([`Danger::Procedural`]).
fn with_event_body(mut risk: Risk, ws: &[W], from: usize) -> Risk {
    let Some(at) = find_top(ws, from, &["DO"]) else { return risk };
    let body = &ws[at + 1..];
    let first = body.first();
    let block = first.is_some_and(|w| w.is("BEGIN")) || body.get(1).is_some_and(|w| w.is_op(":"));
    // An event cannot create or change an event (MySQL refuses it): not read further.
    let nested = object(body).is_some_and(|i| body[i].is("EVENT"));
    let inner = if nested {
        unrecognized()
    } else if block {
        Risk::danger(Class::Procedural, Danger::Procedural)
    } else {
        statement(body)
    };
    // What the body does beats a name changed.
    if matches!(risk.danger, None | Some(Danger::Rename)) && inner.danger.is_some() {
        risk.danger = inner.danger;
        risk.no_where = inner.no_where;
    }
    risk
}

/// How many words of `IF NOT EXISTS` start at `i` (0 or 3).
fn if_not_exists(ws: &[W], i: usize) -> usize {
    let seq = ["IF", "NOT", "EXISTS"];
    if (0..3).all(|k| ws.get(i + k).is_some_and(|w| w.is(seq[k]))) { 3 } else { 0 }
}

/// `ALTER …`.
fn alter(ws: &[W]) -> Risk {
    let Some(object) = object(ws) else { return unrecognized() };
    match ws[object].up.as_str() {
        "USER" => ddl(Some(Danger::Privileges)),
        "EVENT" => {
            let head = find_top(ws, object, &["DO"]).map_or(ws, |at| &ws[..at]);
            let renames = head.iter().any(|w| w.is("RENAME"));
            with_event_body(ddl(renames.then_some(Danger::Rename)), ws, object)
        }
        "INSTANCE" => maintenance(Some(Danger::ServerCommand)),
        "TABLE" => {
            let mut r = ddl(table_change(ws, object + 1));
            r.target = name_at(ws, object + 1).map(|(n, _)| n);
            r
        }
        _ => {
            let renames = ws.iter().any(|w| w.is("RENAME"));
            ddl(renames.then_some(Danger::Rename))
        }
    }
}

/// The worst change of an `ALTER TABLE`'s clauses (from `from` on), if it is dangerous.
fn table_change(ws: &[W], from: usize) -> Option<Danger> {
    let word = |i: usize| ws.get(i).filter(|w| w.depth == 0).map_or("", |w| w.up.as_str());
    let mut found: Option<Danger> = None;
    for (i, w) in ws.iter().enumerate().skip(from) {
        if w.depth != 0 {
            continue;
        }
        let danger = match w.up.as_str() {
            "DROP" => match word(i + 1) {
                // `ALTER [COLUMN] c DROP DEFAULT`.
                "DEFAULT" => None,
                "INDEX" | "KEY" | "PRIMARY" | "FOREIGN" | "CHECK" | "CONSTRAINT" | "PARTITION" => Some(Danger::Drop),
                _ => Some(Danger::DropColumn),
            },
            "MODIFY" | "CHANGE" => Some(Danger::AlterColumnType),
            "CONVERT" if word(i + 1) == "TO" => Some(Danger::AlterColumnType),
            "RENAME" => Some(Danger::Rename),
            // An engine that keeps no rows.
            "ENGINE" if ws[i + 1..].iter().take(2).any(|w| value_text(w).eq_ignore_ascii_case("BLACKHOLE")) => {
                Some(Danger::Truncate)
            }
            "TRUNCATE" if word(i + 1) == "PARTITION" => Some(Danger::Truncate),
            "DISCARD" | "EXCHANGE" | "REMOVE" if matches!(word(i + 1), "TABLESPACE" | "PARTITION" | "PARTITIONING") => {
                Some(Danger::Drop)
            }
            _ => None,
        };
        // Data lost beats a name changed.
        found = match (found, danger) {
            (Some(Danger::Rename), Some(d)) | (None, Some(d)) => Some(d),
            (f, _) => f,
        };
    }
    found
}

/// `DROP …`.
fn drop(ws: &[W]) -> Risk {
    let word = |i: usize| ws.get(i).map_or("", |w| w.up.as_str());
    if word(1) == "PREPARE" {
        return Risk::session(true);
    }
    let temporary = word(1) == "TEMPORARY";
    let at = if temporary { 2 } else { 1 };
    let target = name_at(ws, at + 1 + if_exists(ws, at + 1)).map(|(n, _)| n);
    Risk { target, implicit_commit: !temporary, ..ddl(Some(Danger::Drop)) }
}

/// How many words of `IF EXISTS` start at `i` (0 or 2).
fn if_exists(ws: &[W], i: usize) -> usize {
    if ws.get(i).is_some_and(|w| w.is("IF")) && ws.get(i + 1).is_some_and(|w| w.is("EXISTS")) { 2 } else { 0 }
}

/// `SET …`.
fn set(ws: &[W]) -> Risk {
    let word = |i: usize| ws.get(i).map_or("", |w| w.up.as_str());
    let scoped = matches!(word(1), "GLOBAL" | "PERSIST" | "PERSIST_ONLY" | "SESSION" | "LOCAL");
    let after_scope = if scoped { 2 } else { 1 };
    if word(after_scope) == "TRANSACTION" {
        if matches!(word(1), "GLOBAL" | "PERSIST" | "PERSIST_ONLY") {
            return maintenance(Some(Danger::ServerCommand));
        }
        let read_write = ws.windows(2).any(|w| w[0].is("READ") && w[1].is("WRITE"));
        return Risk { read_write, ..Risk::of(Class::Tx) };
    }
    match word(1) {
        "PASSWORD" => return ddl(Some(Danger::Privileges)),
        "DEFAULT" if word(2) == "ROLE" => return ddl(Some(Danger::Privileges)),
        "ROLE" if !ws.iter().any(|w| w.is_op("=")) => return Risk::session(true),
        "RESOURCE" => return maintenance(Some(Danger::ServerCommand)),
        _ => {}
    }
    // A scope keyword holds for the assignments after it that have none of their own. `NAMES`
    // and `CHARACTER SET` may come among the assignments.
    let mut global = false;
    let mut out: Option<Risk> = None;
    for item in split_commas(&ws[1..]) {
        if let Some(scope) = item.first().filter(|w| w.kind != Tok::Variable) {
            match scope.up.as_str() {
                "GLOBAL" | "PERSIST" | "PERSIST_ONLY" => global = true,
                "SESSION" | "LOCAL" => global = false,
                _ => {}
            }
        }
        let charset = item.first().is_some_and(|w| w.is("NAMES") || w.is("CHARACTER") || w.is("CHARSET"));
        let risk = if charset {
            client_charset(item)
        } else if global {
            maintenance(Some(Danger::ServerCommand))
        } else {
            assignment(item)
        };
        out = Some(match out {
            Some(o) => worse(o, risk),
            None => risk,
        });
    }
    out.unwrap_or_else(unrecognized)
}

/// `NAMES x [COLLATE y]`, `CHARACTER SET x`, `CHARSET x` of a `SET`: safe for a UTF-8 character
/// set ([`CLIENT_CHARSETS`]).
fn client_charset(item: &[W]) -> Risk {
    let at = if item[0].is("CHARACTER") { 2 } else { 1 };
    let shaped = match item.get(at..) {
        Some([_]) => at == 1 || item[1].is("SET"),
        Some([_, c, _]) => item[0].is("NAMES") && c.is("COLLATE"),
        _ => false,
    };
    if !shaped {
        return unrecognized();
    }
    let charset = value_text(&item[at]).to_ascii_lowercase();
    if CLIENT_CHARSETS.contains(&charset.as_str()) {
        Risk::session(true)
    } else {
        Risk::danger(Class::Session, Danger::Setting)
    }
}

/// The text of a value token: a string without its quotes, a name, a number.
fn value_text<'a>(w: &W<'a>) -> &'a str {
    match w.kind {
        Tok::Str | Tok::QuotedIdent
            if w.text.len() >= 2 && !w.text.starts_with(['x', 'X', 'b', 'B', 'n', 'N', '$']) =>
        {
            &w.text[1..w.text.len() - 1]
        }
        _ => w.text,
    }
}

/// Whether a `SET` value turns a switch on: `ON`, `1`, `TRUE` (or their string).
fn on(value: &[W]) -> Option<bool> {
    let [w] = value else { return None };
    match value_text(w).to_ascii_lowercase().as_str() {
        "on" | "1" | "true" => Some(true),
        "off" | "0" | "false" => Some(false),
        _ => None,
    }
}

/// One assignment of a `SET` (`[scope] name = value`, `@@scope.name = value`, `@x := value`).
fn assignment(item: &[W]) -> Risk {
    let Some(eq) = item.iter().position(|w| w.is_op("=")) else { return unrecognized() };
    let mut target = &item[..eq];
    if target.last().is_some_and(|w| w.is_op(":")) {
        target = &target[..target.len() - 1];
    }
    let value = &item[eq + 1..];
    if value.is_empty() || target.is_empty() {
        return unrecognized();
    }
    const SCOPES: [&str; 5] = ["GLOBAL", "PERSIST", "PERSIST_ONLY", "SESSION", "LOCAL"];
    // A variable alone, or a name or names joined by dots after an optional scope: anything else
    // (`SET STATEMENT … FOR …` of MariaDB, …) is no assignment this module knows.
    let (variable, names) = match target.split_first() {
        Some((v, [])) if v.kind == Tok::Variable => (Some(v.text), &target[..0]),
        Some((first, rest)) if SCOPES.iter().any(|k| first.is(k)) => (None, rest),
        _ => (None, target),
    };
    let shaped = variable.is_some()
        || (names.len() % 2 == 1
            && names.iter().enumerate().all(|(k, w)| if k % 2 == 0 { w.is_name() } else { w.kind == Tok::Dot }));
    if !shaped {
        return unrecognized();
    }
    let mut global = target.first().is_some_and(|w| ["GLOBAL", "PERSIST", "PERSIST_ONLY"].iter().any(|k| w.is(k)));
    let name = match variable {
        // A user variable: the session's own state.
        Some(v) if !v.starts_with("@@") => return Risk::session(true),
        // `@@name`, `@@scope.name` (the lexer keeps the dots in the variable).
        Some(v) => {
            let lower = v.trim_start_matches('@').to_ascii_lowercase();
            match lower.split_once('.') {
                Some((scope @ ("global" | "persist" | "persist_only" | "session" | "local"), name)) => {
                    global = matches!(scope, "global" | "persist" | "persist_only");
                    name.to_string()
                }
                _ => lower,
            }
        }
        None => names.iter().map(|w| w.text).collect::<String>().to_ascii_lowercase(),
    };
    if name.is_empty() || name.starts_with('`') {
        return unrecognized();
    }
    if global {
        return maintenance(Some(Danger::ServerCommand));
    }
    if READ_ONLY_SETTINGS.contains(&name.as_str()) {
        let off = on(value) != Some(true);
        return Risk { read_write: off, ..Risk::session(!off) };
    }
    if name == "autocommit" {
        return Risk { implicit_commit: on(value) != Some(false), ..Risk::of(Class::Tx) };
    }
    if name == "character_set_client"
        && value.len() == 1
        && CLIENT_CHARSETS.contains(&value_text(&value[0]).to_ascii_lowercase().as_str())
    {
        return Risk::session(true);
    }
    if RISKY_SETTINGS.contains(&name.as_str()) {
        return Risk::danger(Class::Session, Danger::Setting);
    }
    Risk::session(SAFE_SETTINGS.contains(&name.as_str()))
}

/// Whether the app may run `sql` again (to fetch a later page) or count its rows, as far as its
/// text tells: one query on the allowlist of reads, with no `INTO`, no locking clause, no
/// variable, no call of a function that is not a built-in or that is [`VOLATILE`], and no name
/// in a schema of the server's own (`performance_schema`, `information_schema`, `sys`, `mysql`,
/// whose rows change on their own), in every sql mode that reads it differently ([`readings`]).
/// What only the server can tell (whether a name it reads is a view, whose query may call
/// anything) is for the driver to ask before it runs it again, with the names of [`names`].
pub fn repeatable(sql: &str, mode: MySqlMode) -> Result<(), NotRepeatable> {
    names(sql, mode).map(|_| ())
}

/// The schemas of the server's own.
const SYSTEM_SCHEMAS: &[&str] = &["information_schema", "mysql", "performance_schema", "sys"];

/// [`repeatable`], with every name the statement writes (each table, column, alias or schema,
/// as written, qualified ones joined with `.`): a superset of the tables and views it reads.
pub fn names(sql: &str, mode: MySqlMode) -> Result<Vec<String>, NotRepeatable> {
    guarded(
        || Err(NotRepeatable::Unreadable),
        || {
            let mut all: Vec<String> = Vec::new();
            for (k, m) in readings(sql, mode).into_iter().enumerate() {
                match names_as(sql, m) {
                    None if k == 0 => return Err(NotRepeatable::Unreadable),
                    // Another mode that reads the text as a syntax error: no way it runs so.
                    None => {}
                    Some(Err(why)) => return Err(why),
                    Some(Ok(names)) => {
                        for name in names {
                            if !all.contains(&name) {
                                all.push(name);
                            }
                        }
                    }
                }
            }
            Ok(all)
        },
    )
}

/// [`names`] in one sql mode; `None` when the server in that mode would refuse the text as a
/// syntax error.
fn names_as(sql: &str, mode: MySqlMode) -> Option<Result<Vec<String>, NotRepeatable>> {
    Some(match statements(sql, mode) {
        Ok((stmts, hint)) => names_of(&stmts, hint),
        Err(Unread::Syntax) => return None,
        Err(Unread::Risk(r)) if matches!(r.danger, Some(Danger::Unrecognized | Danger::TooComplex)) => {
            Err(NotRepeatable::Unreadable)
        }
        Err(Unread::Risk(_)) => Err(NotRepeatable::Writes),
    })
}

/// [`names_as`] of a text read into statements.
fn names_of(stmts: &[Vec<W>], hint: bool) -> Result<Vec<String>, NotRepeatable> {
    let [ws] = stmts else { return Err(NotRepeatable::NotOne) };
    let top = ws.first().map(|w| (w.kind, w.up.as_str()));
    let query = starts_query(ws, 0)
        || (top.is_some_and(|(_, w)| w == "WITH") && with_main(ws).is_some_and(|i| starts_query(ws, i)));
    if !query {
        return Err(NotRepeatable::NotSelect);
    }
    let risk = statement(ws);
    match risk.danger {
        Some(Danger::Unrecognized | Danger::TooComplex) => return Err(NotRepeatable::Unreadable),
        Some(_) => return Err(NotRepeatable::Writes),
        None if hint => return Err(NotRepeatable::Writes),
        None => {}
    }
    if risk.class != Class::Read || risk.writes || risk.read_write || risk.explain != Explain::No {
        return Err(NotRepeatable::Writes);
    }
    if risk.runs_code || risk.unchecked_call {
        return Err(NotRepeatable::UserFunction);
    }
    if let Some(c) = calls(ws).into_iter().find(|c| VOLATILE.contains(&c.name.as_str())) {
        return Err(NotRepeatable::Volatile(c.name.to_ascii_lowercase()));
    }
    if let Some(v) = ws.iter().find(|w| w.kind == Tok::Variable) {
        return Err(NotRepeatable::Variable(v.text.to_string()));
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < ws.len() {
        match name_at(ws, i) {
            Some((name, next)) if ws[i].kind == Tok::QuotedIdent || ws[i].kind == Tok::Ident => {
                let schema = if ws[i].kind == Tok::QuotedIdent { unquote(ws[i].text) } else { ws[i].text.to_string() };
                if next > i + 1 && SYSTEM_SCHEMAS.contains(&schema.to_ascii_lowercase().as_str()) {
                    return Err(NotRepeatable::NotATable(name));
                }
                if ws.get(next).is_none_or(|w| w.kind != Tok::LParen) {
                    out.push(name);
                }
                i = next;
            }
            _ => i += 1,
        }
    }
    Ok(out)
}

/// Whether the query `sql` sorts its rows at the top (`ORDER BY` outside every parenthesis,
/// once the parentheses around all of it are left out).
pub fn ordered(sql: &str, mode: MySqlMode) -> bool {
    guarded(
        || false,
        || {
            let Ok((stmts, _)) = statements(sql, mode) else { return false };
            let [ws] = stmts.as_slice() else { return false };
            let ws = unwrap(ws);
            let depth = ws.first().map_or(0, |w| w.depth);
            ws.windows(2).any(|w| w[0].depth == depth && w[0].is("ORDER") && w[1].is("BY"))
        },
    )
}

/// `sql` without the `;` (and the blanks and comments around it) that end it.
fn without_semicolons(sql: &str, mode: MySqlMode) -> &str {
    let mut end = sql.len();
    for t in lex_in(sql, Dialect::MySql(mode)).into_iter().rev() {
        if t.kind == Tok::Semi && t.text(sql) == ";" {
            end = t.start;
        } else if !t.is_trivia() {
            break;
        }
    }
    sql[..end].trim_end()
}

/// The query that counts the rows of `sql` when the user asks: `SELECT COUNT(*) FROM (<sql>)
/// AS datarig_count`, for a statement on the allowlist ([`repeatable`]). The statement goes on
/// lines of its own, so a trailing `#` or `--` comment ends before the closing parenthesis, and
/// a trailing `;` is left out. The text is read again and must itself be on the allowlist.
pub fn count_query(sql: &str, mode: MySqlMode) -> Result<String, NotRepeatable> {
    repeatable(sql, mode)?;
    let text = format!("SELECT COUNT(*) FROM (\n{}\n) AS datarig_count", without_semicolons(sql, mode));
    repeatable(&text, mode)?;
    Ok(text)
}

#[cfg(test)]
mod tests;
