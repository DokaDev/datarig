//! An `EXPLAIN` the user ran for a text plan, asked again for the plan as JSON: the same
//! statement with the same options, its `FORMAT` (`TEXT`, `YAML`, `XML`, or none) replaced
//! by `FORMAT JSON`. PostgreSQL syntax only: another database states its plan options in its
//! own words, and gets a helper of its own.
//!
//! The text is read with the lexer, so comments, quoted names and the case of the words are
//! taken as the server takes them; whatever is not changed is kept as it was written
//! (comments included). [`json`] then has PostgreSQL's parser (through [`risk`]) confirm that
//! both texts are an `EXPLAIN` and that the new one runs the statement exactly when the old one
//! did (`ANALYZE`): a text the two read differently is refused, never run.
//!
//! Running it again may run more than the planner even without `ANALYZE`: the server evaluates
//! an `EXECUTE`'s parameters to plan it. [`JsonExplain::evaluates`] is an allowlist: only a
//! text both readings show to be planned only (no `ANALYZE`, no `EXECUTE`, a plain read to the
//! parser that runs no code it cannot see) is free of it.
//!
//! [`risk`]: crate::sql::risk

use crate::sql::lexer::{Tok, Token, lex};
use crate::sql::risk::{self, Explain};

/// An `EXPLAIN` that asks for the plan as JSON.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonExplain {
    pub sql: String,
    /// It has `ANALYZE`: the statement runs again (a write is rolled back by the driver).
    pub analyze: bool,
    /// Running it again runs something beyond planning, or may: `ANALYZE`, an `EXECUTE` (its
    /// parameters are evaluated to plan it), or what the parser cannot show to be a plain read.
    /// Asked about first; only `ANALYZE` is rolled back.
    pub evaluates: bool,
}

/// Why a text is not asked again for a JSON plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotJson {
    /// It is not an `EXPLAIN`.
    NotExplain,
    /// More than one statement.
    Several,
    /// It asks for JSON already.
    AlreadyJson,
    /// Its options or its statement cannot be read (nothing after them, a list that does not
    /// end, an empty option, a text PostgreSQL's parser reads otherwise).
    Unreadable,
}

/// `sql` (one `EXPLAIN`) with `FORMAT JSON`, checked by PostgreSQL's parser: both texts are an
/// `EXPLAIN`, and the new one has `ANALYZE` exactly when `sql` has. Parses `sql` twice: for an
/// action, not for every frame ([`json_text`] is the lexer's reading alone).
pub fn json(sql: &str) -> Result<JsonExplain, NotJson> {
    let out = json_text(sql)?;
    let risk = risk::classify(sql);
    let before = risk.explain;
    if before == Explain::No || risk::classify(&out.sql).explain != before {
        return Err(NotJson::Unreadable);
    }
    let analyze = out.analyze || before == Explain::Analyze;
    let plain = risk.class == risk::Class::Read && risk.danger.is_none() && !risk.runs_code && !risk.writes;
    Ok(JsonExplain { analyze, evaluates: out.evaluates || analyze || !plain, ..out })
}

/// [`json`] as the lexer reads `sql`, without the parser's check.
pub fn json_text(sql: &str) -> Result<JsonExplain, NotJson> {
    let toks: Vec<Token> = lex(sql).into_iter().filter(|t| !t.is_trivia()).collect();
    let word = |t: &Token, w: &str| t.is_word() && t.text(sql).eq_ignore_ascii_case(w);
    let Some(explain) = toks.first().filter(|t| word(t, "EXPLAIN")) else { return Err(NotJson::NotExplain) };
    // One statement: nothing but `;` after the first `;`.
    if let Some(semi) = toks.iter().position(|t| t.kind == Tok::Semi)
        && toks[semi..].iter().any(|t| t.kind != Tok::Semi)
    {
        return Err(NotJson::Several);
    }
    let rest = &toks[1..];
    // An `EXECUTE` anywhere (also `CREATE TABLE … AS EXECUTE`) evaluates its parameters.
    let execute = rest.iter().any(|t| word(t, "EXECUTE"));
    let statement_at = |i: usize| rest.get(i).is_some_and(|t| t.kind != Tok::Semi);
    // `EXPLAIN (options) statement`, unless the parenthesis opens the statement itself
    // (`EXPLAIN (SELECT 1)`): an option's name is never one of these words.
    let opens_statement = |t: Option<&Token>| {
        t.is_none_or(|t| t.kind == Tok::LParen || ["SELECT", "VALUES", "WITH", "TABLE"].iter().any(|w| word(t, w)))
    };
    if rest.first().is_some_and(|t| t.kind == Tok::LParen) && !opens_statement(rest.get(1)) {
        return options(sql, rest).map(|j| JsonExplain { evaluates: j.analyze || execute, ..j });
    }
    // `EXPLAIN [ANALYZE | ANALYSE] [VERBOSE] statement`.
    let mut i = 0;
    let analyze = rest.first().is_some_and(|t| word(t, "ANALYZE") || word(t, "ANALYSE"));
    i += usize::from(analyze);
    let verbose = rest.get(i).is_some_and(|t| word(t, "VERBOSE"));
    i += usize::from(verbose);
    if !statement_at(i) {
        return Err(NotJson::Unreadable);
    }
    let mut list: Vec<&str> = Vec::new();
    if analyze {
        list.push("ANALYZE");
    }
    if verbose {
        list.push("VERBOSE");
    }
    list.push("FORMAT JSON");
    // The words become options; what is around them (comments, a planner hint) stays where it
    // was, before the statement. A word goes with the blanks after it.
    let mut out = format!("{} ({})", &sql[..explain.end], list.join(", "));
    let mut at = explain.end;
    for w in &rest[..i] {
        out.push_str(&sql[at..w.start]);
        at = w.end
            + sql[w.end..].chars().take_while(|c| crate::sql::lexer::is_space(*c)).map(char::len_utf8).sum::<usize>();
    }
    out.push_str(&sql[at..]);
    Ok(JsonExplain { sql: out, analyze, evaluates: analyze || execute })
}

