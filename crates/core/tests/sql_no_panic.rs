//! Text the user types reaches the SQL lexer, the splitter, the classifiers, the formatter, the
//! completer and the checks of the table bar while it is being written, so none of them may
//! panic on any text, however unfinished: a trailing backslash, a string, quoted name, comment
//! or dollar quote left open, an executable comment cut short, a multi-byte character at the
//! end. Every text here is built from an alphabet of the characters those rules turn on, all of
//! them up to a length and a fixed set of longer ones, and fed to every entry point in every
//! dialect and MySQL sql mode.

use datarig_core::profile::session::{Dbms, SessionPreset, join_statements, split_statements};
use datarig_core::query::{
    ColumnFacts, ColumnKind, FilterVerdict, TableName, Target, check_filter, estimate_allowed, has_subquery,
    parse_order, recognise, split_terms,
};
use datarig_core::sql::complete::{Catalog, complete_in_dialect};
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_core::sql::format::{KeywordCase, Options, format_in};
use datarig_core::sql::lexer::{Delimiter, LexState, changes_schema_in, lex_backslash_strings, lex_from, lex_in};
use datarig_core::sql::plan::is_explain_in;
use datarig_core::sql::requote::requote_strings;
use datarig_core::sql::risk::{self, Classifier};
use datarig_core::sql::split::{segment_at_in, split_from, split_in};

/// The characters the lexers' rules turn on, and one of two, three and four bytes.
const ALPHABET: &[&str] = &[
    "\\", "'", "\"", "`", "$", "/", "*", "!", "M", "-", "#", "@", "\n", "\r", " ", ";", "x", "0", "e", ".", "(", ")",
    "é", "あ", "🐘",
];

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

/// PostgreSQL and every MySQL mode the lexer reads differently.
fn dialects() -> Vec<Dialect> {
    let mut out = vec![Dialect::Postgres];
    out.extend(lexing_modes().into_iter().map(Dialect::MySql));
    out
}

/// PostgreSQL, and MySQL and MariaDB in their default modes and each mode alone.
fn checked_dialects() -> Vec<Dialect> {
    let d = MySqlMode::default();
    let modes = [
        d,
        MySqlMode { mariadb: true, ..d },
        MySqlMode { ansi_quotes: true, ..d },
        MySqlMode { no_backslash_escapes: true, ..d },
        MySqlMode { dollar_quotes: true, ..d },
        MySqlMode { ignore_space: true, ..d },
        MySqlMode {
            ansi_quotes: true,
            no_backslash_escapes: true,
            dollar_quotes: true,
            mariadb: true,
            ignore_space: true,
        },
    ];
    let mut out = vec![Dialect::Postgres];
    out.extend(modes.into_iter().map(Dialect::MySql));
    out
}

/// Every text of up to `n` characters of the alphabet.
fn all_up_to(n: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut last = vec![String::new()];
    for _ in 0..n {
        let mut next = Vec::new();
        for s in &last {
            for c in ALPHABET {
                next.push(format!("{s}{c}"));
            }
        }
        out.extend(next.iter().cloned());
        last = next;
    }
    out
}

/// `n` texts of up to `max` characters of the alphabet after a prefix, the same on every run.
fn sampled(n: usize, max: usize) -> Vec<String> {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    (0..n)
        .map(|_| {
            let mut s = PREFIXES[next() as usize % PREFIXES.len()].to_string();
            for _ in 0..next() as usize % (max + 1) {
                s.push_str(ALPHABET[next() as usize % ALPHABET.len()]);
            }
            s
        })
        .collect()
}

/// Short texts: every one of up to two characters, after each prefix.
fn short_texts() -> Vec<String> {
    let mut out = Vec::new();
    for p in PREFIXES {
        for s in all_up_to(2) {
            out.push(format!("{p}{s}"));
        }
    }
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
    let mut texts = all_up_to(3);
    texts.extend(short_texts());
    texts.extend(sampled(3000, 16));
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
    let mut texts = all_up_to(3);
    texts.extend(short_texts());
    texts.extend(sampled(2000, 16));
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
fn checked_texts() -> Vec<String> {
    let mut texts = all_up_to(2);
    texts.extend(short_texts());
    texts.extend(sampled(400, 12));
    texts
}

#[test]
fn the_classifiers_never_panic() {
    let d = MySqlMode::default();
    let all_on = MySqlMode {
        ansi_quotes: true,
        no_backslash_escapes: true,
        dollar_quotes: true,
        mariadb: true,
        ignore_space: true,
    };
    for src in checked_texts() {
        for d in checked_dialects() {
            let _ = Classifier::new(Language::Sql(d)).classify(&src);
        }
        for dbms in Dbms::ALL {
            let preset = SessionPreset::new("p", dbms, vec![src.clone()]);
            let _ = preset.problem();
            let _ = preset.read_only_refused();
        }
        for m in [d, all_on] {
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
    let cat = Catalog::default();
    let opts = Options { case: KeywordCase::Upper, indent: 2 };
    for src in checked_texts() {
        for d in checked_dialects() {
            let _ = format_in(&src, opts, "  ", d);
            for cursor in (0..=src.len()).filter(|&i| src.is_char_boundary(i)) {
                let _ = complete_in_dialect(&src, cursor, &cat, true, &[], d);
            }
            let _ = d.unquote(&src);
            let _ = d.unquote_lenient(&src);
            let _ = d.unquote_literal(&src);
            let _ = d.literal_open("", &src);
            let _ = d.line_comment_at(&src);
            let _ = d.unparenthesized(&src);
            let _ = d.constant(&src);
        }
        for from in mysql_modes() {
            for to in [MySqlMode::default(), MySqlMode { no_backslash_escapes: true, ..MySqlMode::default() }] {
                let _ = requote_strings(&src, Dialect::MySql(from), Dialect::MySql(to));
            }
        }
    }
}

fn target(d: Dialect) -> Target {
    let columns = vec![ColumnFacts::new("id", ColumnKind::Number), ColumnFacts::new("name", ColumnKind::Text)];
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
