//! Filter text read back into a [`Cond`], for the grammar the app models and nothing more.
//!
//! The text is split at its top-level `OR`s, then at its `AND`s (not the one of a `BETWEEN`),
//! then a leading `NOT` and parentheses around a whole part are read; each part left is one
//! comparison of a known column with a literal ([`Cond::Cmp`], `IS [NOT] NULL`, `IS [NOT]
//! DISTINCT FROM` and MySQL's `<=>`, `[NOT] IN`, `[NOT] LIKE … [ESCAPE …]`, PostgreSQL's
//! `ILIKE`, `LOWER(col) LIKE LOWER(…)`, `[NOT] BETWEEN`), or else it is kept as written
//! ([`Cond::Raw`]). A part that is empty, or text the filter bar refuses (an unclosed
//! parenthesis or string, a comment, a `;`), reads as nothing at all: `None`.
//!
//! What [`render`](super::render) writes reads back as the same condition, with these
//! exceptions: a number written as a string (for a column that is not a number) reads as text;
//! a NULL in an `IN` list reads as its own test of NULL; on MySQL a case-insensitive pattern of a
//! `_ci` column reads as the server's case, and `NOT` of a NULL-safe equality as a NULL-safe
//! inequality.

use super::check::lexical;
use super::{CaseMode, CmpOp, ColName, Cond, Value};
use crate::sql::dialect::Dialect;
use crate::sql::lexer::{Tok, Token, lex_in};

/// What `text` (filter text of dialect `d` on a table with columns `cols`) says, as far as the
/// app models it; `None` when it cannot be read (see the module docs).
pub fn recognise(text: &str, d: Dialect, cols: &[ColName]) -> Option<Cond> {
    lexical(text, d).ok()?;
    let toks: Vec<Token> = lex_in(text, d).into_iter().filter(|t| !t.is_trivia()).collect();
    Reader { src: text, d, cols }.or(&toks)
}

struct Reader<'a> {
    src: &'a str,
    d: Dialect,
    cols: &'a [ColName],
}

