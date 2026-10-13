//! Context-aware completion on top of the tolerant lexer. Works on incomplete SQL
//! because it only looks at the token stream of the `;`-segment around the cursor.
//!
//! The text is lexed, and names are read and written, in the dialect of the text
//! ([`complete_in_dialect`]). Names are offered as SQL: quoted when the server would not read
//! them back bare ([`Dialect::quote_ident`]), and always quoted after an opening quote. A name matches when the
//! typed prefix starts it; names the typed letters only appear in, in order, follow those.
//!
//! Inside a string literal compared with a column (`col = '…'`, `col <> '…'`, `col IN ('a',
//! '…')`, `SET col = '…'`), the values the column may hold are offered ([`complete_with`]):
//! those of its enumerated type ([`ColumnInfo::values`]), then those its table's checks list
//! ([`values::check_values`](super::values::check_values)). They are written into the literal
//! as the dialect escapes them there ([`Dialect::literal_body`]).

use super::dialect::{Dialect, LiteralOpen};
use super::lexer::{LexState, Tok, Token, is_space, lex_from};
use super::values::CheckValues;
use crate::i18n::Label;
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub schemas: Vec<String>,
    pub relations: Vec<Relation>,
}

#[derive(Clone, Debug)]
pub struct Relation {
    pub schema: String,
    pub name: String,
    pub is_view: bool,
    pub columns: Vec<ColumnInfo>,
}

#[derive(Clone, Debug)]
pub struct ColumnInfo {
    pub name: String,
    pub type_name: String,
    /// The values the column's type allows, in the type's order (a PostgreSQL enum, also
    /// through domains over one; a MySQL `ENUM`); `None` for any other type. Columns of one
    /// type share them.
    pub values: Option<Arc<[String]>>,
}

