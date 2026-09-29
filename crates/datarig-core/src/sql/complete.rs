//! Context-aware completion on top of the tolerant lexer. Works on incomplete SQL
//! because it only looks at the token stream of the `;`-segment around the cursor.

use super::lexer::{KEYWORDS, Tok, Token, lex};
use super::split::segment_at;
use crate::i18n::Label;

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Column,
    Schema,
    Table,
    View,
    Keyword,
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
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub label: String,
    pub kind: Kind,
    pub detail: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Completion {
    pub items: Vec<Candidate>,
    /// Byte offset (in the full source) where the typed prefix starts; accepting a candidate
    /// replaces `replace_start..cursor` with the label.
    pub replace_start: usize,
}

pub const MAX_ITEMS: usize = 10;

#[derive(Debug, PartialEq, Eq)]
struct TableRef {
    schema: Option<String>,
    name: String,
    alias: Option<String>,
}

fn unquote(tok: &Token, src: &str) -> String {
    let t = tok.text(src);
    if tok.kind == Tok::QuotedIdent { t.trim_matches('"').replace("\"\"", "\"") } else { t.to_string() }
}

fn is_name(t: &Token) -> bool {
    matches!(t.kind, Tok::Ident | Tok::QuotedIdent)
}

fn kw(t: &Token, src: &str, words: &[&str]) -> bool {
    t.kind == Tok::Keyword && words.iter().any(|w| t.text(src).eq_ignore_ascii_case(w))
}

const TABLE_INTRO: &[&str] = &["FROM", "JOIN", "UPDATE", "INTO", "TABLE"];
/// Keywords that may appear inside a FROM list without ending it.
const FROM_LIST_OK: &[&str] =
    &["AS", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "FULL", "CROSS", "NATURAL", "LATERAL", "ONLY"];

