//! The editor's formatter: SQL laid out by `sqlformat`, accepted only when nothing but the layout
//! changed.
//!
//! `sqlformat` decides where lines break and how far they are indented. The text is then
//! rebuilt from the input's own tokens (their text exactly as written; a keyword's case may
//! change when asked) with the whitespace `sqlformat` put between them. It is refused, and the
//! text stays as it was, unless:
//!
//! * the formatted text has the same tokens as the input, in the same order (datarig's lexer, in
//!   the text's dialect; words compared ignoring their ASCII case, as `sqlformat` may change it,
//!   every other token exactly, comments included; a PostgreSQL dollar-quoted body is not given
//!   to `sqlformat` at all);
//! * operator characters written together stay together and those written apart stay apart
//!   (the lexer reads each operator character alone, the server reads `<=` or `->>` as one
//!   operator), as do, in PostgreSQL, `U&` before a string or a name, `:` before what follows
//!   it (a psql variable) and `\` (a psql command), and in MySQL the name of a built-in function
//!   the server parses itself (`count`, `group_concat`, …) and the `(` after it (followed by a
//!   blank it is read otherwise);
//! * two strings in a row keep a line break between them, or keep none (strings separated by a
//!   line break are one string to the server);
//! * the rebuilt text lexes to the input's tokens again, exactly but for the case of keywords;
//! * MySQL: the text has no client command (`DELIMITER`) line.
//!
//! A keyword's case changes only where that cannot change what it names: in MySQL, only a
//! reserved word's (a non-reserved one may be a table's name, whose case the server may keep).

use super::dialect::Dialect;
use super::ident;
use super::lexer::{Tok, Token, lex_in};
use std::ops::Range;

/// MySQL's built-in functions whose name must be followed by `(` right away, unless the
/// session has `IGNORE_SPACE` (the manual's "Function Name Parsing and Resolution", 8.0 to
/// 9.x).
const MYSQL_SPECIAL_FUNCTIONS: &[&str] = &[
    "ADDDATE",
    "BIT_AND",
    "BIT_OR",
    "BIT_XOR",
    "CAST",
    "COUNT",
    "CURDATE",
    "CURTIME",
    "DATE_ADD",
    "DATE_SUB",
    "EXTRACT",
    "GROUP_CONCAT",
    "JSON_ARRAYAGG",
    "JSON_OBJECTAGG",
    "MAX",
    "MID",
    "MIN",
    "NOW",
    "POSITION",
    "SESSION_USER",
    "ST_COLLECT",
    "STD",
    "STDDEV",
    "STDDEV_POP",
    "STDDEV_SAMP",
    "SUBDATE",
    "SUBSTR",
    "SUBSTRING",
    "SUM",
    "SYSDATE",
    "SYSTEM_USER",
    "TRIM",
    "VARIANCE",
    "VAR_POP",
    "VAR_SAMP",
];

/// What a dollar-quoted body is to `sqlformat`.
const DOLLAR_MASK: &str = "''";

/// How the formatter writes keywords (the words the highlighter shows as keywords).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeywordCase {
    /// As they are written.
    #[default]
    Preserve,
    Upper,
    Lower,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub case: KeywordCase,
    /// Spaces per indent level.
    pub indent: u8,
}

/// Why the text was not formatted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The layout would change more than whitespace, from this byte of the input on.
    Changed { at: usize },
}

/// `src` formatted: the text that replaces `src[span]`, from its first token to the end of its
/// last (the blanks around them stay). Lines after the first start with `indent` (the column
/// the text starts at). The same text as `src[span]` when there is nothing to change.
/// PostgreSQL ([`format_in`]).
pub fn format(src: &str, opts: Options, indent: &str) -> Result<(Range<usize>, String), Refused> {
    format_in(src, opts, indent, Dialect::Postgres)
}