impl ColumnInfo {
    /// A column of type `type_name` with no values of its type ([`ColumnInfo::values`]).
    pub fn new(name: impl Into<String>, type_name: impl Into<String>) -> Self {
        Self { name: name.into(), type_name: type_name.into(), values: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Column,
    Schema,
    Table,
    View,
    Keyword,
    /// A value the column a string literal is compared with may hold.
    Value,
}

impl Kind {
    /// Catalog label of the kind (completion popup).
    pub fn label(self) -> Label {
        match self {
            Kind::Column => Label::CompleteKindColumn,
            Kind::Schema => Label::CompleteKindSchema,
            Kind::Table => Label::CompleteKindTable,
            Kind::View => Label::CompleteKindView,
            Kind::Keyword => Label::CompleteKindKeyword,
            Kind::Value => Label::CompleteKindValue,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub label: String,
    pub kind: Kind,
    pub detail: Option<String>,
    /// The text accepting it writes, when it is not the label: a value written inside its
    /// literal (`it''s` for `it's`).
    pub insert: Option<String>,
}

impl Candidate {
    /// The text accepting the candidate writes.
    pub fn text(&self) -> &str {
        self.insert.as_deref().unwrap_or(&self.label)
    }
}

#[derive(Clone, Debug)]
pub struct Completion {
    pub items: Vec<Candidate>,
    /// Byte offset (in the full source) where the typed prefix starts; accepting a candidate
    /// replaces `replace_start..cursor + trail` with its text ([`Candidate::text`]).
    pub replace_start: usize,
    /// Bytes after the cursor that the label replaces too: the closing `"` of a quoted name
    /// being typed (`"Mi|"`), which the label brings itself, or the rest of a literal's value
    /// (`'pa|id'`; its closing quote stays).
    pub trail: usize,
}

/// What [`complete_with`] found: the completion, and the table whose checks would give values
/// if they were known.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub completion: Option<Completion>,
    pub wants_checks: Option<WantsChecks>,
}

/// The cursor is in a string literal compared with a column of `schema.table`, whose checks are
/// not known ([`complete_with`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WantsChecks {
    pub schema: String,
    pub table: String,
    /// Where the literal's value starts: a byte offset in the full source.
    pub literal: usize,
}

pub const MAX_ITEMS: usize = 10;

#[derive(Debug, PartialEq, Eq)]
struct TableRef {
    schema: Option<String>,
    name: String,
    alias: Option<String>,
}

/// The name a token stands for in dialect `d`: a quoted one as written (closed or not), a bare
/// one folded as the server folds it.
fn unquote(d: Dialect, tok: &Token, src: &str) -> String {
    let t = tok.text(src);
    if tok.kind == Tok::QuotedIdent { d.unquote_lenient(t) } else { d.fold(t) }
}

fn is_name(t: &Token) -> bool {
    matches!(t.kind, Tok::Ident | Tok::QuotedIdent)
}

fn kw(t: &Token, src: &str, words: &[&str]) -> bool {
    t.kind == Tok::Keyword && words.iter().any(|w| t.text(src).eq_ignore_ascii_case(w))
}

/// Keywords a table name follows (`STRAIGHT_JOIN` is MySQL's; PostgreSQL does not read it as a
/// keyword).
const TABLE_INTRO: &[&str] = &["FROM", "JOIN", "UPDATE", "INTO", "TABLE", "STRAIGHT_JOIN"];
/// Keywords that may appear inside a FROM list without ending it.
const FROM_LIST_OK: &[&str] =
    &["AS", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "FULL", "CROSS", "NATURAL", "LATERAL", "ONLY", "STRAIGHT_JOIN"];

/// FROM/JOIN/UPDATE/INTO references with aliases (`x AS a`, `x a`) in the whole segment.
fn table_refs(d: Dialect, toks: &[Token], src: &str) -> Vec<TableRef> {
    let sig: Vec<&Token> = toks.iter().filter(|t| !t.is_trivia()).collect();
    let mut refs = Vec::new();
    let mut in_from = false;
    let mut i = 0;
    while i < sig.len() {
        let t = sig[i];
        let starts_ref = if kw(t, src, &["FROM", "JOIN", "STRAIGHT_JOIN", "UPDATE", "INTO"]) {
            in_from = kw(t, src, &["FROM", "JOIN", "STRAIGHT_JOIN"]);
            true
        } else if t.kind == Tok::Comma && in_from {
            true
        } else {
            if t.kind == Tok::Keyword && !kw(t, src, FROM_LIST_OK) {
                in_from = false;
            }
            false
        };
        i += 1;
        if !starts_ref {
            continue;
        }
        if i < sig.len() && kw(sig[i], src, &["ONLY", "LATERAL"]) {
            i += 1;
        }
        if i >= sig.len() || !is_name(sig[i]) {
            continue;
        }
        let first = unquote(d, sig[i], src);
        i += 1;
        let (schema, name) = if i + 1 < sig.len() && sig[i].kind == Tok::Dot && is_name(sig[i + 1]) {
            let n = unquote(d, sig[i + 1], src);
            i += 2;
            (Some(first), n)
        } else {
            (None, first)
        };
        if i < sig.len() && kw(sig[i], src, &["AS"]) {
            i += 1;
        }
        let alias = if i < sig.len() && is_name(sig[i]) {
            let a = unquote(d, sig[i], src);
            i += 1;
            Some(a)
        } else {
            None
        };
        refs.push(TableRef { schema, name, alias });
    }
    refs
}

/// The relation `schema.name`, or an unqualified `name` as the search path finds it: the
/// first schema of `path` that has it, else (a name the path does not reach) any schema's.
/// Names match whatever their ASCII case; in MySQL, which keeps a name's case (whether `Users`
/// and `users` are one table is the server's `lower_case_table_names`), one written as it is
/// in the catalog comes first among those the path reaches, and then among the rest.
fn find_rel<'a>(
    d: Dialect,
    cat: &'a Catalog,
    schema: Option<&str>,
    name: &str,
    path: &[String],
) -> Option<&'a Relation> {
    // `exact`: names in the same case; `on_path`: only the path's schemas.
    let found = |exact: bool, on_path: bool| {
        let same = |a: &str, b: &str| if exact { a == b } else { a.eq_ignore_ascii_case(b) };
        let matches = |r: &&Relation| same(&r.name, name);
        match schema {
            Some(s) => cat.relations.iter().filter(matches).find(|r| same(&r.schema, s)),
            None => cat
                .relations
                .iter()
                .filter(matches)
                .filter(|r| !on_path || path.iter().any(|p| same(p, &r.schema)))
                .min_by_key(|r| path.iter().position(|p| same(p, &r.schema)).unwrap_or(path.len())),
        }
    };
    match d {
        Dialect::Postgres => found(false, false),
        Dialect::MySql(_) => found(true, true)
            .or_else(|| found(false, true))
            .or_else(|| found(true, false))
            .or_else(|| found(false, false)),
    }
}

/// The search path a PostgreSQL session has without a schema of its own
/// ([`Dialect::default_path`]).
pub fn default_path() -> Vec<String> {
    Dialect::Postgres.default_path(None)
}

/// The search path of a PostgreSQL session in `schema` ([`Dialect::default_path`]).
pub fn schema_path(schema: &str) -> Vec<String> {
    Dialect::Postgres.default_path(Some(schema))
}

fn starts_ci(s: &str, prefix: &str) -> bool {
    s.to_lowercase().starts_with(&prefix.to_lowercase())
}

/// Whether the letters of `typed` appear in `name` in order (case-insensitive).
fn subsequence_ci(name: &str, typed: &str) -> bool {
    let mut it = name.chars().flat_map(char::to_lowercase);
    typed.chars().flat_map(char::to_lowercase).all(|t| it.any(|c| c == t))
}

/// How well a name matches what was typed: [`PREFIX`] when the prefix starts it,
/// [`SUBSEQUENCE`] when its letters only appear in it in order.
fn rank(name: &str, typed: &str) -> Option<u8> {
    if starts_ci(name, typed) {
        Some(PREFIX)
    } else if subsequence_ci(name, typed) {
        Some(SUBSEQUENCE)
    } else {
        None
    }
}

