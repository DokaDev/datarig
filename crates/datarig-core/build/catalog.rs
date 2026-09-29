//! Locale catalog checks and code generation for the typed message API.
//!
//! This module is pure (no file or cargo I/O) so that it can be shared by `build.rs`, which
//! turns problems into build errors, and by `tests/catalog.rs`, which feeds it fixtures.
//!
//! Rules (see `docs/architecture.md`, "UI strings (i18n)"):
//! - The source catalog (`en.toml`) defines the keys. A key is lowercase ASCII words joined by
//!   `.` or `_`; it becomes a Rust variant name (`conn.failed` -> `ConnFailed`).
//! - A placeholder is `{name}` with a lowercase identifier. Its Rust type follows from the
//!   name: `count` is a `u64` (thousands separators), `elapsed` and `latency` are a
//!   `Duration` (`123ms` / `1.2s`), anything else is a `String`. Other braces are literal text.
//! - Every other locale must have exactly the source keys with the same placeholder set per
//!   key. A [`Policy::Optional`] locale may miss keys (English is used, with a warning); extra
//!   keys and placeholder mismatches are always errors.
//! - Plurals: a value may be a table `{ one = "…", other = "…" }` instead of a string. The
//!   form is picked by the `{count}` argument (`one` when it is 1). In the source catalog every
//!   entry with `{count}` must be plural, a plural entry must have `{count}`, both forms have
//!   the same placeholders (the `one` form may leave `{count}` out), and `(s)`-style pseudo
//!   plurals are rejected. A translation may use
//!   a plain string for a plural entry (a language without plural forms, like Korean) or both
//!   forms; it may not give forms to an entry that is not plural in the source.

#![allow(dead_code)] // `build.rs` and `tests/catalog.rs` each use a different part.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};

/// How strictly a locale must match the source catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// The source of truth for keys and placeholders (English).
    Source,
    /// Every key must be translated; a missing key fails the build.
    Required,
    /// Missing keys fall back to the source text and only warn.
    Optional,
}

/// One locale file.
#[derive(Clone, Debug)]
pub struct Locale {
    /// Short code, also the stem of the file name (`ko` for `ko.toml`).
    pub code: String,
    /// Path shown in messages.
    pub file: String,
    pub policy: Policy,
    /// Key -> text; for a plural entry its `other` form.
    pub entries: BTreeMap<String, String>,
    /// Key -> the `one` form of each plural entry.
    pub one: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProblemKind {
    /// The file is not valid TOML.
    Syntax(String),
    /// A value that is neither a string nor a plural table.
    NotAString,
    /// A table that is not exactly `{ one = "…", other = "…" }` with string values.
    BadPlural,
    /// A source entry with `{count}` that is not plural.
    PluralMissing,
    /// A plural entry without `{count}`.
    PluralWithoutCount,
    /// The `one` and `other` forms have different placeholders.
    PluralForms { one: Vec<String>, other: Vec<String> },
    /// A translation gives plural forms to an entry that is not plural in the source catalog.
    PluralNotInSource,
    /// Source text with a `(s)`-style pseudo plural.
    PseudoPlural,
    /// The key does not follow the key syntax.
    BadKey,
    /// Two keys map to the same Rust name.
    NameClash { other: String },
    /// A placeholder name that is not a lowercase identifier or is a Rust keyword.
    BadPlaceholder(String),
    /// The source catalog has the key, this locale does not.
    Missing,
    /// This locale has a key the source catalog does not.
    Extra,
    /// The placeholder sets differ.
    Placeholders { expected: Vec<String>, found: Vec<String> },
}

/// A problem with one key of one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub file: String,
    pub key: String,
    pub kind: ProblemKind,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |v: &[String]| {
            if v.is_empty() {
                "none".to_string()
            } else {
                v.iter().map(|p| format!("{{{p}}}")).collect::<Vec<_>>().join(", ")
            }
        };
        match &self.kind {
            ProblemKind::Syntax(e) => write!(f, "{}: not valid TOML: {e}", self.file),
            ProblemKind::NotAString => write!(
                f,
                "{}: key `{}`: the value must be a string or a plural table {{ one = \"…\", other = \"…\" }}",
                self.file, self.key
            ),
            ProblemKind::BadPlural => write!(
                f,
                "{}: key `{}`: a plural table has exactly the string forms `one` and `other`",
                self.file, self.key
            ),
            ProblemKind::PluralMissing => write!(
                f,
                "{}: key `{}`: has {{count}}, so it must be plural: {{ one = \"…\", other = \"…\" }}",
                self.file, self.key
            ),
            ProblemKind::PluralWithoutCount => {
                write!(f, "{}: key `{}`: a plural entry needs the {{count}} placeholder", self.file, self.key)
            }
            ProblemKind::PluralForms { one, other } => write!(
                f,
                "{}: key `{}`: the plural forms have different placeholders (one: {}, other: {})",
                self.file,
                self.key,
                list(one),
                list(other)
            ),
            ProblemKind::PluralNotInSource => write!(
                f,
                "{}: key `{}`: plural forms for an entry that is not plural in the source catalog",
                self.file, self.key
            ),
            ProblemKind::PseudoPlural => write!(
                f,
                "{}: key `{}`: no \"(s)\" plurals; use {{count}} with `one` and `other` forms",
                self.file, self.key
            ),
            ProblemKind::BadKey => {
                write!(f, "{}: key `{}`: keys are lowercase words (a-z, 0-9) joined by `.` or `_`", self.file, self.key)
            }
            ProblemKind::NameClash { other } => {
                write!(f, "{}: key `{}`: same Rust name as `{other}`; rename one of them", self.file, self.key)
            }
            ProblemKind::BadPlaceholder(p) => write!(
                f,
                "{}: key `{}`: placeholder `{{{p}}}` must be a lowercase identifier that is not a Rust keyword",
                self.file, self.key
            ),
            ProblemKind::Missing => write!(
                f,
                "{}: key `{}`: missing (every key of the source catalog must be translated)",
                self.file, self.key
            ),
            ProblemKind::Extra => write!(
                f,
                "{}: key `{}`: not in the source catalog (remove it, or add it there first)",
                self.file, self.key
            ),
            ProblemKind::Placeholders { expected, found } => write!(
                f,
                "{}: key `{}`: placeholders differ from the source catalog (expected {}, found {})",
                self.file,
                self.key,
                list(expected),
                list(found)
            ),
        }
    }
}