/// [`format`] for text in dialect `d`: laid out by `sqlformat` as its nearest dialect
/// ([`Dialect::sqlformat_dialect`]) and checked with `d`'s lexer.
pub fn format_in(src: &str, opts: Options, indent: &str, d: Dialect) -> Result<(Range<usize>, String), Refused> {
    let lex = |text: &str| lex_in(text, d);
    let input: Vec<Token> = lex(src).into_iter().filter(|t| t.kind != Tok::Whitespace).collect();
    let (Some(first), Some(last)) = (input.first(), input.last()) else { return Ok((0..0, String::new())) };
    let span = first.start..last.end;
    let fopts = sqlformat::FormatOptions {
        indent: sqlformat::Indent::Spaces(opts.indent),
        uppercase: None,
        lines_between_queries: 1,
        dialect: d.sqlformat_dialect(),
        ..sqlformat::FormatOptions::default()
    };
    // PostgreSQL: a dollar-quoted body goes to `sqlformat` as an empty string: it lays out the
    // code around it, never what is inside (the body comes back from the input as it is).
    let mut masked = String::with_capacity(src.len());
    let mut from = 0;
    match d {
        Dialect::Postgres => {
            for t in input.iter().filter(|t| t.kind == Tok::Dollar) {
                masked.push_str(&src[from..t.start]);
                masked.push_str(DOLLAR_MASK);
                from = t.end;
            }
        }
        // A client command line changes what the text after it means to the client, and
        // `sqlformat` lays it out as SQL.
        Dialect::MySql(_) => {
            if let Some(t) = input.iter().find(|t| t.kind == Tok::Directive) {
                return Err(Refused::Changed { at: t.start });
            }
        }
    }
    masked.push_str(&src[from..]);
    let laid = sqlformat::format(&masked, &sqlformat::QueryParams::None, &fopts);
    let layout: Vec<Token> = lex(&laid).into_iter().filter(|t| t.kind != Tok::Whitespace).collect();
    let at = |i: usize| Refused::Changed { at: input.get(i).map_or(src.len(), |t| t.start) };
    if let Some(i) = (0..input.len().max(layout.len())).find(|&i| match (input.get(i), layout.get(i)) {
        (Some(a), Some(b)) => !same_token(a, src, b, &laid),
        _ => true,
    }) {
        return Err(at(i));
    }

    let mut out = String::with_capacity(laid.len() + indent.len() * 8);
    for (i, t) in input.iter().enumerate() {
        if i > 0 {
            let gap = relaid(&laid[layout[i - 1].end..layout[i].start], indent);
            if !layout_only(d, &input, i, src, &gap) {
                return Err(at(i));
            }
            out.push_str(&gap);
        }
        let text = t.text(src);
        let recase = t.kind == Tok::Keyword
            && match d {
                Dialect::Postgres => true,
                // A keyword is a plain word: it needs quotes when it is reserved.
                Dialect::MySql(_) => ident::mysql::needs_quotes(text),
            };
        match (recase, opts.case) {
            (true, KeywordCase::Upper) => out.push_str(&text.to_ascii_uppercase()),
            (true, KeywordCase::Lower) => out.push_str(&text.to_ascii_lowercase()),
            _ => out.push_str(text),
        }
    }

    let again: Vec<Token> = lex(&out).into_iter().filter(|t| t.kind != Tok::Whitespace).collect();
    if let Some(i) = (0..input.len().max(again.len())).find(|&i| match (input.get(i), again.get(i)) {
        (Some(a), Some(b)) => {
            let (x, y) = (a.text(src), b.text(&out));
            a.kind != b.kind || if a.kind == Tok::Keyword { !x.eq_ignore_ascii_case(y) } else { x != y }
        }
        _ => true,
    }) {
        return Err(at(i));
    }
    Ok((span, out))
}

/// The whitespace `gap` of the laid-out text as written: no blanks at the end of a line, and
/// `indent` at the start of every new line.
fn relaid(gap: &str, indent: &str) -> String {
    match gap.rsplit_once('\n') {
        None => gap.to_string(),
        Some((lines, last)) => {
            let breaks = lines.matches('\n').count() + 1;
            format!("{}{last}", format!("\n{indent}").repeat(breaks))
        }
    }
}

/// Token `a` of `src` and token `b` of the laid-out text are the same token: words ignoring
/// ASCII case, everything else exactly.
fn same_token(a: &Token, src: &str, b: &Token, laid: &str) -> bool {
    let (x, y) = (a.text(src), b.text(laid));
    let word = |k: Tok| matches!(k, Tok::Keyword | Tok::Ident);
    match (a.kind, b.kind) {
        (Tok::Dollar, Tok::Str) => y == DOLLAR_MASK,
        (ka, kb) if word(ka) && word(kb) => x.eq_ignore_ascii_case(y),
        (ka, kb) => ka == kb && x == y,
    }
}

/// Whether the input's whitespace before token `i` may become `after` (the module's rules, in
/// dialect `d`).
fn layout_only(d: Dialect, toks: &[Token], i: usize, src: &str, after: &str) -> bool {
    let (a, b) = (&toks[i - 1], &toks[i]);
    let before = &src[a.end..b.start];
    let (ta, tb) = (a.text(src), b.text(src));
    let unicode_escape = |u: &Token, amp: &Token| {
        u.kind == Tok::Ident && u.text(src).eq_ignore_ascii_case("u") && amp.text(src) == "&" && u.end == amp.start
    };
    let keep = (a.kind == Tok::Op && b.kind == Tok::Op)
        || match d {
            // A built-in function MySQL parses itself and its `(`: with a blank between (and
            // `IGNORE_SPACE` off, the default) the server reads `count (*)` otherwise.
            Dialect::MySql(_) => {
                b.kind == Tok::LParen
                    && a.is_word()
                    && MYSQL_SPECIAL_FUNCTIONS.contains(&ta.to_ascii_uppercase().as_str())
            }
            Dialect::Postgres => {
                unicode_escape(a, b)
                    || (i >= 2 && unicode_escape(&toks[i - 2], a))
                    || (a.kind == Tok::Op && (ta == ":" || ta == "\\"))
                    || tb == "\\"
            }
        };
    if keep && before.is_empty() != after.is_empty() {
        return false;
    }
    !(a.kind == Tok::Str && b.kind == Tok::Str && before.contains('\n') != after.contains('\n'))
}

#[cfg(test)]
mod tests;
