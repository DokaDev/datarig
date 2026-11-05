//! Which language a tab's text is written in. The SQL tools (lexing, splitting, completion,
//! formatting, quoting, the risk classifier) pick their rules from it; a driver says which one
//! its sessions speak (`driver::Capabilities::language`).
//!
//! A dialect holds what each tool needs to know about it: its keywords, the search path a
//! session starts with ([`Dialect::default_path`]), the comment markers the editor writes and
//! strips and the `sqlformat` dialect it is laid out as. How a statement's plan is asked for
//! is the plan format's (`driver::ExplainFormat`), not the dialect's.
//!
//! A dialect also says how names and strings are written in its SQL ([`Dialect::quote_ident`],
//! [`Dialect::quote_literal`]) and how the server reads a name back ([`Dialect::fold`],
//! [`Dialect::unquote`]): every piece of SQL the app writes, and every name it reads out of
//! SQL text, goes through these.

use super::{ident, lexer};

/// The SQL dialect a tab's text is in. An exhaustive `match` makes every tool decide what to do
/// with each.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Dialect {
    #[default]
    Postgres,
    /// MySQL, read as a session in that sql mode reads it.
    MySql(MySqlMode),
}

/// What of a MySQL session changes how its text is read and how strings are written: parts of
/// its `sql_mode`, and whether its server reads dollar quotes. The default is a default MySQL
/// 8.0 server's: `"x"` is a string, a backslash escapes the next character in a string, and
/// `$$` is no quote.
///
/// Until a session says otherwise, text is read in the default mode. On a session whose mode
/// differs, a string the app writes ([`Dialect::quote_literal`]) is read otherwise (under
/// `NO_BACKSLASH_ESCAPES` a backslash written doubled stays doubled), and statements may be cut
/// elsewhere. So a MySQL driver must give the dialect the mode its session has before the app
/// writes or splits SQL for it, and again whenever it changes: MySQL's client reads
/// `NO_BACKSLASH_ESCAPES` from the server's status flags after every statement, as a `SET
/// sql_mode` the user runs changes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct MySqlMode {
    /// `ANSI_QUOTES`: `"x"` is a quoted name, not a string.
    pub ansi_quotes: bool,
    /// `NO_BACKSLASH_ESCAPES`: a backslash in a string is an ordinary character.
    pub no_backslash_escapes: bool,
    /// The server reads dollar quotes (`$$…$$`, `$tag$…$tag$`): MySQL's client asks it with
    /// `select $$` (a syntax error there means yes: MySQL 8.4 and 9.x; 8.0 reads `$$` as a name)
    /// and then does not end a statement inside one.
    pub dollar_quotes: bool,
}

impl Dialect {
    /// The character the app quotes an identifier with (PostgreSQL: `"`). Inside a quoted name
    /// it is written twice.
    pub fn ident_quote(self) -> char {
        match self {
            Self::Postgres => '"',
            Self::MySql(_) => '`',
        }
    }