/// Texts of a catalog file: key -> text (the `other` form of a plural entry), and key -> the
/// `one` form of each plural entry.
pub type Parsed = (BTreeMap<String, String>, BTreeMap<String, String>);

/// Parse a catalog file: a flat table of `"key" = "text"` or
/// `"key" = { one = "…", other = "…" }`.
pub fn parse(file: &str, src: &str) -> Result<Parsed, Vec<Problem>> {
    let table: toml::Table = src.parse().map_err(|e: toml::de::Error| {
        vec![Problem { file: file.to_string(), key: String::new(), kind: ProblemKind::Syntax(e.to_string()) }]
    })?;
    let mut out = BTreeMap::new();
    let mut one = BTreeMap::new();
    let mut problems = Vec::new();
    for (k, v) in table {
        match v {
            toml::Value::String(s) => {
                out.insert(k, s);
            }
            toml::Value::Table(t) => {
                let form = |name: &str| t.get(name).and_then(toml::Value::as_str).map(str::to_string);
                match (form("one"), form("other")) {
                    (Some(o), Some(other)) if t.len() == 2 => {
                        one.insert(k.clone(), o);
                        out.insert(k, other);
                    }
                    _ => problems.push(Problem { file: file.to_string(), key: k, kind: ProblemKind::BadPlural }),
                }
            }
            _ => problems.push(Problem { file: file.to_string(), key: k, kind: ProblemKind::NotAString }),
        }
    }
    if problems.is_empty() { Ok((out, one)) } else { Err(problems) }
}

/// Plural rules of one locale's entries (both forms carry the same placeholders, `{count}`
/// aside); `source`:
/// also require every `{count}` entry to be plural and reject pseudo plurals.
fn check_plurals(loc: &Locale, source: bool) -> Vec<Problem> {
    let mut problems = Vec::new();
    let problem = |key: &str, kind| Problem { file: loc.file.clone(), key: key.to_string(), kind };
    for (key, text) in &loc.entries {
        let other = placeholders(text);
        match loc.one.get(key) {
            Some(one) => {
                // The `one` form may leave the number out ("its open tab").
                let one = placeholders(one);
                let without = |v: &[String]| v.iter().filter(|p| *p != "count").cloned().collect::<Vec<_>>();
                if !other.iter().any(|p| p == "count") {
                    problems.push(problem(key, ProblemKind::PluralWithoutCount));
                } else if without(&one) != without(&other) {
                    problems.push(problem(key, ProblemKind::PluralForms { one, other }));
                }
            }
            None if source && other.iter().any(|p| p == "count") => {
                problems.push(problem(key, ProblemKind::PluralMissing));
            }
            None => {}
        }
        if source && (text.contains("(s)") || loc.one.get(key).is_some_and(|t| t.contains("(s)"))) {
            problems.push(problem(key, ProblemKind::PseudoPlural));
        }
    }
    problems
}

