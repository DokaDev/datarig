//! Text the user types reaches the SQL lexer, the splitter, the classifiers, the formatter, the
//! completer, the check values and the checks of the table bar while it is being written, so
//! none of them may panic on any text, however unfinished: a trailing backslash, a string,
//! quoted name, comment or dollar quote left open, an executable comment cut short, a
//! multi-byte character at the end. Every text here is built from an alphabet of the characters
//! those rules turn on, all of them up to a length and a fixed sample of longer ones, and fed to
//! the lexers and the splitter in every dialect and every sql mode that changes how MySQL text
//! is lexed (from every lexer state), and to the other entry points in PostgreSQL and every
//! combination of MySQL's and MariaDB's sql mode flags.

use datarig_core::driver::structure::CheckConstraint;
use datarig_core::profile::session::{Dbms, SessionPreset, join_statements, split_statements};
use datarig_core::query::{
    ColumnFacts, ColumnKind, FilterVerdict, TableName, Target, and_filter, check_filter, estimate_allowed,
    has_subquery, parse_order, recognise, split_terms,
};
use datarig_core::sql::complete::{Catalog, ColumnInfo, Relation, complete_in_dialect};
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_core::sql::format::{KeywordCase, Options, format_in};
use datarig_core::sql::lexer::{Delimiter, LexState, changes_schema_in, lex_backslash_strings, lex_from, lex_in};
use datarig_core::sql::plan::is_explain_in;
use datarig_core::sql::requote::requote_strings;
use datarig_core::sql::risk::{self, Classifier};
use datarig_core::sql::split::{segment_at_in, split_from, split_in};
use datarig_core::sql::values::check_values;

/// The characters the lexers' rules turn on, and one of two, three and four bytes: every text
/// of up to three of them is generated.
const CORE: &[&str] = &[
    "\\", "'", "\"", "`", "$", "/", "*", "!", "M", "-", "#", "@", "\n", "\r", " ", ";", "x", "0", "e", ".", "(", ")",
    "é", "あ", "🐘",
];

/// More characters the checks read (operators, brackets, a tab, NUL, a no-break space, a
/// combining mark, the letters of `\G`, `\N`, `U&`, `SELECT`, `FOR`): with [`CORE`], the
/// alphabet of every text of up to two characters and of the longer sampled ones.
const EXTRA: &[&str] = &[
    ",", ":", "=", "+", "<", ">", "&", "|", "^", "?", "[", "]", "{", "}", "_", "%", "\t", "\0", "\u{a0}", "\u{301}",
    "g", "G", "N", "u", "U", "S", "E", "L", "F", "O", "R",
];

/// [`CORE`] and [`EXTRA`].
fn alphabet() -> Vec<&'static str> {
    CORE.iter().chain(EXTRA).copied().collect()
}

/// Openings of the longer constructs, put before the generated text.
const PREFIXES: &[&str] = &[
    "",
    "/*!",
    "/*!8",
    "/*!80023",
    "/*M!",
    "/*M!100",
    "/*+",
    "$$",
    "$t$",
    "x'",
    "b'",
    "N'",
    "E'",
    "U&'",
    "_utf8mb4'",
    "@'",
    "@@",
    "--",
    "-- ",
    "#",
    "delimiter ",
    "DELIMITER //\n",
    "\\g",
    "id = 1 ",
    "id IN (",
    "SELECT ",
    ":=",
    "::",
    "||",
    "&&",
    "\\G",
    "\\N",
    "_latin1'",
    "/*+ SET_VAR(sort_buffer_size = 16M) */",
    "SELECT * FROM t WHERE (",
];

/// Every MySQL sql mode the lexer reads differently, MariaDB's too.
fn mysql_modes() -> Vec<MySqlMode> {
    let mut out = Vec::new();
    for bits in 0..32u8 {
        out.push(MySqlMode {
            ansi_quotes: bits & 1 != 0,
            no_backslash_escapes: bits & 2 != 0,
            dollar_quotes: bits & 4 != 0,
            mariadb: bits & 8 != 0,
            ignore_space: bits & 16 != 0,
        });
    }
    out
}

/// The modes that change what the lexer reads (the others change only what a name means).
fn lexing_modes() -> Vec<MySqlMode> {
    mysql_modes().into_iter().filter(|m| !m.mariadb && !m.ignore_space).collect()
}

/// PostgreSQL and every MySQL mode the lexer reads differently, on MySQL and on MariaDB.
fn dialects() -> Vec<Dialect> {
    let mut out = vec![Dialect::Postgres];
    out.extend(lexing_modes().into_iter().map(Dialect::MySql));
    out.extend(lexing_modes().into_iter().map(|m| Dialect::MariaDb(MySqlMode { mariadb: true, ..m })));
    out
}