/// `EXPLAIN (option [value], …) statement` (`rest`: the tokens after `EXPLAIN`, from the `(`):
/// each `FORMAT` option becomes `FORMAT JSON`, or `FORMAT JSON` is added after the last one.
/// The last of an option counts, as on the server.
fn options(sql: &str, rest: &[Token]) -> Result<JsonExplain, NotJson> {
    // The options, each as its tokens, up to the `)` that closes the list.
    let mut list: Vec<Vec<Token>> = vec![Vec::new()];
    let mut depth = 0usize;
    let mut close = None;
    for (i, t) in rest.iter().enumerate().skip(1) {
        match t.kind {
            Tok::RParen if depth == 0 => {
                close = Some(i);
                break;
            }
            Tok::Comma if depth == 0 => list.push(Vec::new()),
            Tok::Semi => break,
            _ => {
                depth = match t.kind {
                    Tok::LParen => depth + 1,
                    Tok::RParen => depth - 1,
                    _ => depth,
                };
                list.last_mut().expect("one at least").push(*t);
            }
        }
    }
    let Some(close) = close else { return Err(NotJson::Unreadable) };
    if list.iter().any(Vec::is_empty) || !rest.get(close + 1).is_some_and(|t| t.kind != Tok::Semi) {
        return Err(NotJson::Unreadable);
    }
    let mut analyze = false;
    let mut format = None;
    for o in &list {
        let value = o.get(1).map(|t| value(sql, t));
        match name(sql, &o[0]).as_deref() {
            Some("analyze" | "analyse") => {
                analyze = !matches!(value.as_deref(), Some("false" | "off" | "0"));
            }
            Some("format") => format = value,
            _ => {}
        }
    }
    if format.as_deref() == Some("json") {
        return Err(NotJson::AlreadyJson);
    }
    // Each `FORMAT` replaced where it is (comments around it stay), or one added at the end.
    let mut out = String::with_capacity(sql.len() + 16);
    let mut at = 0;
    let formats: Vec<&Vec<Token>> = list.iter().filter(|o| name(sql, &o[0]).as_deref() == Some("format")).collect();
    for o in &formats {
        let (start, end) = (o[0].start, o[o.len() - 1].end);
        out.push_str(&sql[at..start]);
        out.push_str("FORMAT JSON");
        at = end;
    }
    if formats.is_empty() {
        let last = list.last().expect("one at least");
        let end = last[last.len() - 1].end;
        out.push_str(&sql[at..end]);
        out.push_str(", FORMAT JSON");
        at = end;
    }
    out.push_str(&sql[at..]);
    Ok(JsonExplain { sql: out, analyze, evaluates: analyze })
}

/// An option's name as the server compares it: a word in lower case, a quoted name as it is.
fn name(sql: &str, t: &Token) -> Option<String> {
    match t.kind {
        Tok::Keyword | Tok::Ident => Some(t.text(sql).to_ascii_lowercase()),
        Tok::QuotedIdent => Some(unquote(t.text(sql), '"')),
        _ => None,
    }
}

/// An option's value, in lower case (the server reads a boolean in any case): the text of a
/// quoted name or a string, a word or a number as written.
fn value(sql: &str, t: &Token) -> String {
    let text = t.text(sql);
    let v = match t.kind {
        Tok::QuotedIdent => unquote(text, '"'),
        Tok::Str if text.starts_with('\'') => unquote(text, '\''),
        // `$tag$…$tag$`: the text between the tags.
        Tok::Dollar => {
            let tag = text[1..].find('$').map_or(text.len(), |i| i + 2);
            text.get(tag..text.len().saturating_sub(tag)).unwrap_or_default().to_string()
        }
        _ => text.to_string(),
    };
    v.to_ascii_lowercase()
}

/// The text between the quotes `q`, a doubled quote read as one.
fn unquote(text: &str, q: char) -> String {
    let inner = text.strip_prefix(q).unwrap_or(text);
    let inner = inner.strip_suffix(q).unwrap_or(inner);
    inner.replace(&format!("{q}{q}"), &q.to_string())
}

#[cfg(test)]
mod tests;