impl Reader<'_> {
    fn text(&self, t: &Token) -> &str {
        t.text(self.src)
    }

    fn word(&self, t: Option<&Token>, w: &str) -> bool {
        t.is_some_and(|t| t.is_word() && self.text(t).eq_ignore_ascii_case(w))
    }

    fn or(&self, toks: &[Token]) -> Option<Cond> {
        let parts = self.split(toks, "OR");
        if parts.len() == 1 {
            return self.and(toks);
        }
        Some(Cond::Any(parts.into_iter().map(|p| self.and(p)).collect::<Option<_>>()?))
    }

    fn and(&self, toks: &[Token]) -> Option<Cond> {
        let parts = self.split(toks, "AND");
        if parts.len() == 1 {
            return self.not(toks);
        }
        Some(Cond::All(parts.into_iter().map(|p| self.not(p)).collect::<Option<_>>()?))
    }

    /// `toks` cut at each `word` outside parentheses (for `AND`, not the one of a `BETWEEN`).
    fn split<'t>(&self, toks: &'t [Token], word: &str) -> Vec<&'t [Token]> {
        let mut parts = Vec::new();
        let (mut depth, mut start, mut between) = (0usize, 0, false);
        for (i, t) in toks.iter().enumerate() {
            match t.kind {
                Tok::LParen => depth += 1,
                Tok::RParen => depth = depth.saturating_sub(1),
                _ if depth > 0 => {}
                _ if self.word(Some(t), "BETWEEN") => between = true,
                _ if self.word(Some(t), word) => {
                    if word == "AND" && between {
                        between = false;
                    } else {
                        parts.push(&toks[start..i]);
                        start = i + 1;
                    }
                }
                _ => {}
            }
        }
        parts.push(&toks[start..]);
        parts
    }

    fn not(&self, toks: &[Token]) -> Option<Cond> {
        if !self.word(toks.first(), "NOT") {
            return self.primary(toks);
        }
        let rest = &toks[1..];
        // MySQL's NULL-safe inequality: `NOT (col <=> value)`.
        if let Some(inner) = self.group(rest)
            && let Some(Cond::NotDistinct { col, value, negated: false }) = self.predicate(inner)
            && matches!(self.d, Dialect::MySql(_))
        {
            return Some(Cond::NotDistinct { col, value, negated: true });
        }
        Some(Cond::Not(Box::new(self.not(rest)?)))
    }

    fn primary(&self, toks: &[Token]) -> Option<Cond> {
        let (first, last) = (toks.first()?, toks.last()?);
        if let Some(inner) = self.group(toks) {
            return self.or(inner);
        }
        Some(self.predicate(toks).unwrap_or_else(|| Cond::Raw(self.src[first.start..last.end].to_string())))
    }

    /// What is inside the parentheses, when one pair encloses the whole of `toks`.
    fn group<'t>(&self, toks: &'t [Token]) -> Option<&'t [Token]> {
        let (first, last) = (toks.first()?, toks.last()?);
        if toks.len() < 2 || first.kind != Tok::LParen || last.kind != Tok::RParen {
            return None;
        }
        let mut depth = 0usize;
        for (i, t) in toks.iter().enumerate() {
            match t.kind {
                Tok::LParen => depth += 1,
                Tok::RParen => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 && i + 1 < toks.len() {
                        return None;
                    }
                }
                _ => {}
            }
        }
        Some(&toks[1..toks.len() - 1])
    }

    /// The column `t` names, as the catalog has it: a quoted name exactly, a bare one as the
    /// server folds it (MySQL: column names compare without case).
    fn column(&self, t: &Token) -> Option<ColName> {
        let text = self.text(t);
        let name = match t.kind {
            Tok::QuotedIdent => self.d.unquote(text)?,
            Tok::Ident | Tok::Keyword => self.d.fold(text),
            _ => return None,
        };
        match self.d {
            Dialect::Postgres => self.cols.iter().find(|c| **c == name).cloned(),
            Dialect::MySql(_) => self.cols.iter().find(|c| c.eq_ignore_ascii_case(&name)).cloned(),
        }
    }

    /// The operator made of the adjacent operator characters from `at`, and the index after it.
    fn operator(&self, toks: &[Token], at: usize) -> (String, usize) {
        let mut op = String::new();
        let mut i = at;
        while let Some(t) = toks.get(i).filter(|t| t.kind == Tok::Op && (i == at || toks[i - 1].end == t.start)) {
            op.push_str(self.text(t));
            i += 1;
        }
        (op, i)
    }

    /// A literal starting at `at`, and the index after it.
    fn value(&self, toks: &[Token], at: usize) -> Option<(Value, usize)> {
        let t = toks.get(at)?;
        let text = self.text(t);
        if t.is_word() {
            let v = match text.to_ascii_uppercase().as_str() {
                "NULL" => Value::Null,
                "TRUE" => Value::Bool(true),
                "FALSE" => Value::Bool(false),
                // MySQL text written as its UTF-8 bytes: `_utf8mb4 X'…'`.
                "_UTF8MB4" if matches!(self.d, Dialect::MySql(_)) => {
                    let bytes = self.hex_string(toks.get(at + 1)?)?;
                    return Some((Value::Text(String::from_utf8(bytes).ok()?), at + 2));
                }
                _ => return None,
            };
            return Some((v, at + 1));
        }
        match t.kind {
            Tok::Number => Some((Value::Number(text.to_string()), at + 1)),
            Tok::Op if text == "-" || text == "+" => {
                let n = toks.get(at + 1).filter(|n| n.kind == Tok::Number && n.start == t.end)?;
                let sign = if text == "-" { "-" } else { "" };
                Some((Value::Number(format!("{sign}{}", self.text(n))), at + 2))
            }
            Tok::Str if matches!(self.d, Dialect::MySql(_)) && self.hex_string(t).is_some() => {
                Some((Value::Bytes(self.hex_string(t)?), at + 1))
            }
            Tok::Str => {
                let s = self.d.unquote_literal(text)?;
                // PostgreSQL bytes: `'\x…'::bytea`.
                let (cast, next) = self.operator(toks, at + 1);
                if self.d == Dialect::Postgres && cast == "::" && self.word(toks.get(next), "bytea") {
                    let bytes = s.strip_prefix("\\x").and_then(unhex)?;
                    return Some((Value::Bytes(bytes), next + 1));
                }
                Some((Value::Text(s), at + 1))
            }
            _ => None,
        }
    }

    /// The bytes of a MySQL hexadecimal string, `X'…'`.
    fn hex_string(&self, t: &Token) -> Option<Vec<u8>> {
        let text = self.text(t);
        let body = text.strip_prefix(['x', 'X'])?.strip_prefix('\'')?.strip_suffix('\'')?;
        (t.kind == Tok::Str).then_some(())?;
        unhex(body)
    }

    /// One comparison of a known column with literals, the whole of `toks`.
    fn predicate(&self, toks: &[Token]) -> Option<Cond> {
        // `LOWER(col) [NOT] LIKE LOWER('…') [ESCAPE '…']`: a case-insensitive pattern.
        if self.word(toks.first(), "LOWER")
            && toks.get(1).is_some_and(|t| t.kind == Tok::LParen)
            && toks.get(3).is_some_and(|t| t.kind == Tok::RParen)
        {
            let col = self.column(toks.get(2)?)?;
            let mut i = 4;
            let negated = self.word(toks.get(i), "NOT");
            i += usize::from(negated);
            if !self.word(toks.get(i), "LIKE")
                || !self.word(toks.get(i + 1), "LOWER")
                || toks.get(i + 2).is_none_or(|t| t.kind != Tok::LParen)
            {
                return None;
            }
            let (raw, close) = self.like_string(toks, i + 3)?;
            toks.get(close).filter(|t| t.kind == Tok::RParen)?;
            let pattern = self.pattern(toks, raw, close + 1)?;
            return Some(Cond::Like { col, pattern, negated, case: CaseMode::Insensitive });
        }
        let col = self.column(toks.first()?)?;
        let mut i = 1;
        let done = |i: usize| i == toks.len();
        if self.word(toks.get(i), "IS") {
            i += 1;
            let negated = self.word(toks.get(i), "NOT");
            i += usize::from(negated);
            if self.word(toks.get(i), "NULL") && done(i + 1) {
                return Some(Cond::IsNull { col, negated });
            }
            if self.word(toks.get(i), "DISTINCT") && self.word(toks.get(i + 1), "FROM") {
                let (value, end) = self.value(toks, i + 2)?;
                // `IS NOT DISTINCT FROM` is the NULL-safe equality.
                return done(end).then_some(Cond::NotDistinct { col, value, negated: !negated });
            }
            return None;
        }
        let negated = self.word(toks.get(i), "NOT");
        i += usize::from(negated);
        if self.word(toks.get(i), "IN") {
            let list = self.group(&toks[i + 1..])?;
            let mut values = Vec::new();
            let mut j = 0;
            while j < list.len() {
                let (v, end) = self.value(list, j)?;
                values.push(v);
                match list.get(end) {
                    None => j = end,
                    Some(c) if c.kind == Tok::Comma && end + 1 < list.len() => j = end + 1,
                    Some(_) => return None,
                }
            }
            return (!values.is_empty()).then_some(Cond::In { col, values, negated });
        }
        let like = self.word(toks.get(i), "LIKE");
        let ilike = self.d == Dialect::Postgres && self.word(toks.get(i), "ILIKE");
        if like || ilike {
            let (raw, next) = self.like_string(toks, i + 1)?;
            let pattern = self.pattern(toks, raw, next)?;
            let case = if ilike { CaseMode::Insensitive } else { CaseMode::Server };
            return Some(Cond::Like { col, pattern, negated, case });
        }
        if self.word(toks.get(i), "BETWEEN") {
            let (lo, end) = self.value(toks, i + 1)?;
            if !self.word(toks.get(end), "AND") {
                return None;
            }
            let (hi, end) = self.value(toks, end + 1)?;
            return done(end).then_some(Cond::Between { col, lo, hi, negated });
        }
        if negated {
            return None;
        }
        let (op, at) = self.operator(toks, i);
        // A sign glued to the operator belongs to the number (`a=-1`).
        let (op, at) = match op.strip_suffix(['-', '+']) {
            Some(rest) if CmpOp::from_sql(rest).is_some() || rest == "<=>" => (rest.to_string(), at - 1),
            _ => (op, at),
        };
        let (value, end) = self.value(toks, at)?;
        if !done(end) {
            return None;
        }
        if op == "<=>" && matches!(self.d, Dialect::MySql(_)) {
            return Some(Cond::NotDistinct { col, value, negated: false });
        }
        let op = CmpOp::from_sql(&op)?;
        (value != Value::Null).then_some(Cond::Cmp { col, op, value })
    }

    /// The string of a `LIKE` at `at` (any literal that holds text), and the index after it.
    fn like_string(&self, toks: &[Token], at: usize) -> Option<(String, usize)> {
        match self.value(toks, at)? {
            (Value::Text(s), next) => Some((s, next)),
            _ => None,
        }
    }

    /// Pattern `raw` of a `LIKE` whose `ESCAPE` clause, if any, starts at `escape` and ends the
    /// text, as a pattern escaped with `!`.
    fn pattern(&self, toks: &[Token], raw: String, escape: usize) -> Option<String> {
        if escape == toks.len() {
            return Some(rebang(&raw));
        }
        let e = toks.get(escape + 1).filter(|t| t.kind == Tok::Str)?;
        let bang = self.word(toks.get(escape), "ESCAPE") && escape + 2 == toks.len();
        (bang && self.d.unquote_literal(self.text(e))? == "!").then_some(raw)
    }
}

/// A pattern escaped with a backslash (the default escape) written with `!` instead.
fn rebang(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    let mut chars = p.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push('!');
                out.extend(chars.next());
            }
            '!' => out.push_str("!!"),
            c => out.push(c),
        }
    }
    out
}

/// Hexadecimal digits as bytes; `None` unless they are pairs of hexadecimal digits.
fn unhex(h: &str) -> Option<Vec<u8>> {
    if !h.len().is_multiple_of(2) || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).ok()).collect()
}