/// PostgreSQL, and MySQL and MariaDB in every combination of the sql mode flags, MariaDB's
/// also as a MariaDB session says them.
fn checked_dialects() -> Vec<Dialect> {
    let mut out = vec![Dialect::Postgres];
    out.extend(mysql_modes().into_iter().map(Dialect::MySql));
    out.extend(mysql_modes().into_iter().filter(|m| m.mariadb).map(Dialect::MariaDb));
    out
}

/// Every text of up to `n` characters of `alphabet`.
fn all_up_to(alphabet: &[&str], n: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut last = vec![String::new()];
    for _ in 0..n {
        let mut next = Vec::new();
        for s in &last {
            for c in alphabet {
                next.push(format!("{s}{c}"));
            }
        }
        out.extend(next.iter().cloned());
        last = next;
    }
    out
}

/// `n` texts of up to `max` characters of the whole alphabet after a prefix, the same on every
/// run.
fn sampled(n: usize, max: usize) -> Vec<String> {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let alphabet = alphabet();
    (0..n)
        .map(|_| {
            let mut s = PREFIXES[next() as usize % PREFIXES.len()].to_string();
            for _ in 0..next() as usize % (max + 1) {
                s.push_str(alphabet[next() as usize % alphabet.len()]);
            }
            s
        })
        .collect()
}

/// The texts the lexers and the splitter are fed: every one of up to three characters of
/// [`CORE`] and of up to two of the whole alphabet; after each prefix, every one of up to two of
/// [`CORE`] and one of the whole alphabet; and a fixed sample of longer ones.
fn lexed_texts() -> Vec<String> {
    let mut out = all_up_to(CORE, 3);
    out.extend(all_up_to(&alphabet(), 2));
    for p in PREFIXES {
        for s in all_up_to(CORE, 2).into_iter().chain(all_up_to(&alphabet(), 1)) {
            out.push(format!("{p}{s}"));
        }
    }
    out.extend(sampled(3000, 32));
    out
}

/// The lexer's states a line of a longer text may start in.
fn states() -> Vec<LexState> {
    let base = LexState::default();
    let mut out = vec![base, LexState { pending: true, ..base }, LexState { exec: true, ..base }];
    out.push(LexState { line_start: false, ..base });
    for d in ["//", "$$", "\\", "é", "あい", "end"] {
        if let Some(delimiter) = Delimiter::new(d) {
            out.push(LexState { delimiter, ..base });
            out.push(LexState { delimiter, exec: true, pending: true, ..base });
        }
    }
    out
}

/// The tokens of `src` cover it, in order, each at least one character, each starting and
/// ending on a character's boundary.
fn assert_covers(src: &str, toks: &[datarig_core::sql::lexer::Token], what: &str) {
    let mut at = 0;
    for t in toks {
        assert_eq!(t.start, at, "{what}: {src:?} {toks:?}");
        assert!(t.end > t.start && t.end <= src.len(), "{what}: {src:?} {t:?}");
        assert!(src.is_char_boundary(t.end), "{what}: {src:?} {t:?}");
        let _ = t.text(src);
        at = t.end;
    }
    assert_eq!(at, src.len(), "{what}: {src:?} {toks:?}");
}

#[test]
fn the_lexers_read_every_text_into_tokens_that_cover_it() {
    let texts = lexed_texts();
    let states = states();
    for src in &texts {
        assert_covers(src, &lex_backslash_strings(src), "backslash strings");
        for d in dialects() {
            assert_covers(src, &lex_in(src, d), &format!("{d:?}"));
            let _ = changes_schema_in(src, d);
            let _ = is_explain_in(src, d);
        }
        for m in lexing_modes() {
            for s in &states {
                let toks = lex_from(src, Dialect::MySql(m), *s);
                assert_covers(src, &toks, &format!("{m:?} from {s:?}"));
                let mut state = *s;
                for t in &toks {
                    state = state.after(t, src, Dialect::MySql(m));
                }
            }
        }
    }
}

#[test]
fn splitting_never_panics() {
    let texts = lexed_texts();
    let states = states();
    for src in &texts {
        for d in dialects() {
            for s in split_in(src, d) {
                let _ = s.body(src);
            }
            for cursor in (0..=src.len()).filter(|&i| src.is_char_boundary(i)) {
                let _ = segment_at_in(src, cursor, d);
            }
        }
        for m in lexing_modes() {
            for s in &states {
                for stmt in split_from(src, Dialect::MySql(m), *s) {
                    let _ = stmt.body(src);
                }
            }
        }
        for dbms in Dbms::ALL {
            let parts = split_statements(dbms, src);
            let _ = join_statements(dbms, &parts);
        }
    }
}