    /// Every character an identifier may be quoted with when it is read (PostgreSQL: `"` only;
    /// MySQL: `` ` ``, and `"` under `ANSI_QUOTES`; a dialect may accept more than the one it
    /// writes with, [`Dialect::ident_quote`]).
    pub fn ident_quotes(self) -> &'static [char] {
        match self {
            Self::Postgres => &['"'],
            Self::MySql(m) if m.ansi_quotes => &['`', '"'],
            Self::MySql(_) => &['`'],
        }
    }

    /// The quote character `token` opens with, if it opens a quoted identifier
    /// ([`Dialect::ident_quotes`]).
    pub fn opening_quote(self, token: &str) -> Option<char> {
        token.chars().next().filter(|c| self.ident_quotes().contains(c))
    }

    /// Whether `name` must be quoted to be read back as that same name: it is not what the
    /// server folds a bare name to, or it is a keyword that cannot stand bare
    /// (PostgreSQL: [`ident::needs_quotes`]; MySQL: [`ident::mysql::needs_quotes`]).
    pub fn needs_quotes(self, name: &str) -> bool {
        match self {
            Self::Postgres => ident::needs_quotes(name),
            Self::MySql(_) => ident::mysql::needs_quotes(name),
        }
    }

    /// `name` as SQL: bare when that reads back as the same name ([`Dialect::needs_quotes`]),
    /// else quoted ([`Dialect::force_quote_ident`]): `Mixed Col` → `"Mixed Col"`.
    pub fn quote_ident(self, name: &str) -> String {
        if self.needs_quotes(name) { self.force_quote_ident(name) } else { name.to_string() }
    }

    /// `name` as SQL, always quoted (any name, any case, keywords), the quote character
    /// doubled inside: `Order "Items"` → `"Order ""Items"""` (MySQL: `` `Order "Items"` ``).
    pub fn force_quote_ident(self, name: &str) -> String {
        match self {
            Self::Postgres => format!("\"{}\"", name.replace('"', "\"\"")),
            Self::MySql(_) => format!("`{}`", name.replace('`', "``")),
        }
    }

    /// `s` as a string literal. PostgreSQL: `'` doubled, written for
    /// `standard_conforming_strings = on` (the default since 9.1), where a backslash is an
    /// ordinary character. MySQL: `'` doubled, and (unless `NO_BACKSLASH_ESCAPES`, see
    /// [`MySqlMode`]) a backslash doubled and a NUL written `\0`, as the server reads a
    /// backslash as an escape.
    pub fn quote_literal(self, s: &str) -> String {
        let open = LiteralOpen { prefix: 0, quote: '\'', backslash: self.backslash_strings() };
        format!("'{}'", self.literal_body(s, open))
    }

    /// A backslash escapes the next character in a plain `'…'` string (MySQL unless
    /// `NO_BACKSLASH_ESCAPES`; never in PostgreSQL, see [`Dialect::quote_literal`]).
    fn backslash_strings(self) -> bool {
        match self {
            Self::Postgres => false,
            Self::MySql(m) => !m.no_backslash_escapes,
        }
    }

    /// The characters a string literal may be quoted with (PostgreSQL: `'`; MySQL: `'`, and
    /// `"` unless `ANSI_QUOTES` makes it a name's quote).
    pub fn string_quotes(self) -> &'static [char] {
        match self {
            Self::Postgres => &['\''],
            Self::MySql(m) if m.ansi_quotes => &['\''],
            Self::MySql(_) => &['\'', '"'],
        }
    }

    /// How string literal `token` (a lexer's string token) opens, when it holds a plain value:
    /// its prefix, its quote and whether a backslash escapes in it. `before` is the text right
    /// before the token. `None` for the strings that hold no such value: bit and hex strings
    /// (`B'…'`, `X'…'`), PostgreSQL's Unicode escapes (`U&'…'`), dollar quotes, and anything
    /// that is not a string. PostgreSQL: `'…'` and `N'…'` (no backslash escapes), `E'…'`
    /// (backslash escapes). MySQL: `'…'`, `"…"` (without `ANSI_QUOTES`) and `N'…'`, a backslash
    /// escaping unless `NO_BACKSLASH_ESCAPES`.
    pub fn literal_open(self, before: &str, token: &str) -> Option<LiteralOpen> {
        let mut chars = token.chars();
        let first = chars.next()?;
        let (prefix, quote) = if self.string_quotes().contains(&first) {
            (0, first)
        } else {
            (first.len_utf8(), chars.next().filter(|&q| q == '\'')?)
        };
        let backslash = match (self, prefix, first.to_ascii_uppercase()) {
            (_, 0, _) | (_, _, 'N') => self.backslash_strings(),
            (Self::Postgres, _, 'E') => true,
            _ => return None,
        };
        // PostgreSQL's `U&'…'` is `U`, `&` and a string to the lexer.
        if self == Self::Postgres
            && prefix == 0
            && let Some(rest) = before.strip_suffix('&')
            && rest.ends_with(['U', 'u'])
            && !rest[..rest.len() - 1].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$')
        {
            return None;
        }
        Some(LiteralOpen { prefix, quote, backslash })
    }

    /// `s` written inside a string literal that opens as `open` (without its quotes): its
    /// quote doubled, and with backslash escapes a backslash doubled and a NUL written `\0`.
    /// [`Dialect::quote_literal`] is a plain literal's.
    pub fn literal_body(self, s: &str, open: LiteralOpen) -> String {
        let mut out = String::with_capacity(s.len());
        for c in s.chars() {
            match c {
                c if c == open.quote => {
                    out.push(c);
                    out.push(c);
                }
                '\\' if open.backslash => out.push_str("\\\\"),
                '\0' if open.backslash => out.push_str("\\0"),
                c => out.push(c),
            }
        }
        out
    }

    /// The value the text `body` between the quotes of a literal that opens as `open` stands
    /// for: a doubled quote is one, and with backslash escapes each escape is its character
    /// (PostgreSQL's `E'…'`: `\n`, `\t`, octal, `\x`, `\u` and the others; MySQL's: `\0`, `\n`,
    /// `\Z`, … and `\%`, `\_` kept as written). Lenient: an escape cut short (a literal being
    /// typed) is kept as written.
    pub fn unescape_literal_body(self, body: &str, open: LiteralOpen) -> String {
        let mut out = String::with_capacity(body.len());
        let mut chars = body.chars().peekable();
        while let Some(c) = chars.next() {
            if c == open.quote {
                out.push(c);
                if chars.peek() == Some(&c) {
                    chars.next();
                }
            } else if c == '\\' && open.backslash {
                let Some(e) = chars.next() else {
                    out.push(c);
                    break;
                };
                match self {
                    Self::Postgres => unescape_pg(e, &mut chars, &mut out),
                    Self::MySql(_) => match e {
                        '0' => out.push('\0'),
                        'b' => out.push('\x08'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'Z' => out.push('\x1a'),
                        '%' | '_' => {
                            out.push('\\');
                            out.push(e);
                        }
                        e => out.push(e),
                    },
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    /// The value string literal `token` holds ([`Dialect::literal_open`], with nothing before
    /// it), `None` unless it is such a literal and closed.
    pub fn unquote_literal(self, token: &str) -> Option<String> {
        let open = self.literal_open("", token)?;
        let body = token.get(open.prefix + open.quote.len_utf8()..)?.strip_suffix(open.quote)?;
        // The last quote must close it: not one half of a doubled quote, nor escaped.
        let lexed = super::lexer::lex_in(&format!("{token} "), self);
        lexed.first().filter(|t| t.end == token.len())?;
        Some(self.unescape_literal_body(body, open))
    }

    /// The dialect of the SQL a server shows of its own (a check's condition, a column's
    /// type): MySQL writes it as its default mode reads it (a quote in a string escaped `\'`,
    /// names in backticks), whatever the session's mode.
    pub fn printed(self) -> Self {
        match self {
            Self::Postgres => Self::Postgres,
            Self::MySql(_) => Self::MySql(MySqlMode::default()),
        }
    }

    /// The name an unquoted identifier stands for: PostgreSQL folds ASCII letters to lower
    /// case (`Shop` is `shop`; other letters stay as written). MySQL keeps it as written
    /// (whether names that differ in case are the same table is the server's
    /// `lower_case_table_names`).
    pub fn fold(self, name: &str) -> String {
        match self {
            Self::Postgres => name.to_ascii_lowercase(),
            Self::MySql(_) => name.to_string(),
        }
    }

    /// The name a quoted identifier `token` stands for (`"Mixed ""Q"""` → `Mixed "Q"`): the text
    /// between its first and last character, each doubled quote one. `None` unless `token` is at
    /// least two characters long and starts and ends with the same quote character (no closing
    /// one, as while it is being typed). What lies between is not checked: `"a""` (whose last
    /// quote is escaped, so it is not closed) is read as `a"`.
    pub fn unquote(self, token: &str) -> Option<String> {
        let q = self.opening_quote(token)?;
        let closed = token.len() >= 2 && token.ends_with(q);
        closed.then(|| self.unescape_ident(q, &token[q.len_utf8()..token.len() - q.len_utf8()]))
    }

    /// [`Dialect::unquote`] that takes what it gets: the opening quote character is dropped at
    /// either end, however many times it is there (a name being typed has no closing one yet);
    /// another quote character the dialect reads stays (`` `say "hi"` `` is `say "hi"`).
    pub fn unquote_lenient(self, token: &str) -> String {
        let q = self.opening_quote(token).unwrap_or(self.ident_quote());
        self.unescape_ident(q, token.trim_matches(q))
    }

    /// The text inside identifier quotes `quote` as the name it stands for: each doubled quote
    /// character is one.
    pub fn unescape_ident(self, quote: char, inner: &str) -> String {
        inner.replace(&format!("{quote}{quote}"), &quote.to_string())
    }

    /// The words the lexer reads as keywords (the highlighter shows them, completion offers
    /// them): sorted, upper case. PostgreSQL: [`lexer::KEYWORDS`]; MySQL:
    /// [`lexer::mysql::KEYWORDS`].
    pub fn keywords(self) -> &'static [&'static str] {
        match self {
            Self::Postgres => lexer::KEYWORDS,
            Self::MySql(_) => lexer::mysql::KEYWORDS,
        }
    }

    /// Whether `word` is one of [`Dialect::keywords`], whatever its case.
    pub fn is_keyword(self, word: &str) -> bool {
        let upper = word.to_ascii_uppercase();
        self.keywords().binary_search(&upper.as_str()).is_ok()
    }

    /// The schemas an unqualified name is looked up in, in order, for a session in `schema`
    /// (`None`: the server's default). PostgreSQL: the search path, `public` (its default
    /// without the `$user` schema) after the session's schema, as an extension's objects there
    /// resolve unqualified; `pg_catalog` is implicitly first on the server and is not listed.
    /// MySQL: the session's database (a database is the schema), none without one.
    pub fn default_path(self, schema: Option<&str>) -> Vec<String> {
        match self {
            Self::Postgres => {
                let mut path: Vec<String> = schema.into_iter().map(str::to_string).collect();
                if schema != Some("public") {
                    path.push("public".to_string());
                }
                path
            }
            Self::MySql(_) => schema.into_iter().map(str::to_string).collect(),
        }
    }

    /// What starts a line comment the editor writes: `--` (followed by a blank, or the end of
    /// the line, which MySQL needs).
    pub fn comment_marker(self) -> &'static str {
        match self {
            Self::Postgres | Self::MySql(_) => "--",
        }
    }

    /// The line comment marker `text` starts with, when a line comment starts there: what the
    /// editor strips to uncomment a line. PostgreSQL: `--`. MySQL: `#`, or `--` followed by a
    /// blank, a control character or the end (`--1` is code: minus minus one).
    pub fn line_comment_at(self, text: &str) -> Option<&'static str> {
        match self {
            Self::Postgres => text.starts_with("--").then_some("--"),
            Self::MySql(_) if text.starts_with('#') => Some("#"),
            Self::MySql(_) => text
                .strip_prefix("--")
                .is_some_and(|rest| rest.chars().next().is_none_or(|c| c <= ' ' || c == '\x7f'))
                .then_some("--"),
        }
    }

    /// The dialect `sqlformat` lays text of this dialect out as (MySQL: the generic one; it has
    /// none of MySQL's own).
    pub fn sqlformat_dialect(self) -> sqlformat::Dialect {
        match self {
            Self::Postgres => sqlformat::Dialect::PostgreSql,
            Self::MySql(_) => sqlformat::Dialect::Generic,
        }
    }
}

/// How a string literal opens ([`Dialect::literal_open`]): the bytes of its prefix before the
/// quote (`E'…'`: 1), its quote character, and whether a backslash escapes in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiteralOpen {
    pub prefix: usize,
    pub quote: char,
    pub backslash: bool,
}