const PREFIX: u8 = 0;
const SUBSEQUENCE: u8 = 1;

/// A candidate and how well it matched ([`rank`]).
type Ranked = (u8, Candidate);

/// The prefix being typed and how names are written for it.
struct Typed {
    text: String,
    /// After an opening quote: every name is written quoted.
    quoted: bool,
    /// The dialect names are written in.
    dialect: Dialect,
}

impl Typed {
    fn ident(&self, name: &str) -> String {
        if self.quoted { self.dialect.force_quote_ident(name) } else { self.dialect.quote_ident(name) }
    }
}

fn columns_of(rel: &Relation, typed: &Typed) -> Vec<Ranked> {
    rel.columns
        .iter()
        .filter_map(|c| {
            let detail = (!c.type_name.is_empty()).then(|| c.type_name.clone());
            rank(&c.name, &typed.text)
                .map(|m| (m, Candidate { label: typed.ident(&c.name), kind: Kind::Column, detail, insert: None }))
        })
        .collect()
}

fn relation_candidate(r: &Relation, label: String) -> Candidate {
    Candidate { label, kind: if r.is_view { Kind::View } else { Kind::Table }, detail: None, insert: None }
}

/// Compute completion candidates at byte offset `cursor` of `src`, unqualified names resolved in
/// PostgreSQL's default search path ([`default_path`]).
/// `force` (manual trigger: Ctrl+N / F4) allows an empty prefix outside of a `qualifier.` context.
pub fn complete(src: &str, cursor: usize, cat: &Catalog, force: bool) -> Option<Completion> {
    complete_in(src, cursor, cat, force, &default_path())
}

/// [`complete`] for a session whose search path is `path` (a tab's schema): an
/// unqualified name resolves in the first schema of `path` that has it, and in a table position
/// the relations of `path`'s schemas are offered by their bare name, before every relation
/// qualified. The `WITH` queries of the statement come before the catalog's relations.
/// PostgreSQL text ([`complete_in_dialect`]).
pub fn complete_in(src: &str, cursor: usize, cat: &Catalog, force: bool, path: &[String]) -> Option<Completion> {
    complete_in_dialect(src, cursor, cat, force, path, Dialect::Postgres)
}

/// [`complete_in`] for text in dialect `d`: it is lexed as `d` reads it, names are read and
/// written as `d` does, and its keywords are offered.
pub fn complete_in_dialect(
    src: &str,
    cursor: usize,
    cat: &Catalog,
    force: bool,
    path: &[String],
    d: Dialect,
) -> Option<Completion> {
    complete_from(src, cursor, cat, force, path, d, LexState::default())
}

/// [`complete_in_dialect`] for text that starts where the lexer's state is `state` (lines of a
/// longer text: see [`lex_from`]). No table's checks are known ([`complete_with`]).
pub fn complete_from(
    src: &str,
    cursor: usize,
    cat: &Catalog,
    force: bool,
    path: &[String],
    d: Dialect,
    state: LexState,
) -> Option<Completion> {
    complete_with(src, cursor, cat, force, path, d, state, &|_, _| None).completion
}