/// A piece of a message template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Placeholder(String),
}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_lowercase() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'
}

/// Split a template into text and `{placeholder}` pieces. A `{` that does not open a
/// `{identifier}` is literal text.
pub fn pieces(template: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        text.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        let name_len = tail.find(|c: char| !is_ident_char(c)).unwrap_or(tail.len());
        let name = &tail[..name_len];
        if !name.is_empty() && name.starts_with(is_ident_start) && tail[name_len..].starts_with('}') {
            if !text.is_empty() {
                out.push(Piece::Text(std::mem::take(&mut text)));
            }
            out.push(Piece::Placeholder(name.to_string()));
            rest = &tail[name_len + 1..];
        } else {
            text.push('{');
            rest = tail;
        }
    }
    text.push_str(rest);
    if !text.is_empty() {
        out.push(Piece::Text(text));
    }
    out
}

/// The placeholder names of a template, sorted and without duplicates.
pub fn placeholders(template: &str) -> Vec<String> {
    let set: BTreeSet<String> = pieces(template)
        .into_iter()
        .filter_map(|p| if let Piece::Placeholder(n) = p { Some(n) } else { None })
        .collect();
    set.into_iter().collect()
}

/// Rust type of a placeholder argument, chosen by its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgType {
    /// `String`, shown as is.
    Text,
    /// `u64`, shown with thousands separators.
    Count,
    /// `std::time::Duration`, shown as `123ms` / `1.2s`.
    Elapsed,
}

pub fn arg_type(name: &str) -> ArgType {
    match name {
        "count" => ArgType::Count,
        "elapsed" | "latency" => ArgType::Elapsed,
        _ => ArgType::Text,
    }
}

const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn",
    "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self",
    "static", "struct", "super", "trait", "true", "try", "type", "unsafe", "use", "where", "while", "yield", "_",
];

fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .split(['.', '_'])
            .all(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()))
        && key.starts_with(|c: char| c.is_ascii_lowercase())
}

/// Rust variant name of a key: `conn.disconnected_tx` -> `ConnDisconnectedTx`.
pub fn variant_name(key: &str) -> String {
    key.split(['.', '_'])
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_ascii_uppercase().to_string() + c.as_str()).unwrap_or_default()
        })
        .collect()
}

/// Check the source catalog on its own: key syntax, Rust name clashes, placeholder names.
pub fn check_source(src: &Locale) -> Vec<Problem> {
    let mut problems = Vec::new();
    let mut names: BTreeMap<String, &str> = BTreeMap::new();
    let problem = |key: &str, kind| Problem { file: src.file.clone(), key: key.to_string(), kind };
    for (key, text) in &src.entries {
        if !valid_key(key) {
            problems.push(problem(key, ProblemKind::BadKey));
            continue;
        }
        if let Some(other) = names.insert(variant_name(key), key) {
            problems.push(problem(key, ProblemKind::NameClash { other: other.to_string() }));
        }
        for p in placeholders(text) {
            if KEYWORDS.contains(&p.as_str()) {
                problems.push(problem(key, ProblemKind::BadPlaceholder(p)));
            }
        }
    }
    problems.extend(check_plurals(src, true));
    problems
}

/// Check a translation against the source catalog. Returns (errors, warnings); a key missing
/// from an [`Policy::Optional`] locale is a warning, every other problem is an error.
pub fn check_locale(src: &Locale, loc: &Locale) -> (Vec<Problem>, Vec<Problem>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let problem = |key: &str, kind| Problem { file: loc.file.clone(), key: key.to_string(), kind };
    for (key, text) in &src.entries {
        match loc.entries.get(key) {
            None if loc.policy == Policy::Optional => warnings.push(problem(key, ProblemKind::Missing)),
            None => errors.push(problem(key, ProblemKind::Missing)),
            Some(t) => {
                let (expected, found) = (placeholders(text), placeholders(t));
                if expected != found {
                    errors.push(problem(key, ProblemKind::Placeholders { expected, found }));
                }
            }
        }
    }
    for key in loc.entries.keys().filter(|k| !src.entries.contains_key(*k)) {
        errors.push(problem(key, ProblemKind::Extra));
    }
    for key in loc.one.keys().filter(|k| src.entries.contains_key(*k) && !src.one.contains_key(*k)) {
        errors.push(problem(key, ProblemKind::PluralNotInSource));
    }
    errors.extend(check_plurals(loc, false).into_iter().filter(|p| src.one.contains_key(&p.key)));
    (errors, warnings)
}