/// PostgreSQL's escape `\e` in an `E'…'` string (the backslash read), the characters after it
/// taken from `chars` as it needs them.
fn unescape_pg(e: char, chars: &mut std::iter::Peekable<std::str::Chars>, out: &mut String) {
    let mut digits = |radix: u32, max: usize| {
        let mut n = 0u32;
        let mut read = String::new();
        while read.len() < max
            && let Some(d) = chars.peek().and_then(|c| c.to_digit(radix))
        {
            n = n * radix + d;
            read.push(chars.next().unwrap_or_default());
        }
        (n, read)
    };
    match e {
        'b' => out.push('\x08'),
        'f' => out.push('\x0c'),
        'n' => out.push('\n'),
        'r' => out.push('\r'),
        't' => out.push('\t'),
        '0'..='7' => {
            let (n, read) = digits(8, 2);
            let n = (e.to_digit(8).unwrap_or(0) << (3 * read.len() as u32)) + n;
            out.extend(char::from_u32(n));
        }
        'x' | 'u' | 'U' => {
            let max = match e {
                'x' => 2,
                'u' => 4,
                _ => 8,
            };
            match digits(16, max) {
                (_, read) if read.is_empty() => out.push(e),
                (n, read) => match char::from_u32(n) {
                    Some(c) => out.push(c),
                    None => {
                        out.push('\\');
                        out.push(e);
                        out.push_str(&read);
                    }
                },
            }
        }
        e => out.push(e),
    }
}