/// [`complete_from`] where `checks(schema, table)` are the values the checks of a table allow
/// its columns, when they are known. In a string literal compared with a column the values of
/// the column are offered: its type's, then its checks' (once each), filtered as names are by
/// what the literal holds before the cursor (an empty one: all). Accepting one replaces the
/// literal's value (not its quotes; one being typed, open at the cursor, gets its closing
/// quote). When the column's table's checks are not known, the outcome says so
/// ([`Outcome::wants_checks`]).
#[allow(clippy::too_many_arguments)]
pub fn complete_with<'c>(
    src: &str,
    cursor: usize,
    cat: &Catalog,
    force: bool,
    path: &[String],
    d: Dialect,
    state: LexState,
    checks: &dyn Fn(&str, &str) -> Option<&'c [CheckValues]>,
) -> Outcome {
    let none = Outcome::default();
    // The segment around the cursor (between the statement ends around it), and the lexer's
    // state where it starts.
    let (mut seg_start, mut seg_end, mut seg_state) = (0, src.len(), state);
    let mut lexed = state;
    for t in lex_from(src, d, state) {
        lexed = lexed.after(&t, src, d);
        if t.ends_statement() {
            if cursor <= t.start {
                seg_end = t.start;
                break;
            }
            // Inside a terminator of more than one character, or a client command line.
            if cursor < t.end {
                return none;
            }
            (seg_start, seg_state) = (t.end, lexed);
        }
    }
    let seg = &src[seg_start..seg_end];
    let cur = cursor - seg_start;
    let toks = lex_from(seg, d, seg_state);

    // A quoted name being typed: its opening `"` is before the cursor, with no other `"`
    // between, and after the cursor comes the end, a space or the closing `"`. (A `"` typed
    // before other quoted names pairs with the next `"`: `SELECT "Mi| FROM "t"`.)
    let open_quote = toks.iter().find(|t| {
        t.kind == Tok::QuotedIdent
            && d.opening_quote(t.text(seg)).is_some_and(|quote| {
                t.start < cur
                    && cur <= t.end
                    && !(closed(d, t, seg) && cur == t.end)
                    && !seg[t.start + quote.len_utf8()..cur].contains(quote)
                    && seg[cur..t.end].chars().next().is_none_or(|c| c == quote || is_space(c))
            })
    });

    // In a string: the values of the column it is compared with.
    if let Some(lit) = toks.iter().find(|t| t.kind == Tok::Str && t.start < cur && (cur < t.end || !closed(d, t, seg)))
    {
        let ctx = Context { d, cat, path, seg, state: seg_state };
        return literal_values(&ctx, &toks, lit, cur, checks)
            .map(|mut o| {
                if let Some(c) = &mut o.completion {
                    c.replace_start += seg_start;
                }
                if let Some(w) = &mut o.wants_checks {
                    w.literal += seg_start;
                }
                o
            })
            .unwrap_or_default();
    }

    // No completion inside strings, comments, other quoted identifiers or dollar bodies.
    for t in &toks {
        let inside = match t.kind {
            Tok::LineComment | Tok::Variable => t.start < cur && cur <= t.end,
            Tok::BlockComment | Tok::Dollar => t.start < cur && (cur < t.end || !closed(d, t, seg)),
            Tok::Str => t.start < cur && (cur < t.end || !closed(d, t, seg)),
            Tok::QuotedIdent => open_quote != Some(t) && t.start < cur && (cur < t.end || !closed(d, t, seg)),
            _ => false,
        };
        if inside {
            return none;
        }
    }

    // Prefix being typed.
    let (prefix_start, typed, trail) = match open_quote {
        Some(t) => {
            let quote = d.opening_quote(t.text(seg)).unwrap_or(d.ident_quote());
            let ends_here = seg[cur..].starts_with(quote) && closed(d, t, seg) && cur + quote.len_utf8() == t.end;
            let trail = if ends_here { quote.len_utf8() } else { 0 };
            let text = d.unescape_ident(quote, &seg[t.start + quote.len_utf8()..cur]);
            (t.start, Typed { text, quoted: true, dialect: d }, trail)
        }
        None => match toks.iter().find(|t| t.is_word() && t.start < cur && cur <= t.end) {
            Some(t) => (t.start, Typed { text: seg[t.start..cur].to_string(), quoted: false, dialect: d }, 0),
            None => (cur, Typed { text: String::new(), quoted: false, dialect: d }, 0),
        },
    };
    let prefix = typed.text.as_str();
    let before: Vec<&Token> = toks.iter().filter(|t| t.end <= prefix_start && !t.is_trivia()).collect();

    // `qualifier.` or `schema.table.` right before the prefix.
    let qualifier = match before.as_slice() {
        [.., s, d1, q, d2]
            if d2.kind == Tok::Dot && d2.end == prefix_start && is_name(q) && d1.kind == Tok::Dot && is_name(s) =>
        {
            Some((Some(unquote(d, s, seg)), unquote(d, q, seg)))
        }
        [.., q, dot] if dot.kind == Tok::Dot && dot.end == prefix_start && (is_name(q) || q.kind == Tok::Keyword) => {
            Some((None, unquote(d, q, seg)))
        }
        _ => None,
    };

    if prefix.is_empty() && qualifier.is_none() && !force && !typed.quoted {
        return none;
    }

    // The statement's tables and `WITH` queries. An open quote runs to the end of the text, so
    // they are read with the name being typed left out.
    let rest;
    let (ctx_src, ctx_toks) = match open_quote {
        Some(t) => {
            rest = format!("{}{}", &seg[..t.start], &seg[cur + trail..]);
            (rest.as_str(), lex_from(&rest, d, seg_state))
        }
        None => (seg, toks.clone()),
    };
    let refs = table_refs(d, &ctx_toks, ctx_src);
    let ctes = with_queries(d, &ctx_toks, ctx_src);
    let resolve = |schema: Option<&str>, name: &str| resolve_in(d, cat, &ctes, path, schema, name);
    let mut items: Vec<Ranked> = Vec::new();

    if let Some((qs, q)) = qualifier {
        if let Some(s) = qs {
            if let Some(rel) = find_rel(d, cat, Some(&s), &q, path) {
                items.extend(columns_of(rel, &typed));
            }
        } else if let Some(rel) = referenced(&refs, &q, &resolve) {
            items.extend(columns_of(rel, &typed));
        } else if cat.schemas.iter().any(|s| s.eq_ignore_ascii_case(&q)) {
            let mut rels: Vec<(u8, &Relation)> = cat
                .relations
                .iter()
                .filter(|r| r.schema.eq_ignore_ascii_case(&q))
                .filter_map(|r| rank(&r.name, prefix).map(|m| (m, r)))
                .collect();
            rels.sort_by(|a, b| a.1.name.cmp(&b.1.name));
            items.extend(rels.into_iter().map(|(m, r)| (m, relation_candidate(r, typed.ident(&r.name)))));
        } else if let Some(rel) = resolve(None, &q) {
            items.extend(columns_of(rel, &typed));
        }
    } else if in_table_position(&before, seg) {
        // The statement's `WITH` queries hide relations of the same name.
        items.extend(ctes.iter().filter_map(|c| {
            rank(&c.name, prefix)
                .map(|m| (m, Candidate { label: typed.ident(&c.name), kind: Kind::Table, detail: None, insert: None }))
        }));
        items.extend(cat.schemas.iter().filter_map(|s| {
            rank(s, prefix)
                .map(|m| (m, Candidate { label: typed.ident(s), kind: Kind::Schema, detail: None, insert: None }))
        }));
        // The relations the search path reaches, by their bare name (the first schema of the
        // path that has a name wins, as the server resolves it) ...
        let on_path = |r: &Relation| path.iter().any(|p| p.eq_ignore_ascii_case(&r.schema));
        let mut bare: Vec<(u8, &Relation)> = cat
            .relations
            .iter()
            .filter(|r| on_path(r) && !ctes.iter().any(|c| c.name.eq_ignore_ascii_case(&r.name)))
            .filter_map(|r| rank(&r.name, prefix).map(|m| (m, r)))
            .filter(|(_, r)| find_rel(d, cat, None, &r.name, path).is_some_and(|f| std::ptr::eq(f, *r)))
            .collect();
        bare.sort_by(|a, b| a.1.name.cmp(&b.1.name));
        items.extend(bare.into_iter().map(|(m, r)| (m, relation_candidate(r, typed.ident(&r.name)))));
        // ... then every relation qualified.
        let mut rels: Vec<(u8, String, &Relation)> = cat
            .relations
            .iter()
            .filter_map(|r| {
                let full = format!("{}.{}", r.schema, r.name);
                let m = rank(&full, prefix).into_iter().chain(rank(&r.name, prefix)).min()?;
                Some((m, full, r))
            })
            .collect();
        rels.sort_by(|a, b| a.1.cmp(&b.1));
        items.extend(rels.into_iter().map(|(m, _, r)| {
            (m, relation_candidate(r, format!("{}.{}", typed.ident(&r.schema), typed.ident(&r.name))))
        }));
    } else {
        let mut seen = std::collections::HashSet::new();
        for r in &refs {
            if let Some(rel) = resolve(r.schema.as_deref(), &r.name) {
                for c in columns_of(rel, &typed) {
                    if seen.insert(c.1.label.to_lowercase()) {
                        items.push(c);
                    }
                }
            }
        }
        // Keywords match by their prefix only, and are never quoted.
        if (!prefix.is_empty() || force) && !typed.quoted {
            let lower = !prefix.is_empty() && prefix.chars().all(|c| !c.is_uppercase());
            items.extend(d.keywords().iter().filter(|k| starts_ci(k, prefix)).map(|k| {
                let label = if lower { k.to_lowercase() } else { k.to_string() };
                (PREFIX, Candidate { label, kind: Kind::Keyword, detail: None, insert: None })
            }));
        }
    }

    // Prefix matches first, each group in its order.
    items.sort_by_key(|(m, _)| *m);
    let mut items: Vec<Candidate> = items.into_iter().map(|(_, c)| c).collect();
    items.truncate(MAX_ITEMS);
    if items.is_empty() {
        return none;
    }
    Outcome {
        completion: Some(Completion { items, replace_start: seg_start + prefix_start, trail }),
        wants_checks: None,
    }
}