/// Check every locale. `locales[0]` is the source. Returns (errors, warnings).
pub fn check(locales: &[Locale]) -> (Vec<Problem>, Vec<Problem>) {
    let Some((src, rest)) = locales.split_first() else { return (Vec::new(), Vec::new()) };
    let mut errors = check_source(src);
    let mut warnings = Vec::new();
    for loc in rest {
        let (e, w) = check_locale(src, loc);
        errors.extend(e);
        warnings.extend(w);
    }
    (errors, warnings)
}

/// A Rust string literal for `s`.
fn lit(s: &str) -> String {
    format!("{s:?}")
}

/// A `format!` string literal for the pieces (literal braces doubled).
fn format_lit(pieces: &[Piece]) -> String {
    let mut s = String::new();
    for p in pieces {
        match p {
            Piece::Text(t) => s.push_str(&t.replace('{', "{{").replace('}', "}}")),
            Piece::Placeholder(n) => {
                s.push('{');
                s.push_str(n);
                s.push('}');
            }
        }
    }
    lit(&s)
}

/// An expression for a message text: `format!(..)`, or a plain `String` when it has no
/// placeholder (a plural `one` form may have none).
fn text_expr(template: &str) -> String {
    let ps = pieces(template);
    if ps.iter().any(|p| matches!(p, Piece::Placeholder(_))) {
        format!("format!({})", format_lit(&ps))
    } else {
        format!("{}.to_string()", lit(template))
    }
}

/// `Lang` variant of a locale code: `ko` -> `Ko`.
pub fn lang_variant(code: &str) -> String {
    variant_name(code)
}