/// A constant a server printed as a column's default ([`Dialect::constant`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Constant {
    /// `NULL`.
    Null,
    /// A string's value (unquoted), a number, `true` or `false`, as written.
    Value(String),
}

impl Dialect {
    /// `sql` without the parentheses around the whole of it (`((1))` is `1`; `(a) + (b)` keeps
    /// them), trimmed.
    pub fn unparenthesized(self, sql: &str) -> &str {
        let mut s = sql.trim();
        loop {
            let toks: Vec<lexer::Token> = lexer::lex_in(s, self).into_iter().filter(|t| !t.is_trivia()).collect();
            let (Some(first), Some(last)) = (toks.first(), toks.last()) else { return s };
            if toks.len() < 2 || first.kind != lexer::Tok::LParen || last.kind != lexer::Tok::RParen {
                return s;
            }
            // The first parenthesis must close at the very end.
            let mut depth = 0usize;
            for (i, t) in toks.iter().enumerate() {
                match t.kind {
                    lexer::Tok::LParen => depth += 1,
                    lexer::Tok::RParen => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 && i + 1 < toks.len() {
                            return s;
                        }
                    }
                    _ => {}
                }
            }
            s = s[first.end..last.start].trim();
        }
    }

    /// The constant a column default is, read as SQL of this dialect: a string literal (in
    /// MySQL also after its character set's introducer, `_utf8mb4'x'`), a number (with its
    /// sign), `true`, `false` or `NULL`, in parentheses or not; in PostgreSQL also with casts
    /// after it (`'x'::character varying`, `'{}'::jsonb`, `NULL::text`), the constant in
    /// parentheses too (`(0)::numeric`). `None` for anything else, such as a call (`now()`,
    /// `CURRENT_TIMESTAMP`) or an operator, and for a PostgreSQL string cast to a type other
    /// than text that is one of the special date and time inputs (`'now'::date`,
    /// `('now'::text)::date`): those are read when a row is inserted, not when the default was
    /// set.
    pub fn constant(self, sql: &str) -> Option<Constant> {
        use lexer::Tok;
        let sql = self.unparenthesized(sql);
        let toks: Vec<lexer::Token> = lexer::lex_in(sql, self).into_iter().filter(|t| !t.is_trivia()).collect();
        let text = |t: &lexer::Token| t.text(sql);
        let word =
            |t: &lexer::Token, w: &str| matches!(t.kind, Tok::Keyword | Tok::Ident) && text(t).eq_ignore_ascii_case(w);
        // MySQL: a string's character set introducer (`_utf8mb4'x'`) goes.
        let introducer = |i: &lexer::Token, s: &lexer::Token| {
            matches!(self, Self::MySql(_)) && i.kind == Tok::Ident && text(i).starts_with('_') && i.end == s.start
        };
        let (value, rest) = match toks.as_slice() {
            // A constant in parentheses with a cast after them (PostgreSQL prints `0::numeric`
            // as `(0)::numeric`).
            [open, ..] if open.kind == Tok::LParen => {
                let mut depth = 0usize;
                let close = toks.iter().position(|t| {
                    match t.kind {
                        Tok::LParen => depth += 1,
                        Tok::RParen => depth = depth.saturating_sub(1),
                        _ => {}
                    }
                    t.kind == Tok::RParen && depth == 0
                })?;
                (self.constant(&sql[open.end..toks[close].start])?, &toks[close + 1..])
            }
            [i, s, rest @ ..] if s.kind == Tok::Str && introducer(i, s) => {
                (Constant::Value(self.unquote_literal(text(s))?), rest)
            }
            [s, rest @ ..] if s.kind == Tok::Str => {
                let t = text(s);
                // An introducer the lexer took into the string's token.
                let t = match self {
                    Self::MySql(_) if t.starts_with('_') => &t[t.find(self.string_quotes())?..],
                    _ => t,
                };
                (Constant::Value(self.unquote_literal(t)?), rest)
            }
            [sign, n, rest @ ..]
                if sign.kind == Tok::Op && matches!(text(sign), "-" | "+") && n.kind == Tok::Number =>
            {
                let sign = if text(sign) == "-" { "-" } else { "" };
                (Constant::Value(format!("{sign}{}", text(n))), rest)
            }
            [n, rest @ ..] if n.kind == Tok::Number => (Constant::Value(text(n).to_string()), rest),
            [b, rest @ ..] if word(b, "true") || word(b, "false") => {
                (Constant::Value(text(b).to_ascii_lowercase()), rest)
            }
            [n, rest @ ..] if word(n, "null") => (Constant::Null, rest),
            _ => return None,
        };
        let cast = |rest: &[lexer::Token]| {
            let starts = rest.len() >= 2 && rest[..2].iter().all(|t| t.kind == Tok::Op && text(t) == ":");
            starts
                && rest.iter().all(|t| match t.kind {
                    Tok::Op => matches!(text(t), ":" | "[" | "]"),
                    Tok::Ident | Tok::Keyword | Tok::QuotedIdent | Tok::Number => true,
                    Tok::LParen | Tok::RParen | Tok::Comma | Tok::Dot => true,
                    _ => false,
                })
        };
        let whole = match self {
            Self::Postgres => rest.is_empty() || cast(rest),
            Self::MySql(_) => rest.is_empty(),
        };
        // `'now'::date` is the time of the insert: no constant (fail-safe: any type but text).
        let special = |v: &str| {
            let v = v.trim().to_ascii_lowercase();
            ["now", "today", "tomorrow", "yesterday", "epoch", "infinity", "-infinity", "allballs"]
                .contains(&v.as_str())
        };
        let text_types = ["text", "character", "varying", "varchar", "char", "bpchar", "name"];
        let to_other = rest.iter().any(|t| {
            matches!(t.kind, Tok::Ident | Tok::Keyword | Tok::QuotedIdent)
                && !text_types.contains(&t.text(sql).to_ascii_lowercase().as_str())
        });
        if self == Self::Postgres && to_other && matches!(&value, Constant::Value(v) if special(v)) {
            return None;
        }
        whole.then_some(value)
    }

    /// Whether a column default the server printed draws the next number of a sequence
    /// (PostgreSQL: `nextval(…)`, a `serial` column's). MySQL's `AUTO_INCREMENT` is no default.
    pub fn draws_number(self, default: &str) -> bool {
        match self {
            Self::Postgres => {
                let toks: Vec<lexer::Token> =
                    lexer::lex_in(default, self).into_iter().filter(|t| !t.is_trivia()).collect();
                matches!(toks.as_slice(), [f, p, ..] if f.text(default).eq_ignore_ascii_case("nextval") && p.kind == lexer::Tok::LParen)
            }
            Self::MySql(_) => false,
        }
    }

    /// Of a column default as the app reads it from the structure, what a new row takes: in
    /// MySQL without what an `UPDATE` sets (`CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP`).
    pub fn insert_default(self, default: &str) -> &str {
        match self {
            Self::Postgres => default,
            Self::MySql(_) => {
                // `ON UPDATE` as words, not inside a string.
                let toks: Vec<lexer::Token> =
                    lexer::lex_in(default, self).into_iter().filter(|t| !t.is_trivia()).collect();
                let at = toks.windows(2).find(|w| {
                    w[0].text(default).eq_ignore_ascii_case("on") && w[1].text(default).eq_ignore_ascii_case("update")
                });
                at.map_or(default, |w| default[..w[0].start].trim_end())
            }
        }
    }
}

/// The language of an editor's text: SQL of some dialect (other kinds of consoles may join
/// it later).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Sql(Dialect),
}

impl Default for Language {
    /// PostgreSQL SQL: what a tab without a profile (or with one whose driver is unknown) is
    /// written in.
    fn default() -> Self {
        Self::Sql(Dialect::default())
    }
}

impl Language {
    /// The SQL dialect of the text.
    pub fn dialect(self) -> Dialect {
        match self {
            Self::Sql(d) => d,
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mysql_tests;