/// The relation `schema.name` (`schema` `None`: unqualified) of a statement whose `WITH`
/// queries are `ctes`: an unqualified name is one of those first, else a relation of `cat` as
/// the search path `path` finds it.
fn resolve_in<'a>(
    d: Dialect,
    cat: &'a Catalog,
    ctes: &'a [Relation],
    path: &[String],
    schema: Option<&str>,
    name: &str,
) -> Option<&'a Relation> {
    match schema {
        None => ctes.iter().find(|c| c.name.eq_ignore_ascii_case(name)).or_else(|| find_rel(d, cat, None, name, path)),
        Some(_) => find_rel(d, cat, schema, name, path),
    }
}

/// The relation qualifier `q` stands for in a statement that reads `refs`: the one it is an
/// alias of, else one read by that name without an alias.
fn referenced<'a>(
    refs: &[TableRef],
    q: &str,
    resolve: &dyn Fn(Option<&str>, &str) -> Option<&'a Relation>,
) -> Option<&'a Relation> {
    refs.iter()
        .filter(|r| r.alias.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(q)))
        .chain(refs.iter().filter(|r| r.alias.is_none() && r.name.eq_ignore_ascii_case(q)))
        .find_map(|r| resolve(r.schema.as_deref(), &r.name))
}

/// What completion reads names in: the dialect, the catalog, the search path, and the segment
/// around the cursor with the lexer's state where it starts.
struct Context<'a> {
    d: Dialect,
    cat: &'a Catalog,
    path: &'a [String],
    seg: &'a str,
    state: LexState,
}