/// The texts the slower entry points (classifiers, formatter, completer, checks) are fed.
/// Fewer than the lexers get, as each runs in every mode: every text of up to two characters of
/// the whole alphabet, every one character after each prefix, and a fixed sample of longer
/// ones after a prefix.
fn checked_texts() -> Vec<String> {
    let alphabet = alphabet();
    let mut texts = all_up_to(&alphabet, 2);
    for p in PREFIXES {
        for s in all_up_to(&alphabet, 1) {
            texts.push(format!("{p}{s}"));
        }
    }
    texts.extend(sampled(600, 32));
    texts
}

#[test]
fn the_classifiers_never_panic() {
    for src in checked_texts() {
        for d in checked_dialects() {
            let _ = Classifier::new(Language::Sql(d)).classify(&src);
        }
        for dbms in Dbms::ALL {
            let preset = SessionPreset::new("p", dbms, vec![src.clone()]);
            let _ = preset.problem();
            let _ = preset.read_only_refused();
        }
        for m in mysql_modes() {
            let _ = risk::mysql::repeatable(&src, m);
            let _ = risk::mysql::names(&src, m);
            let _ = risk::mysql::named(&src, m);
            let _ = risk::mysql::reads_zone(&src, m);
            let _ = risk::mysql::ordered(&src, m);
            let _ = risk::mysql::count_query(&src, m);
        }
        let _ = risk::mysql::session_set(&src);
    }
}

#[test]
fn formatting_requoting_and_completing_never_panic() {
    let cat = catalog();
    let path = ["shop".to_string()];
    let checks = [CheckConstraint {
        name: "c".into(),
        expression: String::new(),
        columns: vec!["name".into()],
        definition: String::new(),
    }];
    let opts = Options { case: KeywordCase::Upper, indent: 2 };
    for src in checked_texts() {
        for d in checked_dialects() {
            let _ = format_in(&src, opts, "  ", d);
            for cursor in (0..=src.len()).filter(|&i| src.is_char_boundary(i)) {
                let _ = complete_in_dialect(&src, cursor, &cat, true, &path, d);
            }
            let _ = d.unquote(&src);
            let _ = d.unquote_lenient(&src);
            let _ = d.unquote_literal(&src);
            let _ = d.literal_open("", &src);
            let _ = d.line_comment_at(&src);
            let _ = d.unparenthesized(&src);
            let _ = d.constant(&src);
            let mut check = checks.clone();
            check[0].expression = src.clone();
            check[0].definition = format!("CHECK ({src})");
            let _ = check_values(d, &check);
            let _ = and_filter(&src, "id = 1", d);
            let _ = and_filter("SELECT * FROM `shop`.`users` WHERE (id = 1)", &src, d);
        }
        for from in mysql_modes() {
            for to in lexing_modes().into_iter().filter(|m| !m.dollar_quotes) {
                let _ = requote_strings(&src, Dialect::MySql(from), Dialect::MySql(to));
            }
        }
    }
}

/// `shop.users` with a column of each kind the checks treat apart, one in mixed case.
fn catalog() -> Catalog {
    let columns = ["id:int", "name:text", "Mixed Case:text", "data:bytea", "born:timestamp", "mood:enum"]
        .iter()
        .map(|c| {
            let (n, t) = c.split_once(':').unwrap();
            ColumnInfo::new(n, t)
        })
        .collect();
    let users = Relation { schema: "shop".into(), name: "users".into(), is_view: false, columns };
    Catalog { schemas: vec!["shop".into(), "public".into()], relations: vec![users] }
}

fn target(d: Dialect) -> Target {
    let columns = vec![
        ColumnFacts::new("id", ColumnKind::Number),
        ColumnFacts::new("name", ColumnKind::Text),
        ColumnFacts::new("Mixed Case", ColumnKind::Text),
        ColumnFacts::new("data", ColumnKind::Bytes),
        ColumnFacts::new("born", ColumnKind::Other),
    ];
    Target { primary_key: vec!["id".into()], ..Target::new(d, columns) }
}

#[test]
fn the_table_bar_checks_never_panic() {
    let table = TableName { database: None, schema: "shop".into(), name: "users".into() };
    for src in checked_texts() {
        for d in checked_dialects() {
            let t = target(d);
            let classifier = Classifier::new(Language::Sql(d));
            for read_only in [false, true] {
                if let Ok(v) = check_filter(&src, &table, &t, &classifier, read_only) {
                    let _ = estimate_allowed(&v, &src, d);
                }
            }
            let _ = estimate_allowed(&FilterVerdict::Clear, &src, d);
            let _ = has_subquery(&src, d);
            let _ = parse_order(&src, &t);
            let _ = recognise(&src, &t);
            let _ = split_terms(&src, d);
        }
    }
}
