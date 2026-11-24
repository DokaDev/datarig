//! Text the user types into a table tab's filter line and filter dialog reaches the app's own
//! callers of the SQL lexer while it is being written, and they cut what they lexed by the
//! tokens' byte ranges: none may panic on any text, however unfinished (a trailing backslash, a
//! string, quoted name, comment or dollar quote left open, an executable comment cut short, a
//! multi-byte character at the end). Every text here is built from an alphabet of the characters
//! the lexer's rules turn on, all of them up to a length and a fixed sample of longer ones, and
//! fed in PostgreSQL and in MySQL and MariaDB under each sql mode that changes how text is
//! lexed. The core's own entry points (the checks of the table bar, the reader of a condition)
//! are fed by the core's no-panic tests.

use super::apply::{breaks_lost, term_at};
use super::draft::{ColumnInfo, Op, Where, catalogue_meta, kind_of_type, parse_input, parse_list, zoned_type};
use super::values::mentions;
use crate::app::table_filter::order_completion;
use crate::widgets::filter_dialog::{highlight, short_type};
use datarig_core::driver::{ColumnMeta, ValueKind};
use datarig_core::query::{ColumnFacts, ColumnKind, Target};
use datarig_core::sql::dialect::{Dialect, DollarQuotes, MySqlMode};

/// The characters the lexer's rules turn on, and one of two, three and four bytes.
const ALPHABET: &[&str] = &[
    "\\", "'", "\"", "`", "$", "/", "*", "!", "M", "-", "#", "@", "\n", "\r", " ", ";", "x", "0", "e", ".", "(", ")",
    ",", ":", "=", "<", ">", "&", "|", "%", "_", "\t", "\0", "N", "U", "é", "あ", "🐘",
];

/// Openings of the longer constructs, put before the generated text.
const PREFIXES: &[&str] = &[
    "",
    "/*!",
    "/*!80023",
    "/*M!",
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
    "--",
    "-- ",
    "#",
    "id = 1 ",
    "id = 1 AND ",
    "id IN (",
    "name LIKE '",
    "\"Mixed Case\" = ",
    "`Mixed Case` = ",
    "id DESC NULLS ",
    "id, ",
    "\\G",
    "\\N",
];

/// Every text of up to two characters of [`ALPHABET`], every one character after each prefix,
/// and a fixed sample of longer ones after a prefix (the same on every run).
pub(crate) fn texts() -> Vec<String> {
    let mut out = vec![String::new()];
    for a in ALPHABET {
        out.push(a.to_string());
        for b in ALPHABET {
            out.push(format!("{a}{b}"));
        }
    }
    for p in PREFIXES {
        out.push(p.to_string());
        for a in ALPHABET {
            out.push(format!("{p}{a}"));
        }
    }
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for _ in 0..400 {
        let mut s = PREFIXES[next() as usize % PREFIXES.len()].to_string();
        for _ in 0..next() as usize % 33 {
            s.push_str(ALPHABET[next() as usize % ALPHABET.len()]);
        }
        out.push(s);
    }
    out
}

/// PostgreSQL, MySQL in every sql mode the lexer reads differently (dollar quotes read, not
/// read and not known), and MariaDB with and without `ANSI_QUOTES` and `NO_BACKSLASH_ESCAPES`.
pub(crate) fn dialects() -> Vec<Dialect> {
    let mut out = vec![Dialect::Postgres];
    for bits in 0..4u8 {
        let mode = |dollar_quotes| MySqlMode {
            ansi_quotes: bits & 1 != 0,
            no_backslash_escapes: bits & 2 != 0,
            dollar_quotes,
            ignore_space: false,
        };
        for dollar in [DollarQuotes::NotRead, DollarQuotes::Read, DollarQuotes::NotKnown] {
            out.push(Dialect::MySql(mode(dollar)));
        }
        out.push(Dialect::MariaDb(mode(DollarQuotes::NotRead)));
    }
    out
}

/// A table of a number, a text, a bytes and an unknown column, one named with a space.
pub(crate) fn target(d: Dialect) -> Target {
    let columns = vec![
        ColumnFacts::new("id", ColumnKind::Number),
        ColumnFacts::new("name", ColumnKind::Text),
        ColumnFacts::new("Mixed Case", ColumnKind::Text),
        ColumnFacts::new("data", ColumnKind::Bytes),
        ColumnFacts::new("born", ColumnKind::Other),
    ];
    Target { primary_key: vec!["id".into()], ..Target::new(d, columns) }
}

/// The byte offsets of `text` a cursor may stand at: each character's start and the end.
pub(crate) fn cursors(text: &str) -> impl Iterator<Item = usize> + '_ {
    text.char_indices().map(|(i, _)| i).chain([text.len()])
}

#[test]
fn the_filter_dialogs_lexer_callers_never_panic() {
    let column =
        |name: &str, ty: &str, kind| ColumnInfo::from_meta(&ColumnMeta::new(name.into(), ty.into(), kind, None));
    let fields = [
        column("id", "int8", ValueKind::Integer),
        column("name", "text", ValueKind::Text),
        column("data", "bytea", ValueKind::Bytes),
    ];
    for text in texts() {
        for d in dialects() {
            let t = target(d);
            // The SQL field shown on one line, a value count's column, the highlight.
            let _ = breaks_lost(&text, d);
            for col in ["id", "Mixed Case", "\u{e9}", ""] {
                let _ = mentions(&text, col, d);
            }
            let _ = highlight(&text, d, Some(&(0..text.len())));
            // The text read into chips, written back, without a column, and a place in it.
            let w = Where::from_text(&text, &t);
            let _ = (w.text(), w.chips(), w.text_without("id"), w.text_without("name"));
            for at in [0..0, 0..text.len(), text.len()..text.len()] {
                let _ = term_at(&w, &at);
            }
            // The text as a catalogue's type name: the column list's type, the kind of a column.
            let _ = (short_type(d, &text), kind_of_type(d, &text), zoned_type(d, &text));
            let _ = catalogue_meta(d, "c", &text);
            // The order field's completion at every place of the cursor.
            for cursor in cursors(&text) {
                for force in [false, true] {
                    let _ = order_completion(&text, cursor, &t, force);
                }
            }
        }
        // The value fields: one value, two, and a list.
        for c in &fields {
            for op in [Op::Eq, Op::Between, Op::InList, Op::Contains] {
                let _ = parse_input(op, &[&text, &text], c);
            }
            let _ = parse_list(&text, c, &[]);
        }
    }
}