/// The values offered in string literal `lit` of the segment (tokens `toks`), the cursor at
/// `cur` inside it ([`complete_with`]); offsets in the segment. `None`: it is not compared
/// with a column whose values are known.
fn literal_values<'c>(
    ctx: &Context,
    toks: &[Token],
    lit: &Token,
    cur: usize,
    checks: &dyn Fn(&str, &str) -> Option<&'c [CheckValues]>,
) -> Option<Outcome> {
    let (d, seg) = (ctx.d, ctx.seg);
    let open: LiteralOpen = d.literal_open(&seg[..lit.start], lit.text(seg))?;
    let quote = open.quote.len_utf8();
    let start = lit.start + open.prefix + quote;
    if cur < start {
        return None;
    }
    // A quote typed before other text pairs with a quote further on: one on another line, or
    // one a name or another quote follows right away. The literal is then open at the cursor.
    let is_closed = closed(d, lit, seg);
    let end = if is_closed { lit.end - quote } else { lit.end };
    let far = is_closed
        && (seg[cur..end].contains('\n')
            || seg[lit.end..].chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '\'' || c == '"'));
    let open_here = !is_closed || far;
    let mut before: Vec<&Token> = toks.iter().filter(|t| t.end <= lit.start && !t.is_trivia()).collect();
    // A MySQL charset introducer (`_utf8mb4'…'`) is the literal's.
    if matches!(d, Dialect::MySql(_))
        && before.last().is_some_and(|t| t.kind == Tok::Ident && t.end == lit.start && t.text(seg).starts_with('_'))
    {
        before.pop();
    }
    let target = compared_column(&before, seg)?;
    // The statement's tables, read with the literal left out (an open one runs to the end).
    let rest = format!("{} {}", &seg[..lit.start], &seg[if open_here { cur } else { lit.end }..]);
    let rest_toks = lex_from(&rest, d, ctx.state);
    let refs = table_refs(d, &rest_toks, &rest);
    let ctes = with_queries(d, &rest_toks, &rest);
    let resolve = |schema: Option<&str>, name: &str| resolve_in(d, ctx.cat, &ctes, ctx.path, schema, name);
    let names: Vec<String> = target.iter().map(|t| unquote(d, t, seg)).collect();
    let (rel, column) = match names.as_slice() {
        [c] => refs.iter().find_map(|r| {
            let rel = resolve(r.schema.as_deref(), &r.name)?;
            Some((rel, column_named(rel, c)?))
        })?,
        [q, c] => {
            let rel = referenced(&refs, q, &resolve).or_else(|| resolve(None, q))?;
            (rel, column_named(rel, c)?)
        }
        [s, t, c] => {
            let rel = find_rel(d, ctx.cat, Some(s), t, ctx.path)?;
            (rel, column_named(rel, c)?)
        }
        _ => return None,
    };
    // The column's type's values, then its checks' (a `WITH` query's columns have neither).
    let mut values: Vec<(&str, Option<&str>)> = Vec::new();
    for v in column.values.iter().flat_map(|v| v.iter()) {
        values.push((v.as_str(), Some(column.type_name.as_str())));
    }
    let mut wants_checks = None;
    if !rel.schema.is_empty() {
        match checks(&rel.schema, &rel.name) {
            Some(cs) => {
                for v in cs.iter().filter(|c| c.column == column.name).flat_map(|c| c.values.iter()) {
                    if !values.iter().any(|(x, _)| *x == v.as_str()) {
                        values.push((v.as_str(), Some("CHECK")));
                    }
                }
            }
            None => {
                wants_checks = Some(WantsChecks { schema: rel.schema.clone(), table: rel.name.clone(), literal: start })
            }
        }
    }
    let typed = d.unescape_literal_body(&seg[start..cur], open);
    let mut items: Vec<Ranked> = values
        .into_iter()
        .filter_map(|(v, detail)| {
            let mut insert = d.literal_body(v, open);
            if open_here {
                insert.push(open.quote);
            }
            let detail = detail.filter(|t| !t.is_empty()).map(str::to_string);
            rank(v, &typed)
                .map(|m| (m, Candidate { label: v.to_string(), kind: Kind::Value, detail, insert: Some(insert) }))
        })
        .collect();
    items.sort_by_key(|(m, _)| *m);
    let items: Vec<Candidate> = items.into_iter().map(|(_, c)| c).take(MAX_ITEMS).collect();
    let trail = if open_here { 0 } else { end - cur };
    let completion = (!items.is_empty()).then_some(Completion { items, replace_start: start, trail });
    Some(Outcome { completion, wants_checks })
}