/// Generate the typed API (`Label`, `Msg`) for checked locales. `locales[0]` is the source;
/// a key an optional locale lacks uses the source text.
pub fn generate(locales: &[Locale]) -> String {
    let src = &locales[0];
    let text_of = |loc: &Locale, key: &str| loc.entries.get(key).unwrap_or(&src.entries[key]).clone();
    // The `one` form of a plural entry in `loc` (a missing translation uses the source's).
    let one_of = |loc: &Locale, key: &str| match loc.entries.get(key) {
        Some(_) => loc.one.get(key).cloned(),
        None => src.one.get(key).cloned(),
    };
    let (labels, msgs): (Vec<_>, Vec<_>) = src.entries.iter().partition(|(_, t)| placeholders(t).is_empty());
    let doc = |key: &str, text: &str| format!("    /// `{key}`: {}\n", lit(text).replace('`', "'"));
    let mut o = String::new();
    o.push_str("// @generated by crates/datarig-core/build.rs from locales/*.toml. Do not edit.\n\n");

    // Label
    o.push_str(
        "/// A catalog entry without placeholders. Variants are generated from the source catalog\n\
         /// (`en.toml`); every locale is checked to translate each of them at build time.\n\
         #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]\npub enum Label {\n",
    );
    for (key, text) in &labels {
        o.push_str(&doc(key, text));
        let _ = writeln!(o, "    {},", variant_name(key));
    }
    o.push_str("}\n\nimpl Label {\n    /// Every label, in key order.\n    pub const ALL: &'static [Label] = &[\n");
    for (key, _) in &labels {
        let _ = writeln!(o, "        Label::{},", variant_name(key));
    }
    o.push_str(
        "    ];\n\n    /// The catalog key.\n    pub const fn key(self) -> &'static str {\n        match self {\n",
    );
    for (key, _) in &labels {
        let _ = writeln!(o, "            Label::{} => {},", variant_name(key), lit(key));
    }
    o.push_str("        }\n    }\n\n    /// The text in `lang`.\n    pub const fn text(self, lang: Lang) -> &'static str {\n        match lang {\n");
    for loc in locales {
        let _ = writeln!(o, "            Lang::{} => match self {{", lang_variant(&loc.code));
        for (key, _) in &labels {
            let _ = writeln!(o, "                Label::{} => {},", variant_name(key), lit(&text_of(loc, key)));
        }
        o.push_str("            },\n");
    }
    o.push_str("        }\n    }\n}\n\n");

    // Msg
    o.push_str(
        "/// Any catalog message: a [`Label`], or an entry with placeholders and its typed arguments.\n\
         /// A message keeps its arguments, so it can be rendered again after a language switch.\n\
         #[derive(Clone, Debug, PartialEq, Eq)]\npub enum Msg {\n    /// An entry without placeholders.\n    Label(Label),\n",
    );
    let fields = |text: &str| -> Vec<(String, ArgType)> {
        placeholders(text).into_iter().map(|p| (p.clone(), arg_type(&p))).collect()
    };
    let rust_type = |t: ArgType| match t {
        ArgType::Text => "String",
        ArgType::Count => "u64",
        ArgType::Elapsed => "std::time::Duration",
    };
    for (key, text) in &msgs {
        o.push_str(&doc(key, text));
        let fs: Vec<String> = fields(text).iter().map(|(n, t)| format!("{n}: {}", rust_type(*t))).collect();
        let _ = writeln!(o, "    {} {{ {} }},", variant_name(key), fs.join(", "));
    }
    o.push_str("}\n\nimpl From<Label> for Msg {\n    fn from(l: Label) -> Self {\n        Msg::Label(l)\n    }\n}\n\n");
    o.push_str("impl Msg {\n    /// The catalog key.\n    pub fn key(&self) -> &'static str {\n        match self {\n            Msg::Label(l) => l.key(),\n");
    for (key, _) in &msgs {
        let _ = writeln!(o, "            Msg::{} {{ .. }} => {},", variant_name(key), lit(key));
    }
    o.push_str("        }\n    }\n\n    /// The text in `lang`, with the arguments filled in.\n    pub fn render(&self, lang: Lang) -> String {\n        match self {\n            Msg::Label(l) => l.text(lang).to_string(),\n");
    for (key, text) in &msgs {
        let fs = fields(text);
        let names: Vec<&str> = fs.iter().map(|(n, _)| n.as_str()).collect();
        let _ = writeln!(o, "            Msg::{} {{ {} }} => {{", variant_name(key), names.join(", "));
        // The `one` form of `loc` when it differs from `other` (identical forms need no branch).
        let one_form = |loc: &Locale| one_of(loc, key).filter(|one| *one != text_of(loc, key));
        if locales.iter().any(|loc| one_form(loc).is_some()) {
            o.push_str("                let plural_one = *count == 1;\n");
        }
        for (n, t) in &fs {
            match t {
                ArgType::Text => {}
                ArgType::Count => {
                    let _ = writeln!(o, "                let {n} = fmt_count(*{n});");
                }
                ArgType::Elapsed => {
                    let _ = writeln!(o, "                let {n} = fmt_elapsed(*{n});");
                }
            }
        }
        o.push_str("                match lang {\n");
        for loc in locales {
            let other = text_expr(&text_of(loc, key));
            let _ = match one_form(loc) {
                Some(one) => writeln!(
                    o,
                    "                    Lang::{} => if plural_one {{ {} }} else {{ {other} }},",
                    lang_variant(&loc.code),
                    text_expr(&one)
                ),
                None => writeln!(o, "                    Lang::{} => {other},", lang_variant(&loc.code)),
            };
        }
        o.push_str("                }\n            }\n");
    }
    o.push_str("        }\n    }\n\n");
    // One sample of every message with arguments, for the runtime round-trip test.
    o.push_str("    /// One message per entry with placeholders, with distinctive arguments (tests).\n    #[cfg(test)]\n    pub(crate) fn samples() -> Vec<Msg> {\n        vec![\n");
    for (key, text) in &msgs {
        let fs: Vec<String> = fields(text)
            .iter()
            .map(|(n, t)| match t {
                ArgType::Text => format!("{n}: \"<{n}>\".to_string()"),
                ArgType::Count => format!("{n}: 1234567"),
                ArgType::Elapsed => format!("{n}: std::time::Duration::from_millis(1234)"),
            })
            .collect();
        let _ = writeln!(o, "            Msg::{} {{ {} }},", variant_name(key), fs.join(", "));
    }
    o.push_str("        ]\n    }\n}\n");
    o
}