/// FROM/JOIN/UPDATE/INTO references with aliases (`x AS a`, `x a`) in the whole segment.
fn table_refs(toks: &[Token], src: &str) -> Vec<TableRef> {
    let sig: Vec<&Token> = toks.iter().filter(|t| !t.is_trivia()).collect();
    let mut refs = Vec::new();
    let mut in_from = false;
    let mut i = 0;
    while i < sig.len() {
        let t = sig[i];
        let starts_ref = if kw(t, src, &["FROM", "JOIN", "UPDATE", "INTO"]) {
            in_from = kw(t, src, &["FROM", "JOIN"]);
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
        let first = unquote(sig[i], src);
        i += 1;
        let (schema, name) = if i + 1 < sig.len() && sig[i].kind == Tok::Dot && is_name(sig[i + 1]) {
            let n = unquote(sig[i + 1], src);
            i += 2;
            (Some(first), n)
        } else {
            (None, first)
        };
        if i < sig.len() && kw(sig[i], src, &["AS"]) {
            i += 1;
        }
        let alias = if i < sig.len() && is_name(sig[i]) {
            let a = unquote(sig[i], src);
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
fn find_rel<'a>(cat: &'a Catalog, schema: Option<&str>, name: &str, path: &[String]) -> Option<&'a Relation> {
    let matches = |r: &&Relation| r.name.eq_ignore_ascii_case(name);
    match schema {
        Some(s) => cat.relations.iter().filter(matches).find(|r| r.schema.eq_ignore_ascii_case(s)),
        None => cat
            .relations
            .iter()
            .filter(matches)
            .min_by_key(|r| path.iter().position(|p| p.eq_ignore_ascii_case(&r.schema)).unwrap_or(path.len())),
    }
}

/// The search path a session has without a schema of its own (PostgreSQL's default without
/// the `$user` schema).
pub fn default_path() -> Vec<String> {
    vec!["public".to_string()]
}

/// The search path of a session in `schema`: that schema,
/// then `public` (an extension's objects there resolve unqualified), or `public` alone when that
/// is the schema. `pg_catalog` is implicitly first on the server and is not listed.
pub fn schema_path(schema: &str) -> Vec<String> {
    let mut path = vec![schema.to_string()];
    if schema != "public" {
        path.push("public".to_string());
    }
    path
}

fn starts_ci(s: &str, prefix: &str) -> bool {
    s.to_lowercase().starts_with(&prefix.to_lowercase())
}

fn columns_of(rel: &Relation, prefix: &str) -> Vec<Candidate> {
    rel.columns
        .iter()
        .filter(|c| starts_ci(&c.name, prefix))
        .map(|c| Candidate { label: c.name.clone(), kind: Kind::Column, detail: Some(c.type_name.clone()) })
        .collect()
}

fn relation_candidate(r: &Relation, label: String) -> Candidate {
    Candidate { label, kind: if r.is_view { Kind::View } else { Kind::Table }, detail: None }
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
/// qualified.
pub fn complete_in(src: &str, cursor: usize, cat: &Catalog, force: bool, path: &[String]) -> Option<Completion> {
    let (seg_start, seg_end) = segment_at(src, cursor);
    let seg = &src[seg_start..seg_end];
    let cur = cursor - seg_start;
    let toks = lex(seg);

    // No completion inside strings, comments, quoted identifiers or dollar bodies.
    for t in &toks {
        let inside = match t.kind {
            Tok::LineComment => t.start < cur && cur <= t.end,
            Tok::BlockComment | Tok::Dollar => t.start < cur && (cur < t.end || !closed(t, seg)),
            Tok::Str | Tok::QuotedIdent => t.start < cur && (cur < t.end || !closed(t, seg)),
            _ => false,
        };
        if inside {
            return None;
        }
    }

    // Prefix being typed.
    let (prefix_start, prefix) = match toks.iter().find(|t| t.is_word() && t.start < cur && cur <= t.end) {
        Some(t) => (t.start, &seg[t.start..cur]),
        None => (cur, ""),
    };
    let before: Vec<&Token> = toks.iter().filter(|t| t.end <= prefix_start && !t.is_trivia()).collect();

    // `qualifier.` or `schema.table.` right before the prefix.
    let qualifier = match before.as_slice() {
        [.., s, d1, q, d2]
            if d2.kind == Tok::Dot && d2.end == prefix_start && is_name(q) && d1.kind == Tok::Dot && is_name(s) =>
        {
            Some((Some(unquote(s, seg)), unquote(q, seg)))
        }
        [.., q, d] if d.kind == Tok::Dot && d.end == prefix_start && (is_name(q) || q.kind == Tok::Keyword) => {
            Some((None, unquote(q, seg)))
        }
        _ => None,
    };

    if prefix.is_empty() && qualifier.is_none() && !force {
        return None;
    }

    let refs = table_refs(&toks, seg);
    let mut items: Vec<Candidate> = Vec::new();

    if let Some((qs, q)) = qualifier {
        if let Some(s) = qs {
            if let Some(rel) = find_rel(cat, Some(&s), &q, path) {
                items.extend(columns_of(rel, prefix));
            }
        } else if let Some(rel) = refs
            .iter()
            .filter(|r| r.alias.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(&q)))
            .chain(refs.iter().filter(|r| r.alias.is_none() && r.name.eq_ignore_ascii_case(&q)))
            .find_map(|r| find_rel(cat, r.schema.as_deref(), &r.name, path))
        {
            items.extend(columns_of(rel, prefix));
        } else if cat.schemas.iter().any(|s| s.eq_ignore_ascii_case(&q)) {
            let mut rels: Vec<&Relation> = cat
                .relations
                .iter()
                .filter(|r| r.schema.eq_ignore_ascii_case(&q) && starts_ci(&r.name, prefix))
                .collect();
            rels.sort_by(|a, b| a.name.cmp(&b.name));
            items.extend(rels.into_iter().map(|r| relation_candidate(r, r.name.clone())));
        } else if let Some(rel) = find_rel(cat, None, &q, path) {
            items.extend(columns_of(rel, prefix));
        }
    } else if in_table_position(&before, seg) {
        items.extend(cat.schemas.iter().filter(|s| starts_ci(s, prefix)).map(|s| Candidate {
            label: s.clone(),
            kind: Kind::Schema,
            detail: None,
        }));
        // The relations the search path reaches, by their bare name (the first schema of the
        // path that has a name wins, as the server resolves it) ...
        let on_path = |r: &Relation| path.iter().any(|p| p.eq_ignore_ascii_case(&r.schema));
        let mut bare: Vec<&Relation> = cat
            .relations
            .iter()
            .filter(|r| on_path(r) && starts_ci(&r.name, prefix))
            .filter(|r| find_rel(cat, None, &r.name, path).is_some_and(|f| std::ptr::eq(f, *r)))
            .collect();
        bare.sort_by(|a, b| a.name.cmp(&b.name));
        items.extend(bare.into_iter().map(|r| relation_candidate(r, r.name.clone())));
        // ... then every relation qualified.
        let mut rels: Vec<(String, &Relation)> = cat
            .relations
            .iter()
            .map(|r| (format!("{}.{}", r.schema, r.name), r))
            .filter(|(full, r)| starts_ci(full, prefix) || starts_ci(&r.name, prefix))
            .collect();
        rels.sort_by(|a, b| a.0.cmp(&b.0));
        items.extend(rels.into_iter().map(|(full, r)| relation_candidate(r, full)));
    } else {
        let mut seen = std::collections::HashSet::new();
        for r in &refs {
            if let Some(rel) = find_rel(cat, r.schema.as_deref(), &r.name, path) {
                for c in columns_of(rel, prefix) {
                    if seen.insert(c.label.to_lowercase()) {
                        items.push(c);
                    }
                }
            }
        }
        if !prefix.is_empty() || force {
            let lower = !prefix.is_empty() && prefix.chars().all(|c| !c.is_uppercase());
            items.extend(KEYWORDS.iter().filter(|k| starts_ci(k, prefix)).map(|k| Candidate {
                label: if lower { k.to_lowercase() } else { k.to_string() },
                kind: Kind::Keyword,
                detail: None,
            }));
        }
    }

    items.truncate(MAX_ITEMS);
    if items.is_empty() {
        return None;
    }
    Some(Completion { items, replace_start: seg_start + prefix_start })
}

fn closed(t: &Token, src: &str) -> bool {
    let s = t.text(src);
    match t.kind {
        Tok::Str => s.len() >= 2 && s.ends_with('\''),
        Tok::QuotedIdent => s.len() >= 2 && s.ends_with('"'),
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