/// Column `name` of `rel`: as written, else whatever its ASCII case.
fn column_named<'a>(rel: &'a Relation, name: &str) -> Option<&'a ColumnInfo> {
    rel.columns
        .iter()
        .find(|c| c.name == name)
        .or_else(|| rel.columns.iter().find(|c| c.name.eq_ignore_ascii_case(name)))
}

/// The column reference (`col`, `t.col`, `s.t.col`: its names' tokens) a literal right after
/// `before` (the significant tokens before it) is compared with: `col = ` (also `<>`, `!=`,
/// `<=>`, and `SET col = `), or `col IN (` / `col NOT IN (` with literals before it in the list.
fn compared_column<'t>(before: &[&'t Token], src: &str) -> Option<Vec<&'t Token>> {
    let mut i = before.len();
    let last = before.last()?;
    if last.kind == Tok::Op {
        // The operator: its one-character tokens written together.
        let mut from = i - 1;
        while from > 0 && before[from - 1].kind == Tok::Op && before[from - 1].end == before[from].start {
            from -= 1;
        }
        let op: String = before[from..i].iter().map(|t| t.text(src)).collect();
        if !matches!(op.as_str(), "=" | "<>" | "!=" | "<=>") {
            return None;
        }
        i = from;
    } else {
        // An `IN` list: literals (a MySQL charset introducer before a string), numbers, `NULL`.
        loop {
            let t = before.get(i.checked_sub(1)?)?;
            match t.kind {
                Tok::LParen => break,
                Tok::Comma => {}
                _ => return None,
            }
            i -= 1;
            let item = before.get(i.checked_sub(1)?)?;
            match item.kind {
                Tok::Str | Tok::Number => i -= 1,
                Tok::Keyword if item.text(src).eq_ignore_ascii_case("NULL") => i -= 1,
                _ => return None,
            }
            if item.kind == Tok::Str
                && let Some(cs) = i.checked_sub(1).map(|k| before[k])
                && cs.kind == Tok::Ident
                && cs.end == item.start
                && cs.text(src).starts_with('_')
            {
                i -= 1;
            }
        }
        i -= 1;
        let kw_at =
            |k: usize, w: &str| before.get(k).is_some_and(|t| t.is_word() && t.text(src).eq_ignore_ascii_case(w));
        if !kw_at(i.checked_sub(1)?, "IN") {
            return None;
        }
        i -= 1;
        if i > 0 && kw_at(i - 1, "NOT") {
            i -= 1;
        }
    }
    // The reference, in parentheses or not.
    let mut end = i;
    let paren = end > 0 && before[end - 1].kind == Tok::RParen;
    if paren {
        end -= 1;
    }
    let mut start = end;
    while start > 0 {
        let t = before[start - 1];
        let name = matches!(t.kind, Tok::Ident | Tok::QuotedIdent | Tok::Keyword);
        let expect_name = (end - start).is_multiple_of(2);
        if (expect_name && name) || (!expect_name && t.kind == Tok::Dot) {
            start -= 1;
        } else {
            break;
        }
    }
    // It starts with a name, and holds three at most.
    if start == end || before[start].kind == Tok::Dot || end - start > 5 {
        return None;
    }
    if paren && !(start > 0 && before[start - 1].kind == Tok::LParen) {
        return None;
    }
    Some(before[start..end].iter().copied().filter(|t| t.kind != Tok::Dot).collect())
}

/// The `WITH` queries of a statement (`WITH a AS (…), b(x, y) AS (…)`, also nested ones) as
/// relations without a schema. Their columns are the written column list, else the names of
/// the query's select list that can be read without running it (`x`, `t.x`, `… AS x`, `… x`);
/// other items (`*`, an expression without a name) are left out.
fn with_queries(d: Dialect, toks: &[Token], src: &str) -> Vec<Relation> {
    let sig: Vec<&Token> = toks.iter().filter(|t| !t.is_trivia()).collect();
    let mut out = Vec::new();
    for (i, t) in sig.iter().enumerate() {
        if !kw(t, src, &["WITH"]) {
            continue;
        }
        let mut j = i + 1;
        if sig.get(j).is_some_and(|t| t.is_word() && t.text(src).eq_ignore_ascii_case("recursive"))
            && sig.get(j + 1).is_some_and(|t| is_name(t))
        {
            j += 1;
        }
        while let Some(name) = sig.get(j).filter(|t| is_name(t)) {
            let name = unquote(d, name, src);
            j += 1;
            let mut listed = None;
            if sig.get(j).is_some_and(|t| t.kind == Tok::LParen) {
                let close = matching_paren(&sig, j);
                listed = Some(sig[j + 1..close].iter().filter(|t| is_name(t)).map(|t| unquote(d, t, src)).collect());
                j = close + 1;
            }
            if !sig.get(j).is_some_and(|t| kw(t, src, &["AS"])) {
                break;
            }
            j += 1;
            if sig.get(j).is_some_and(|t| kw(t, src, &["NOT"])) {
                j += 1;
            }
            if sig.get(j).is_some_and(|t| kw(t, src, &["MATERIALIZED"])) {
                j += 1;
            }
            if !sig.get(j).is_some_and(|t| t.kind == Tok::LParen) {
                break;
            }
            let close = matching_paren(&sig, j);
            let names: Vec<String> = listed.unwrap_or_else(|| select_list_names(d, &sig[j + 1..close], src));
            let columns = names.into_iter().map(|name| ColumnInfo::new(name, "")).collect();
            out.push(Relation { schema: String::new(), name, is_view: false, columns });
            j = close + 1;
            if !sig.get(j).is_some_and(|t| t.kind == Tok::Comma) {
                break;
            }
            j += 1;
        }
    }
    out
}

/// The index of the `)` closing the `(` at `open`, or the end when it is not closed.
fn matching_paren(sig: &[&Token], open: usize) -> usize {
    let mut depth = 0;
    for (i, t) in sig.iter().enumerate().skip(open) {
        match t.kind {
            Tok::LParen => depth += 1,
            Tok::RParen => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    sig.len()
}

/// The column names of a query's select list that are written in it (see [`with_queries`]).
fn select_list_names(d: Dialect, body: &[&Token], src: &str) -> Vec<String> {
    const END: &[&str] = &[
        "FROM",
        "WHERE",
        "GROUP",
        "HAVING",
        "ORDER",
        "LIMIT",
        "OFFSET",
        "UNION",
        "INTERSECT",
        "EXCEPT",
        "WINDOW",
        "FETCH",
        "FOR",
        "INTO",
    ];
    let Some(start) = body.iter().position(|t| kw(t, src, &["SELECT"])) else { return Vec::new() };
    let mut items: Vec<Vec<&Token>> = vec![Vec::new()];
    let mut depth = 0;
    for t in &body[start + 1..] {
        match t.kind {
            Tok::LParen => depth += 1,
            Tok::RParen => depth -= 1,
            Tok::Comma if depth == 0 => {
                items.push(Vec::new());
                continue;
            }
            _ if depth == 0 && kw(t, src, END) => break,
            _ => {}
        }
        items.last_mut().expect("one item at least").push(t);
    }
    items
        .iter()
        .filter_map(|item| {
            // `DISTINCT` / `ALL` before the first item.
            let item = match item.first() {
                Some(t) if kw(t, src, &["DISTINCT", "ALL"]) => &item[1..],
                _ => &item[..],
            };
            let (last, before) = item.split_last()?;
            if !is_name(last) {
                return None;
            }
            // A column reference (`x`, `t.x`), `… AS x`, or `x` right after a name, a
            // literal or `)` that ends what comes before it (`t.y x`, `count(*) n`, `1 one`).
            let chain = |s: &[&Token]| {
                s.iter().enumerate().all(|(i, t)| if i % 2 == 0 { is_name(t) } else { t.kind == Tok::Dot })
            };
            let named = match before.last() {
                None => true,
                Some(p) if kw(p, src, &["AS"]) => true,
                Some(p) => {
                    chain(item)
                        || (chain(before) && before.len() % 2 == 1)
                        || matches!(p.kind, Tok::RParen | Tok::Number | Tok::Str)
                }
            };
            named.then(|| unquote(d, last, src))
        })
        .collect()
}

fn closed(d: Dialect, t: &Token, src: &str) -> bool {
    let s = t.text(src);
    // A string's last quote may be one half of a doubled quote or escaped by a backslash
    // (MySQL's, PostgreSQL's `E'…'`), and in MySQL a string's quote may be `"`: closed when a
    // blank after it is not taken in.
    if t.kind == Tok::Str || (matches!(d, Dialect::MySql(_)) && matches!(t.kind, Tok::QuotedIdent | Tok::BlockComment))
    {
        return lex_from(&format!("{s} "), d, LexState::default()).first().is_some_and(|u| u.end == s.len());
    }
    match t.kind {
        Tok::QuotedIdent => s.len() >= 2 && d.opening_quote(s).is_some_and(|q| s.ends_with(q)),
        Tok::BlockComment => s.len() >= 4 && s.ends_with("*/"),
        Tok::Dollar => {
            let tag_len = s[1..].find('$').map(|i| i + 2).unwrap_or(s.len());
            s.len() >= tag_len * 2 && s.ends_with(&s[..tag_len])
        }
        _ => true,
    }
}

/// Is the prefix in a "table name expected" position (after FROM/JOIN/UPDATE/INTO/TABLE,
/// or after a comma inside a FROM list)?
fn in_table_position(before: &[&Token], src: &str) -> bool {
    let Some(last) = before.last() else { return false };
    if kw(last, src, TABLE_INTRO) || kw(last, src, &["ONLY"]) {
        return true;
    }
    if last.kind == Tok::Comma {
        for t in before.iter().rev() {
            if t.kind == Tok::Keyword && !kw(t, src, FROM_LIST_OK) {
                return kw(t, src, &["FROM"]);
            }
        }
    }
    false
}

#[cfg(test)]
mod tests;
