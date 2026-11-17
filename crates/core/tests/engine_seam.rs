//! The engine seam check: what an engine is decides in a few seam modules only, and no code
//! sends the engines it does not name to a default branch.
//!
//! The scan parses every `.rs` file under the `src/` of every workspace member (test code
//! skipped: `tests.rs`, `tests/`, `#[cfg(test)]` items and files under an inner
//! `#![cfg(test)]` may name engines) and reports, for the six engine enums (`EngineId`, `Dialect`, `EngineFacts`, `ExplainFormat`, `EditFormat`,
//! `MonitorFormat`):
//!
//! - a reference to one of their variants outside the seam modules (construction or pattern);
//! - a legacy marker of engine identity outside the seam modules (the `mariadb` flag,
//!   `is_mysql`, a `mysql: bool` or `pg: bool`, `Scheme::of_driver`, `Dbms::of_driver`, the
//!   text `"mariadb"`);
//! - a `match` with an arm naming a variant and a catch-all arm (`_` or a binding), anywhere;
//! - a boolean test of a variant (`matches!`, `==`, `!=`, `if let`, `let .. else`), anywhere:
//!   both send every variant they do not name to the other branch;
//! - a trait item with a default body in `core/src/engine/spec.rs`: each engine must answer
//!   every rule itself.
//!
//! - another name for an engine enum or one of its variants (`use E as A`, `use E::{self as A}`,
//!   `type A = E`, `<E>::V`, and any import of variants: `use E::V`, `use E::*`), anywhere: a
//!   variant is always named as `E::V`, so the scan never has to resolve names.
//!
//! `Self::V` inside an `impl` of an engine enum is resolved. The sites that exist today are counted per area against
//! `engine_seam/budget.toml`: a count may only go down, and a count that went down lowers its
//! budget in the same change. The fixtures in `engine_seam/fixtures/` (read as text, never
//! compiled) prove that the scan reports each kind of violation.

use proc_macro2::{Span, TokenStream, TokenTree};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use syn::parse::{ParseStream, Parser};
use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{Attribute, Expr, Pat, Token};

/// The enums that each carry engine identity. Those not written yet are named all the same: the
/// scan applies to them the day they appear.
const ENGINE_ENUMS: [&str; 6] = ["EngineId", "Dialect", "EngineFacts", "ExplainFormat", "EditFormat", "MonitorFormat"];

/// The variants of the engine enums not in the code yet, used until their definition appears
/// (then the definition's own variants are used).
const PLANNED_VARIANTS: [(&str, &[&str]); 2] =
    [("EngineId", &["Postgres", "MySql", "MariaDb"]), ("EngineFacts", &["Postgres", "MySql", "MariaDb"])];

/// The file whose traits may not have default bodies.
const SPEC_FILE: &str = "crates/core/src/engine/spec.rs";

/// The workspace members whose `src/` the scan must reach (more are walked: every member).
const MEMBERS_THAT_MUST_BE_WALKED: [&str; 6] = [
    "crates/core/src",
    "crates/tui/src",
    "crates/drivers/mysql/src",
    "crates/drivers/postgres/src",
    "crates/export-formats/src",
    "crates/bench/src",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Kind {
    /// A variant of an engine enum named outside the seam modules.
    Reference,
    /// A legacy marker of engine identity outside the seam modules.
    Marker,
    /// A `match` with an arm naming an engine variant and a catch-all arm.
    CatchAll,
    /// `matches!`, `==`, `!=`, `if let` or `let .. else` on an engine variant.
    BooleanTest,
    /// A trait item with a default body in the seam's rule traits.
    DefaultBody,
    /// Another name for an engine enum or variant.
    Alias,
}

impl Kind {
    const ALL: [Kind; 6] =
        [Kind::Reference, Kind::Marker, Kind::CatchAll, Kind::BooleanTest, Kind::DefaultBody, Kind::Alias];

    /// The section of the budget file.
    fn section(self) -> &'static str {
        match self {
            Kind::Reference => "references",
            Kind::Marker => "markers",
            Kind::CatchAll => "catch_alls",
            Kind::BooleanTest => "boolean_tests",
            Kind::DefaultBody => "default_bodies",
            Kind::Alias => "aliases",
        }
    }
}

#[derive(Clone, Debug)]
struct Finding {
    path: String,
    line: usize,
    kind: Kind,
    what: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {:?}: {}", self.path, self.line, self.kind, self.what)
    }
}

/// The variants of each engine enum.
type Variants = BTreeMap<&'static str, BTreeSet<String>>;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("the workspace root")
}

/// The `src/` directory of every member of the root manifest's `[workspace] members`.
fn member_sources(root: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(root.join("Cargo.toml")).expect("the root Cargo.toml");
    let manifest: toml::Table = toml::from_str(&text).expect("the root Cargo.toml parses");
    let members = manifest
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
        .expect("[workspace] members in the root Cargo.toml");
    let mut out = Vec::new();
    for m in members {
        let m = m.as_str().expect("a member is a path");
        // A glob would need expanding: refuse it rather than miss the members it stands for.
        assert!(!m.contains(['*', '?', '[']), "member {m:?}: the scan does not expand globs, list the members");
        let src = format!("{}/src", m.trim_end_matches('/'));
        assert!(root.join(&src).is_dir(), "member {m:?} has no src/ directory");
        out.push(src);
    }
    out
}

/// Every `.rs` file under `dir`, relative to `root` with `/`, test code skipped.
fn walk(root: &Path, dir: &str, out: &mut Vec<String>) {
    let mut entries: Vec<_> = std::fs::read_dir(root.join(dir))
        .unwrap_or_else(|e| panic!("{dir}: {e}"))
        .map(|e| e.unwrap_or_else(|e| panic!("{dir}: {e}")))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().into_string().expect("a UTF-8 file name");
        let rel = format!("{dir}/{name}");
        let ty = entry.file_type().unwrap_or_else(|e| panic!("{rel}: {e}"));
        if ty.is_dir() {
            if name != "tests" {
                walk(root, &rel, out);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" {
            out.push(rel);
        }
    }
}

/// The files the scan reads, relative to the workspace root. Only `tests.rs` and `tests/` are
/// left out here; test-only code inside a file (`#[cfg(test)]` items, or a whole file under an
/// inner `#![cfg(test)]`) is skipped when the file is scanned. A module declaration never takes a
/// file out: the same file may also be reached by a declaration that is not test-only.
fn scanned_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    for src in member_sources(root) {
        walk(root, &src, &mut files);
    }
    files
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn parse(rel: &str, src: &str) -> syn::File {
    syn::parse_file(src).unwrap_or_else(|e| {
        let at = e.span().start();
        panic!("{rel}:{}:{}: the scan cannot parse this file: {e}", at.line, at.column + 1)
    })
}

/// The variants of the engine enums, from their definitions in the workspace (the planned ones
/// for an enum not defined yet).
fn engine_variants(root: &Path, files: &[String]) -> Variants {
    struct Enums<'a>(&'a mut Variants);
    impl<'ast> Visit<'ast> for Enums<'_> {
        fn visit_item_enum(&mut self, e: &'ast syn::ItemEnum) {
            if let Some(name) = ENGINE_ENUMS.iter().find(|n| e.ident == n) {
                self.0.entry(name).or_default().extend(e.variants.iter().map(|v| v.ident.to_string()));
            }
        }
    }
    let mut variants = Variants::new();
    for rel in files {
        Enums(&mut variants).visit_file(&parse(rel, &read(root, rel)));
    }
    for (name, planned) in PLANNED_VARIANTS {
        variants.entry(name).or_insert_with(|| planned.iter().map(|v| v.to_string()).collect());
    }
    variants
}

/// Whether `rel` is the module `module` (its file or a file below it).
fn under(rel: &str, module: &str) -> bool {
    rel == format!("{module}.rs") || rel.starts_with(&format!("{module}/"))
}

/// The seam modules: where engine variants and the legacy markers may be named.
fn in_seam(rel: &str) -> bool {
    const SEAM: [&str; 5] = [
        "crates/core/src/engine",
        "crates/core/src/sql",
        "crates/core/src/edit/sql",
        "crates/core/src/monitor/decode",
        "crates/tui/src/drivers",
    ];
    if SEAM.iter().any(|m| under(rel, m)) {
        return true;
    }
    // Each driver's own engine module.
    let parts: Vec<&str> = rel.split('/').collect();
    parts.len() >= 5
        && parts[0] == "crates"
        && parts[1] == "drivers"
        && parts[3] == "src"
        && (parts[4] == "engine.rs" || (parts[4] == "engine" && parts.len() > 5))
}

/// Whether an item is compiled only into tests: a `#[cfg(..)]` that holds only when `test` does.
fn is_test_only(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("cfg") && a.parse_args::<syn::Meta>().is_ok_and(|m| needs_test(&m)))
}

/// Whether a cfg predicate can hold only with `test`: `test`, an `all(..)` with such a part, an
/// `any(..)` whose every part is such. Anything else (`not(..)`, a feature, a platform) can hold
/// in a build that is no test.
fn needs_test(meta: &syn::Meta) -> bool {
    let parts = || {
        meta.require_list()
            .and_then(|l| l.parse_args_with(Punctuated::<syn::Meta, Token![,]>::parse_terminated))
            .map(|p| p.into_iter().collect::<Vec<_>>())
    };
    match meta {
        syn::Meta::Path(p) => p.is_ident("test"),
        syn::Meta::List(l) if l.path.is_ident("all") => parts().is_ok_and(|p| p.iter().any(needs_test)),
        syn::Meta::List(l) if l.path.is_ident("any") => {
            parts().is_ok_and(|p| !p.is_empty() && p.iter().all(needs_test))
        }
        _ => false,
    }
}

fn line(span: Span) -> usize {
    span.start().line
}

/// The engine enum a name is, if any. An engine enum is only ever named by its own name, and
/// its variants only through it: another name for either is itself reported.
fn engine_enum(name: &str) -> Option<&'static str> {
    ENGINE_ENUMS.iter().find(|e| **e == name).copied()
}

/// The scan of one file.
struct Scan<'a> {
    rel: &'a str,
    seam: bool,
    variants: &'a Variants,
    /// The type of each enclosing `impl` (`None` inside a trait).
    selves: Vec<Option<String>>,
    findings: Vec<Finding>,
}

impl Scan<'_> {
    fn report(&mut self, span: Span, kind: Kind, what: String) {
        self.findings.push(Finding { path: self.rel.to_string(), line: line(span), kind, what });
    }

    /// The engine enum `owner::variant` names, if any (`owner` an engine enum, or `Self` in its
    /// `impl`).
    fn resolve(&self, owner: &str, variant: &str) -> Option<&'static str> {
        let e =
            if owner == "Self" { engine_enum(&self.selves.last().cloned().flatten()?)? } else { engine_enum(owner)? };
        self.variants[e].contains(variant).then_some(e)
    }

    /// The engine variant a path names (`E::V`, `Alias::V`, `Self::V`, an imported `V`).
    fn variant(&self, path: &syn::Path) -> Option<(&'static str, String)> {
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        match segs.as_slice() {
            [] => None,
            [_] => None,
            [.., owner, last] => self.resolve(owner, last).map(|e| (e, last.clone())),
        }
    }

    /// The first engine variant a pattern names, anywhere in it.
    fn pat_variant(&self, pat: &Pat) -> Option<(&'static str, String)> {
        struct Find<'s, 'a> {
            scan: &'s Scan<'a>,
            found: Option<(&'static str, String)>,
        }
        impl<'ast> Visit<'ast> for Find<'_, '_> {
            fn visit_path(&mut self, p: &'ast syn::Path) {
                if self.found.is_none() {
                    self.found = self.scan.variant(p);
                }
                visit::visit_path(self, p);
            }
        }
        let mut find = Find { scan: self, found: None };
        find.visit_pat(pat);
        find.found
    }

    /// Whether a pattern matches every value here: `_`, `..` or a binding (lower case; `None`
    /// names one value).
    fn wildcard(&self, pat: &Pat) -> Option<Span> {
        match pat {
            Pat::Wild(w) => Some(w.underscore_token.span),
            Pat::Rest(r) => Some(r.dot2_token.spans[0]),
            Pat::Ident(i) if i.subpat.is_none() => {
                let name = i.ident.to_string();
                name.starts_with(|c: char| c.is_lowercase() || c == '_').then(|| i.ident.span())
            }
            _ => None,
        }
    }

    /// The positions in a pattern where an engine variant is named.
    fn variant_positions(&self, pat: &Pat, here: &mut Vec<Step>, out: &mut Vec<Vec<Step>>) {
        let pat = peel(pat);
        let names_variant = match pat {
            Pat::Path(p) => self.variant(&p.path).is_some(),
            Pat::TupleStruct(t) => self.variant(&t.path).is_some(),
            Pat::Struct(st) => self.variant(&st.path).is_some(),
            _ => false,
        };
        if names_variant {
            out.push(here.clone());
            return;
        }
        let before = out.len();
        match pat {
            Pat::Or(o) => o.cases.iter().for_each(|c| self.variant_positions(c, here, out)),
            Pat::Tuple(t) => {
                for (at, e) in indices(&t.elems) {
                    here.push(Step::Part(String::new(), at));
                    self.variant_positions(e, here, out);
                    here.pop();
                }
            }
            Pat::TupleStruct(t) => {
                for (at, e) in indices(&t.elems) {
                    here.push(Step::Part(last_name(&t.path), at));
                    self.variant_positions(e, here, out);
                    here.pop();
                }
            }
            Pat::Slice(sl) => {
                for (at, e) in indices(&sl.elems) {
                    here.push(Step::Part("[]".to_string(), at));
                    self.variant_positions(e, here, out);
                    here.pop();
                }
            }
            Pat::Struct(st) => {
                for f in &st.fields {
                    here.push(Step::Field(last_name(&st.path), member_name(&f.member)));
                    self.variant_positions(&f.pat, here, out);
                    here.pop();
                }
            }
            _ => {}
        }
        // A pattern that names a variant somewhere no position was found for (any other kind
        // of pattern): the variant stands here, so a wildcard here is a catch-all.
        if out.len() == before && self.pat_variant(pat).is_some() {
            out.push(here.clone());
        }
    }

    /// Where `pat` matches every value at position `at`: `_` or a binding there or above it, or
    /// a `..` that may cover it.
    fn wildcard_at(&self, pat: &Pat, at: &[Step]) -> Option<Span> {
        let pat = peel(pat);
        if let Some(span) = self.wildcard(pat) {
            return Some(span);
        }
        // Each alternative of `a | b` stands at this same position, the root included.
        if let Pat::Or(o) = pat {
            return o.cases.iter().find_map(|c| self.wildcard_at(c, at));
        }
        let (step, rest) = at.split_first()?;
        // The part at `at`, or the `..` that covers it.
        let part = |elems: &Punctuated<Pat, Token![,]>, at: &Index| -> Result<Option<Span>, ()> {
            let len = elems.len();
            let dots = elems.iter().position(|p| matches!(p, Pat::Rest(_)));
            let i = match (*at, dots) {
                // Before the `..`, counted from the front; after it, from the back.
                (Index::Front(i) | Index::Both(i, _), Some(r)) if i < r => i,
                (Index::Back(k) | Index::Both(_, k), Some(r)) if k < len - 1 - r => len - 1 - k,
                (_, Some(r)) => return Ok(self.wildcard(&elems[r])),
                (Index::Front(i) | Index::Both(i, _), None) => i,
                (Index::Back(k), None) => len.checked_sub(k + 1).ok_or(())?,
            };
            elems.iter().nth(i).map(|p| self.wildcard_at(p, rest)).ok_or(())
        };
        match (step, pat) {
            (Step::Part(name, i), Pat::Tuple(t)) if name.is_empty() => part(&t.elems, i).ok().flatten(),
            (Step::Part(name, i), Pat::Slice(sl)) if name == "[]" => part(&sl.elems, i).ok().flatten(),
            (Step::Part(name, i), Pat::TupleStruct(t)) if *name == last_name(&t.path) => {
                part(&t.elems, i).ok().flatten()
            }
            (Step::Field(name, field), Pat::Struct(st)) if *name == last_name(&st.path) => {
                let named = st.fields.iter().find(|f| member_name(&f.member) == *field);
                match (named, &st.rest) {
                    (Some(f), _) => self.wildcard_at(&f.pat, rest),
                    (None, Some(r)) => Some(r.dot2_token.spans[0]),
                    (None, None) => None,
                }
            }
            _ => None,
        }
    }

    /// The first engine variant an expression names anywhere in it (`E::V`, `Some(E::V)`,
    /// `(n, E::V)`): comparing with it is a test of that variant.
    fn expr_variant(&self, expr: &Expr) -> Option<(&'static str, String)> {
        struct Find<'s, 'a> {
            scan: &'s Scan<'a>,
            found: Option<(&'static str, String)>,
        }
        impl<'ast> Visit<'ast> for Find<'_, '_> {
            fn visit_path(&mut self, p: &'ast syn::Path) {
                if self.found.is_none() {
                    self.found = self.scan.variant(p);
                }
                visit::visit_path(self, p);
            }
            // A macro's operands are tokens: look for `E::V` in them (`dbg!(Dialect::Postgres)`).
            // A `matches!` is a boolean test of its own, counted where it stands.
            fn visit_macro(&mut self, m: &'ast syn::Macro) {
                if self.found.is_none() && !m.path.is_ident("matches") {
                    self.found = self.scan.tokens_variant(m.tokens.clone());
                }
            }
            // Test-only code inside the expression is not scanned here either.
            fn visit_expr(&mut self, e: &'ast Expr) {
                if !is_test_only(expr_attrs(e)) {
                    visit::visit_expr(self, e);
                }
            }
            fn visit_stmt(&mut self, st: &'ast syn::Stmt) {
                if !stmt_is_test_only(st) {
                    visit::visit_stmt(self, st);
                }
            }
            fn visit_item(&mut self, i: &'ast syn::Item) {
                if !is_test_only(item_attrs(i)) {
                    visit::visit_item(self, i);
                }
            }
            fn visit_arm(&mut self, a: &'ast syn::Arm) {
                if !is_test_only(&a.attrs) {
                    visit::visit_arm(self, a);
                }
            }
            fn visit_field_value(&mut self, f: &'ast syn::FieldValue) {
                if !is_test_only(&f.attrs) {
                    visit::visit_field_value(self, f);
                }
            }
        }
        let mut find = Find { scan: self, found: None };
        find.visit_expr(expr);
        find.found
    }

    /// The first `E::V` (or `Self::V` in an engine enum's `impl`) in tokens, groups included.
    fn tokens_variant(&self, tokens: TokenStream) -> Option<(&'static str, String)> {
        let flat: Vec<TokenTree> = tokens.into_iter().collect();
        for (i, t) in flat.iter().enumerate() {
            match t {
                TokenTree::Group(g) => {
                    if let Some(found) = self.tokens_variant(g.stream()) {
                        return Some(found);
                    }
                }
                TokenTree::Ident(id) => {
                    if let (Some(TokenTree::Punct(a)), Some(TokenTree::Punct(b)), Some(TokenTree::Ident(v))) =
                        (flat.get(i + 1), flat.get(i + 2), flat.get(i + 3))
                        && a.as_char() == ':'
                        && b.as_char() == ':'
                        && let Some(e) = self.resolve(&id.to_string(), &v.to_string())
                    {
                        return Some((e, v.to_string()));
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn reference(&mut self, span: Span, e: &str, v: &str) {
        if !self.seam {
            self.report(span, Kind::Reference, format!("{e}::{v} outside the seam modules"));
        }
    }

    fn marker(&mut self, span: Span, what: &str) {
        if !self.seam {
            self.report(span, Kind::Marker, format!("legacy engine marker {what}"));
        }
    }

    /// `matches!(scrutinee, pattern [if guard])`.
    fn matches_macro(&mut self, mac: &syn::Macro) -> bool {
        let parsed = (|input: ParseStream| -> syn::Result<_> {
            let scrutinee: Expr = input.parse()?;
            input.parse::<Token![,]>()?;
            let pat = Pat::parse_multi_with_leading_vert(input)?;
            let guard = if input.peek(Token![if]) {
                input.parse::<Token![if]>()?;
                Some(input.parse::<Expr>()?)
            } else {
                None
            };
            let _ = input.parse::<Option<Token![,]>>()?;
            Ok((scrutinee, pat, guard))
        })
        .parse2(mac.tokens.clone());
        let Ok((scrutinee, pat, guard)) = parsed else { return false };
        if let Some((e, v)) = self.pat_variant(&pat) {
            self.report(mac.path.segments[0].ident.span(), Kind::BooleanTest, format!("matches! on {e}::{v}"));
        }
        self.visit_expr(&scrutinee);
        self.visit_pat(&pat);
        if let Some(g) = &guard {
            self.visit_expr(g);
        }
        true
    }

    /// Any other macro: its arguments as expressions, else its tokens.
    fn other_macro(&mut self, mac: &syn::Macro) {
        if let Ok(args) = Punctuated::<Expr, Token![,]>::parse_terminated.parse2(mac.tokens.clone()) {
            args.iter().for_each(|a| self.visit_expr(a));
            return;
        }
        if let Ok(block) = syn::Block::parse_within.parse2(mac.tokens.clone()) {
            block.iter().for_each(|s| self.visit_stmt(s));
            return;
        }
        // `vec![x; n]`: the tokens of an array.
        let array = TokenTree::Group(proc_macro2::Group::new(proc_macro2::Delimiter::Bracket, mac.tokens.clone()));
        if let Ok(e) = syn::parse2::<Expr>(array.into()) {
            self.visit_expr(&e);
            return;
        }
        self.tokens(mac.tokens.clone());
    }

    /// Engine variants and markers in tokens that do not parse as Rust; a macro call or a
    /// group in them that does parse is scanned as Rust. An engine variant named in tokens that
    /// do not parse counts as a boolean test: the scan cannot tell how it is used.
    fn tokens(&mut self, tokens: TokenStream) {
        let flat: Vec<TokenTree> = tokens.into_iter().collect();
        let mut skip = 0;
        for (i, t) in flat.iter().enumerate() {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            if let (TokenTree::Ident(_), Some(TokenTree::Punct(bang)), Some(TokenTree::Group(_))) =
                (t, flat.get(i + 1), flat.get(i + 2))
                && bang.as_char() == '!'
                && let Ok(mac) = syn::parse2::<syn::Macro>(flat[i..i + 3].iter().cloned().collect())
            {
                self.visit_macro(&mac);
                skip = 2;
                continue;
            }
            match t {
                TokenTree::Group(g) => {
                    if let Ok(exprs) = Punctuated::<Expr, Token![,]>::parse_terminated.parse2(g.stream()) {
                        exprs.iter().for_each(|e| self.visit_expr(e));
                    } else if let Ok(stmts) = syn::Block::parse_within.parse2(g.stream()) {
                        stmts.iter().for_each(|st| self.visit_stmt(st));
                    } else {
                        self.tokens(g.stream());
                    }
                }
                TokenTree::Ident(id) => {
                    let s = id.to_string();
                    if s == "mariadb" || s == "is_mysql" {
                        self.marker(id.span(), &s);
                    }
                    let path_after = match (flat.get(i + 1), flat.get(i + 2), flat.get(i + 3)) {
                        (Some(TokenTree::Punct(a)), Some(TokenTree::Punct(b)), Some(TokenTree::Ident(v)))
                            if a.as_char() == ':' && b.as_char() == ':' =>
                        {
                            Some(v.to_string())
                        }
                        _ => None,
                    };
                    if let Some(v) = &path_after
                        && let Some(e) = self.resolve(&s, v)
                    {
                        self.reference(id.span(), e, v);
                    }
                    // What these tokens do with an engine enum cannot be read (a branch on it,
                    // another name for it): count each mention as a branch on it. Such code
                    // belongs in the seam, as a `match`.
                    let self_enum = || engine_enum(&self.selves.last().cloned().flatten()?);
                    let mentioned = if s == "Self" && path_after.is_some() { self_enum() } else { engine_enum(&s) };
                    if let Some(e) = mentioned {
                        self.report(
                            id.span(),
                            Kind::BooleanTest,
                            format!("{e} in macro tokens the scan cannot read as Rust"),
                        );
                    }
                    // `E::*` or `E::{..}`: an import of the variants.
                    let import_after = match (flat.get(i + 1), flat.get(i + 2), flat.get(i + 3)) {
                        (Some(TokenTree::Punct(a)), Some(TokenTree::Punct(b)), Some(next))
                            if a.as_char() == ':' && b.as_char() == ':' =>
                        {
                            match next {
                                TokenTree::Punct(star) => star.as_char() == '*',
                                TokenTree::Group(g) => g.delimiter() == proc_macro2::Delimiter::Brace,
                                _ => false,
                            }
                        }
                        _ => false,
                    };
                    if import_after && let Some(e) = engine_enum(&s) {
                        let what = format!("an import of {e}'s variants in macro tokens");
                        self.report(id.span(), Kind::Alias, what);
                    }
                }
                TokenTree::Literal(l) => {
                    if l.to_string() == "\"mariadb\"" {
                        self.marker(l.span(), "\"mariadb\"");
                    }
                }
                TokenTree::Punct(_) => {}
            }
        }
    }

    /// `type name = ty`: reported when `ty` is an engine enum.
    fn type_alias(&mut self, name: &proc_macro2::Ident, ty: &syn::Type) {
        if let Some(e) = type_engine(ty) {
            self.report(name.span(), Kind::Alias, format!("`{name}` is another name for {e}"));
        }
    }

    fn enter_impl(&mut self, ty: &syn::Type) {
        let name = match ty {
            syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
            _ => None,
        };
        self.selves.push(name);
    }
}

impl<'ast> Visit<'ast> for Scan<'_> {
    // Doc comments and other attributes say nothing about engines.
    fn visit_attribute(&mut self, _: &'ast Attribute) {}

    fn visit_item(&mut self, item: &'ast syn::Item) {
        if !is_test_only(item_attrs(item)) {
            visit::visit_item(self, item);
        }
    }

    fn visit_trait_item(&mut self, item: &'ast syn::TraitItem) {
        if !is_test_only(trait_item_attrs(item)) {
            visit::visit_trait_item(self, item);
        }
    }

    fn visit_foreign_item(&mut self, item: &'ast syn::ForeignItem) {
        let attrs: &[Attribute] = match item {
            syn::ForeignItem::Fn(i) => &i.attrs,
            syn::ForeignItem::Static(i) => &i.attrs,
            syn::ForeignItem::Type(i) => &i.attrs,
            syn::ForeignItem::Macro(i) => &i.attrs,
            _ => &[],
        };
        if !is_test_only(attrs) {
            visit::visit_foreign_item(self, item);
        }
    }

    fn visit_field_pat(&mut self, f: &'ast syn::FieldPat) {
        if !is_test_only(&f.attrs) {
            visit::visit_field_pat(self, f);
        }
    }

    fn visit_impl_item(&mut self, item: &'ast syn::ImplItem) {
        let attrs: &[Attribute] = match item {
            syn::ImplItem::Const(i) => &i.attrs,
            syn::ImplItem::Fn(i) => &i.attrs,
            syn::ImplItem::Type(i) => &i.attrs,
            syn::ImplItem::Macro(i) => &i.attrs,
            _ => &[],
        };
        if !is_test_only(attrs) {
            visit::visit_impl_item(self, item);
        }
    }

    fn visit_stmt(&mut self, stmt: &'ast syn::Stmt) {
        if !stmt_is_test_only(stmt) {
            visit::visit_stmt(self, stmt);
        }
    }

    fn visit_expr(&mut self, e: &'ast Expr) {
        if !is_test_only(expr_attrs(e)) {
            visit::visit_expr(self, e);
        }
    }

    fn visit_arm(&mut self, a: &'ast syn::Arm) {
        if !is_test_only(&a.attrs) {
            visit::visit_arm(self, a);
        }
    }

    fn visit_field_value(&mut self, f: &'ast syn::FieldValue) {
        if !is_test_only(&f.attrs) {
            visit::visit_field_value(self, f);
        }
    }

    fn visit_variant(&mut self, v: &'ast syn::Variant) {
        if !is_test_only(&v.attrs) {
            visit::visit_variant(self, v);
        }
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        self.enter_impl(&i.self_ty);
        visit::visit_item_impl(self, i);
        self.selves.pop();
    }

    fn visit_item_trait(&mut self, t: &'ast syn::ItemTrait) {
        self.selves.push(None);
        if self.rel == SPEC_FILE {
            for item in t.items.iter().filter(|i| !is_test_only(trait_item_attrs(i))) {
                let (span, name) = match item {
                    syn::TraitItem::Fn(f) if f.default.is_some() => (f.sig.ident.span(), f.sig.ident.to_string()),
                    syn::TraitItem::Const(c) if c.default.is_some() => (c.ident.span(), c.ident.to_string()),
                    syn::TraitItem::Type(ty) if ty.default.is_some() => (ty.ident.span(), ty.ident.to_string()),
                    _ => continue,
                };
                self.report(span, Kind::DefaultBody, format!("trait {}: {name} has a default", t.ident));
            }
        }
        visit::visit_item_trait(self, t);
        self.selves.pop();
    }

    fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
        fn walk(scan: &mut Scan<'_>, prefix: &mut Vec<String>, tree: &syn::UseTree) {
            let owner = prefix.last().and_then(|p| engine_enum(p));
            if let syn::UseTree::Rename(r) = tree {
                let renamed = if r.ident == "self" { prefix.last().cloned() } else { Some(r.ident.to_string()) };
                if let Some(e) = renamed.as_deref().and_then(engine_enum) {
                    scan.report(r.rename.span(), Kind::Alias, format!("`{}` is another name for {e}", r.rename));
                }
            }
            match tree {
                syn::UseTree::Path(p) => {
                    if (p.ident == "Scheme" || p.ident == "Dbms")
                        && let syn::UseTree::Name(n) = &*p.tree
                        && n.ident == "of_driver"
                    {
                        scan.marker(n.ident.span(), &format!("{}::of_driver", p.ident));
                    }
                    prefix.push(p.ident.to_string());
                    walk(scan, prefix, &p.tree);
                    prefix.pop();
                }
                syn::UseTree::Group(g) => g.items.iter().for_each(|t| walk(scan, prefix, t)),
                syn::UseTree::Name(syn::UseName { ident }) | syn::UseTree::Rename(syn::UseRename { ident, .. }) => {
                    if let Some(e) = owner.filter(|e| scan.variants[e].contains(&ident.to_string())) {
                        scan.reference(ident.span(), e, &ident.to_string());
                        let what = format!("`use {e}::{ident}` names the variant without its enum");
                        scan.report(ident.span(), Kind::Alias, what);
                    }
                }
                syn::UseTree::Glob(g) => {
                    if let Some(e) = owner {
                        scan.reference(g.star_token.spans[0], e, "*");
                        let what = format!("`use {e}::*` names its variants without their enum");
                        scan.report(g.star_token.spans[0], Kind::Alias, what);
                    }
                }
            }
        }
        walk(self, &mut Vec::new(), &u.tree);
    }

    // `<E>::V` names a variant past the scan's paths: reported like another name for `E`.
    fn visit_qself(&mut self, q: &'ast syn::QSelf) {
        // `<Self>::..` inside an `impl` of an engine enum is that enum.
        let is_self = matches!(&*q.ty, syn::Type::Path(p) if p.path.is_ident("Self"));
        let self_enum = || engine_enum(&self.selves.last().cloned().flatten()?);
        if let Some(e) = if is_self { self_enum() } else { type_engine(&q.ty) } {
            self.report(q.lt_token.span, Kind::Alias, format!("`<{e}>::..` names {e} through a qualified path"));
        }
        visit::visit_qself(self, q);
    }

    fn visit_item_type(&mut self, t: &'ast syn::ItemType) {
        self.type_alias(&t.ident, &t.ty);
        visit::visit_item_type(self, t);
    }

    // An associated type is another name too: `impl X for Y { type D = Dialect; }`.
    fn visit_impl_item_type(&mut self, t: &'ast syn::ImplItemType) {
        self.type_alias(&t.ident, &t.ty);
        visit::visit_impl_item_type(self, t);
    }

    fn visit_trait_item_type(&mut self, t: &'ast syn::TraitItemType) {
        if let Some((_, ty)) = &t.default {
            self.type_alias(&t.ident, ty);
        }
        visit::visit_trait_item_type(self, t);
    }

    fn visit_path(&mut self, p: &'ast syn::Path) {
        if let Some((e, v)) = self.variant(p) {
            let span = p.segments.last().map(|s| s.ident.span()).unwrap_or_else(Span::call_site);
            self.reference(span, e, &v);
        }
        // `Alias::<E>::V`: a variant of `E` reached through a generic type, another name for `E`.
        if let Some(last) = p.segments.last() {
            let through = p.segments.iter().take(p.segments.len() - 1).find_map(|s| match &s.arguments {
                syn::PathArguments::AngleBracketed(a) => a.args.iter().find_map(|g| match g {
                    syn::GenericArgument::Type(ty) => type_engine(ty),
                    _ => None,
                }),
                _ => None,
            });
            if let Some(e) = through.filter(|e| self.variants[e].contains(&last.ident.to_string())) {
                let what = format!("`..::<{e}>::{}` names {e} through a generic type", last.ident);
                self.report(last.ident.span(), Kind::Alias, what);
            }
        }
        let segs: Vec<String> = p.segments.iter().map(|s| s.ident.to_string()).collect();
        if let [.., owner, last] = segs.as_slice() {
            let owner =
                if owner == "Self" { self.selves.last().cloned().flatten().unwrap_or_default() } else { owner.clone() };
            if last == "of_driver" && (owner == "Scheme" || owner == "Dbms") {
                self.marker(
                    p.segments.last().map(|s| s.ident.span()).unwrap_or_else(Span::call_site),
                    &format!("{owner}::of_driver"),
                );
            }
        }
        visit::visit_path(self, p);
    }

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        if f.sig.ident == "of_driver"
            && let Some(Some(owner)) = self.selves.last().cloned()
            && (owner == "Scheme" || owner == "Dbms")
        {
            self.marker(f.sig.ident.span(), &format!("{owner}::of_driver"));
        }
        visit::visit_impl_item_fn(self, f);
    }

    fn visit_ident(&mut self, i: &'ast proc_macro2::Ident) {
        if i == "mariadb" || i == "is_mysql" {
            self.marker(i.span(), &i.to_string());
        }
    }

    fn visit_lit_str(&mut self, s: &'ast syn::LitStr) {
        if s.value() == "mariadb" {
            self.marker(s.span(), "\"mariadb\"");
        }
    }

    fn visit_field(&mut self, f: &'ast syn::Field) {
        if is_test_only(&f.attrs) {
            return;
        }
        if let Some(name) = &f.ident
            && (name == "mysql" || name == "pg")
            && is_bool(&f.ty)
        {
            self.marker(name.span(), &format!("{name}: bool"));
        }
        visit::visit_field(self, f);
    }

    fn visit_pat_type(&mut self, p: &'ast syn::PatType) {
        if is_test_only(&p.attrs) {
            return;
        }
        if let Pat::Ident(i) = &*p.pat
            && (i.ident == "mysql" || i.ident == "pg")
            && is_bool(&p.ty)
        {
            self.marker(i.ident.span(), &format!("{}: bool", i.ident));
        }
        visit::visit_pat_type(self, p);
    }

    // A catch-all is a `_`, `..` or binding at a position where another arm names an engine
    // variant: `Some(_)` beside `Some(Dialect::Postgres)`, `(_, n)` beside `(Dialect::Postgres, 0)`.
    fn visit_expr_match(&mut self, m: &'ast syn::ExprMatch) {
        let arms: Vec<&syn::Arm> = m.arms.iter().filter(|a| !is_test_only(&a.attrs)).collect();
        let named = arms.iter().find_map(|a| self.pat_variant(&a.pat));
        let mut positions = Vec::new();
        for arm in &arms {
            self.variant_positions(&arm.pat, &mut Vec::new(), &mut positions);
        }
        let caught = arms.iter().find_map(|arm| positions.iter().find_map(|at| self.wildcard_at(&arm.pat, at)));
        if let (Some((e, v)), Some(span)) = (named, caught) {
            self.report(span, Kind::CatchAll, format!("catch-all arm in a match naming {e}::{v}"));
        }
        visit::visit_expr_match(self, m);
    }

    fn visit_expr_binary(&mut self, b: &'ast syn::ExprBinary) {
        let op = match &b.op {
            syn::BinOp::Eq(t) => Some(("==", t.spans[0])),
            syn::BinOp::Ne(t) => Some(("!=", t.spans[0])),
            _ => None,
        };
        if let Some((op, span)) = op
            && let Some((e, v)) = self.expr_variant(&b.left).or_else(|| self.expr_variant(&b.right))
        {
            self.report(span, Kind::BooleanTest, format!("{op} {e}::{v}"));
        }
        visit::visit_expr_binary(self, b);
    }

    fn visit_expr_let(&mut self, l: &'ast syn::ExprLet) {
        if let Some((e, v)) = self.pat_variant(&l.pat) {
            self.report(l.let_token.span, Kind::BooleanTest, format!("if let / while let on {e}::{v}"));
        }
        visit::visit_expr_let(self, l);
    }

    fn visit_local(&mut self, l: &'ast syn::Local) {
        if l.init.as_ref().is_some_and(|i| i.diverge.is_some())
            && let Some((e, v)) = self.pat_variant(&l.pat)
        {
            self.report(l.let_token.span, Kind::BooleanTest, format!("let .. else on {e}::{v}"));
        }
        visit::visit_local(self, l);
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        let name = mac.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
        if !(name == "matches" && self.matches_macro(mac)) {
            self.other_macro(mac);
        }
        visit::visit_macro(self, mac);
    }
}

/// One step into a pattern: the n-th part of a tuple (no name) or of a tuple variant `P(..)`, or
/// a named field of `P { .. }`.
#[derive(Clone, Debug, PartialEq)]
enum Step {
    Part(String, Index),
    Field(String, String),
}

/// Where a part of a tuple stands: counted from the front, or from the back for a part after a
/// `..` (`(.., d)`: the last part, whatever the length).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Index {
    Front(usize),
    Back(usize),
    /// A part of a tuple without `..`: both counts are known.
    Both(usize, usize),
}

/// The parts of a tuple pattern with where each stands (the `..` itself included).
fn indices(elems: &Punctuated<Pat, Token![,]>) -> Vec<(Index, &Pat)> {
    let dots = elems.iter().position(|p| matches!(p, Pat::Rest(_)));
    let len = elems.len();
    elems
        .iter()
        .enumerate()
        .map(|(i, p)| match dots {
            Some(r) if i > r => (Index::Back(len - 1 - i), p),
            Some(_) => (Index::Front(i), p),
            None => (Index::Both(i, len - 1 - i), p),
        })
        .collect()
}

/// A field's name, or its index for a tuple variant written with braces (`Some { 0: p }`).
fn member_name(m: &syn::Member) -> String {
    match m {
        syn::Member::Named(n) => n.to_string(),
        syn::Member::Unnamed(i) => i.index.to_string(),
    }
}

fn last_name(path: &syn::Path) -> String {
    path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default()
}

/// A pattern without the wrappers that do not change what it matches (`(p)`, `&p`, `p: T`,
/// `x @ p`).
fn peel(pat: &Pat) -> &Pat {
    match pat {
        Pat::Paren(p) => peel(&p.pat),
        Pat::Reference(r) => peel(&r.pat),
        Pat::Type(t) => peel(&t.pat),
        Pat::Ident(i) => match &i.subpat {
            Some((_, sub)) => peel(sub),
            None => pat,
        },
        _ => pat,
    }
}

/// The engine enum a type is, through parentheses: `Dialect`, `crate::sql::Dialect`, `(Dialect)`.
fn type_engine(ty: &syn::Type) -> Option<&'static str> {
    match ty {
        syn::Type::Paren(p) => type_engine(&p.elem),
        syn::Type::Group(g) => type_engine(&g.elem),
        syn::Type::Path(p) => p.path.segments.last().and_then(|s| engine_enum(&s.ident.to_string())),
        _ => None,
    }
}

fn stmt_is_test_only(stmt: &syn::Stmt) -> bool {
    match stmt {
        syn::Stmt::Local(l) => is_test_only(&l.attrs),
        syn::Stmt::Macro(m) => is_test_only(&m.attrs),
        syn::Stmt::Expr(e, _) => is_test_only(expr_attrs(e)),
        syn::Stmt::Item(i) => is_test_only(item_attrs(i)),
    }
}

fn item_attrs(item: &syn::Item) -> &[Attribute] {
    match item {
        syn::Item::Const(i) => &i.attrs,
        syn::Item::Enum(i) => &i.attrs,
        syn::Item::Fn(i) => &i.attrs,
        syn::Item::Impl(i) => &i.attrs,
        syn::Item::Macro(i) => &i.attrs,
        syn::Item::Mod(i) => &i.attrs,
        syn::Item::Static(i) => &i.attrs,
        syn::Item::Struct(i) => &i.attrs,
        syn::Item::Trait(i) => &i.attrs,
        syn::Item::Type(i) => &i.attrs,
        syn::Item::Union(i) => &i.attrs,
        syn::Item::Use(i) => &i.attrs,
        syn::Item::ExternCrate(i) => &i.attrs,
        syn::Item::ForeignMod(i) => &i.attrs,
        syn::Item::TraitAlias(i) => &i.attrs,
        _ => &[],
    }
}

fn trait_item_attrs(item: &syn::TraitItem) -> &[Attribute] {
    match item {
        syn::TraitItem::Const(i) => &i.attrs,
        syn::TraitItem::Fn(i) => &i.attrs,
        syn::TraitItem::Type(i) => &i.attrs,
        syn::TraitItem::Macro(i) => &i.attrs,
        _ => &[],
    }
}

/// The attributes of an expression (`#[cfg(test)] { .. }` as a statement, for one).
fn expr_attrs(e: &Expr) -> &[Attribute] {
    match e {
        Expr::Array(x) => &x.attrs,
        Expr::Assign(x) => &x.attrs,
        Expr::Async(x) => &x.attrs,
        Expr::Await(x) => &x.attrs,
        Expr::Binary(x) => &x.attrs,
        Expr::Block(x) => &x.attrs,
        Expr::Break(x) => &x.attrs,
        Expr::Call(x) => &x.attrs,
        Expr::Cast(x) => &x.attrs,
        Expr::Closure(x) => &x.attrs,
        Expr::Const(x) => &x.attrs,
        Expr::Continue(x) => &x.attrs,
        Expr::Field(x) => &x.attrs,
        Expr::ForLoop(x) => &x.attrs,
        Expr::Group(x) => &x.attrs,
        Expr::If(x) => &x.attrs,
        Expr::Index(x) => &x.attrs,
        Expr::Let(x) => &x.attrs,
        Expr::Lit(x) => &x.attrs,
        Expr::Loop(x) => &x.attrs,
        Expr::Macro(x) => &x.attrs,
        Expr::Match(x) => &x.attrs,
        Expr::MethodCall(x) => &x.attrs,
        Expr::Paren(x) => &x.attrs,
        Expr::Path(x) => &x.attrs,
        Expr::Range(x) => &x.attrs,
        Expr::Reference(x) => &x.attrs,
        Expr::Repeat(x) => &x.attrs,
        Expr::Return(x) => &x.attrs,
        Expr::Struct(x) => &x.attrs,
        Expr::Try(x) => &x.attrs,
        Expr::TryBlock(x) => &x.attrs,
        Expr::Tuple(x) => &x.attrs,
        Expr::Unary(x) => &x.attrs,
        Expr::Unsafe(x) => &x.attrs,
        Expr::While(x) => &x.attrs,
        Expr::Yield(x) => &x.attrs,
        _ => &[],
    }
}

fn is_bool(ty: &syn::Type) -> bool {
    matches!(ty, syn::Type::Path(p) if p.qself.is_none() && p.path.is_ident("bool"))
}

/// Everything the scan reports in one file, read from `rel` (a path relative to the workspace
/// root, which decides whether the file is a seam module).
fn scan_file(rel: &str, src: &str, variants: &Variants) -> Vec<Finding> {
    let file = parse(rel, src);
    if is_test_only(&file.attrs) {
        return Vec::new();
    }
    let mut scan = Scan { rel, seam: in_seam(rel), variants, selves: Vec::new(), findings: Vec::new() };
    scan.visit_file(&file);
    scan.findings
}

/// The whole workspace, once per test binary.
struct Workspace {
    files: Vec<String>,
    variants: Variants,
    findings: Vec<Finding>,
}

fn workspace() -> &'static Workspace {
    static WORKSPACE: std::sync::OnceLock<Workspace> = std::sync::OnceLock::new();
    WORKSPACE.get_or_init(|| {
        let root = workspace_root();
        let files = scanned_files(&root);
        let variants = engine_variants(&root, &files);
        let findings = files.iter().flat_map(|rel| scan_file(rel, &read(&root, rel), &variants)).collect();
        Workspace { files, variants, findings }
    })
}

/// The budget file: per kind, per area (a module path; the longest one that holds a file counts
/// it), the number of sites allowed.
type Budget = BTreeMap<String, BTreeMap<String, usize>>;

fn budget() -> Budget {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/engine_seam/budget.toml");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let table: toml::Table = toml::from_str(&text).expect("the budget file parses");
    let mut budget = Budget::new();
    for (section, areas) in table {
        assert!(Kind::ALL.iter().any(|k| k.section() == section), "budget section [{section}] is no kind of finding");
        let areas = areas.as_table().unwrap_or_else(|| panic!("budget [{section}] is a table"));
        let areas = areas
            .iter()
            .map(|(area, n)| {
                let n = n.as_integer().and_then(|n| usize::try_from(n).ok());
                (area.clone(), n.unwrap_or_else(|| panic!("budget [{section}] {area:?} is a count")))
            })
            .collect();
        budget.insert(section, areas);
    }
    budget
}

/// The area of the budget a file's findings count in: the longest listed module that holds it,
/// else the file's crate directory and first module (a new area, with no budget).
fn area_of(rel: &str, areas: &BTreeMap<String, usize>) -> String {
    if let Some(area) = areas.keys().filter(|a| under(rel, a) || rel == a.as_str()).max_by_key(|a| a.len()) {
        return area.clone();
    }
    let (src, rest) = rel.split_once("/src/").expect("a file under a src/ directory");
    let first = rest.split('/').next().expect("a module").trim_end_matches(".rs");
    format!("{src}/src/{first}")
}

/// Checks the findings of `kinds` against their budgets: a count above its budget, or below it
/// (lower the budget), fails with the findings and the counts to write.
fn check_budget(kinds: &[Kind]) {
    let ws = workspace();
    let budget = budget();
    let empty = BTreeMap::new();
    let mut failures = Vec::new();
    for &kind in kinds {
        let areas = budget.get(kind.section()).unwrap_or(&empty);
        let mut counted: BTreeMap<String, Vec<&Finding>> = BTreeMap::new();
        for f in ws.findings.iter().filter(|f| f.kind == kind) {
            counted.entry(area_of(&f.path, areas)).or_default().push(f);
        }
        let all_areas: BTreeSet<&String> = areas.keys().chain(counted.keys()).collect();
        for area in all_areas {
            let allowed = areas.get(area).copied().unwrap_or(0);
            let found = counted.get(area).map_or(0, Vec::len);
            if found > allowed {
                let sites: Vec<String> = counted[area].iter().map(|f| format!("    {f}")).collect();
                failures.push(format!(
                    "[{}] {area:?}: {found} sites, the budget is {allowed}; no new ones may be added:\n{}",
                    kind.section(),
                    sites.join("\n")
                ));
            } else if found < allowed {
                failures.push(format!(
                    "[{}] {area:?}: {found} sites, the budget is {allowed}; lower the budget to {found}",
                    kind.section()
                ));
            }
        }
        if failures.is_empty() {
            continue;
        }
        let mut counts = format!("[{}]\n", kind.section());
        for (area, sites) in &counted {
            counts.push_str(&format!("{area:?} = {}\n", sites.len()));
        }
        failures.push(format!("today's counts:\n{counts}"));
    }
    assert!(failures.is_empty(), "engine seam budget (tests/engine_seam/budget.toml):\n{}", failures.join("\n"));
}

/// A fixture of `engine_seam/fixtures/`, scanned as if it were the file at `path`.
fn scan_fixture(name: &str, path: &str) -> Vec<Finding> {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/engine_seam/fixtures/{name}.rs"));
    let src = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    scan_file(path, &src, &workspace().variants)
}

#[test]
fn the_scan_walks_every_workspace_member() {
    let root = workspace_root();
    let ws = workspace();
    let sources = member_sources(&root);
    for must in MEMBERS_THAT_MUST_BE_WALKED {
        assert!(sources.iter().any(|s| s == must), "{must} is not a workspace member's src/");
    }
    for src in &sources {
        assert!(ws.files.iter().any(|f| f.starts_with(&format!("{src}/"))), "no file of {src} was scanned");
    }
    for f in &ws.files {
        assert!(!f.split('/').any(|c| c == "tests" || c == "tests.rs"), "test code was scanned: {f}");
        assert!(sources.iter().any(|s| f.starts_with(&format!("{s}/"))), "{f} is outside every member's src/");
    }
    // Test code is skipped, and it exists: the skip is not vacuous.
    let mut all = Vec::new();
    for src in &sources {
        collect_all(&root, src, &mut all);
    }
    assert!(all.iter().any(|f| f.ends_with("/tests.rs")), "no tests.rs under any src/ to skip");
    // Every other file under a member's src/ is scanned: no module declaration takes one out.
    let expected: Vec<&String> =
        all.iter().filter(|f| !f.split('/').any(|c| c == "tests" || c == "tests.rs")).collect();
    let mut scanned: Vec<&String> = ws.files.iter().collect();
    let mut expected = expected;
    expected.sort();
    scanned.sort();
    assert_eq!(scanned, expected, "the files scanned are not every non-test file under src/");
    // The four engine enums written today were found with their variants.
    for name in ["Dialect", "ExplainFormat", "EditFormat", "MonitorFormat"] {
        assert!(ws.variants.get(name).is_some_and(|v| v.len() >= 2), "the variants of {name} were not found");
    }
}

/// Every `.rs` file under `dir`, test code included.
fn collect_all(root: &Path, dir: &str, out: &mut Vec<String>) {
    for entry in std::fs::read_dir(root.join(dir)).unwrap_or_else(|e| panic!("{dir}: {e}")) {
        let entry = entry.unwrap_or_else(|e| panic!("{dir}: {e}"));
        let name = entry.file_name().into_string().expect("a UTF-8 file name");
        let rel = format!("{dir}/{name}");
        if entry.file_type().unwrap_or_else(|e| panic!("{rel}: {e}")).is_dir() {
            collect_all(root, &rel, out);
        } else if name.ends_with(".rs") {
            out.push(rel);
        }
    }
}

#[test]
fn engine_references_stay_within_budget() {
    check_budget(&[Kind::Reference, Kind::Marker, Kind::Alias]);
}

#[test]
fn catch_alls_stay_within_budget() {
    check_budget(&[Kind::CatchAll]);
}

#[test]
fn boolean_engine_tests_stay_within_budget() {
    check_budget(&[Kind::BooleanTest, Kind::DefaultBody]);
}

#[test]
fn every_violation_fixture_is_reported() {
    // (fixture, the path it is scanned as, what must be reported, with the line)
    type Case = (&'static str, &'static str, &'static [(Kind, usize)]);
    let cases: [Case; 22] = [
        ("driver_engine_branch", "crates/drivers/mysql/src/session.rs", &[(Kind::Reference, 6), (Kind::Reference, 7)]),
        ("tui_if_let_dialect", "crates/tui/src/app/editor.rs", &[(Kind::BooleanTest, 6), (Kind::Reference, 6)]),
        ("catch_all_explain", "crates/core/src/sql/plan.rs", &[(Kind::CatchAll, 9)]),
        ("matches_dialect", "crates/core/src/sql/split.rs", &[(Kind::BooleanTest, 6)]),
        ("self_eq_in_impl", "crates/core/src/sql/dialect.rs", &[(Kind::BooleanTest, 7)]),
        ("alias_use", "crates/core/src/sql/lexer.rs", &[(Kind::Alias, 2)]),
        ("glob_use", "crates/drivers/postgres/src/connect.rs", &[(Kind::Reference, 3), (Kind::Alias, 3)]),
        ("trait_default_body", SPEC_FILE, &[(Kind::DefaultBody, 7)]),
        (
            "non_wildcard_engine_ref",
            "crates/tui/src/app/monitor.rs",
            &[(Kind::Reference, 8), (Kind::Reference, 9), (Kind::Marker, 13), (Kind::Marker, 14)],
        ),
        ("cfg_not_only_test", "crates/core/src/sql/split.rs", &[(Kind::BooleanTest, 7)]),
        ("alias_scopes", "crates/tui/src/app/plan.rs", &[(Kind::Alias, 3), (Kind::Alias, 10)]),
        (
            "macro_forms",
            "crates/core/src/sql/complete.rs",
            &[(Kind::BooleanTest, 6), (Kind::BooleanTest, 7), (Kind::BooleanTest, 15)],
        ),
        ("self_rename", "crates/core/src/sql/split.rs", &[(Kind::Alias, 2)]),
        ("macro_alias", "crates/core/src/sql/split.rs", &[(Kind::Alias, 3)]),
        ("alias_reexport", "crates/core/src/sql.rs", &[(Kind::Alias, 2), (Kind::Alias, 5)]),
        (
            "nested_catch_all",
            "crates/core/src/sql/format.rs",
            &[(Kind::CatchAll, 8), (Kind::CatchAll, 17), (Kind::CatchAll, 23), (Kind::CatchAll, 30)],
        ),
        ("macro_metavar", "crates/core/src/sql/values.rs", &[(Kind::BooleanTest, 7), (Kind::BooleanTest, 13)]),
        ("macro_variant_import", "crates/core/src/sql/values.rs", &[(Kind::Alias, 5), (Kind::Alias, 13)]),
        (
            "wrapped_eq",
            "crates/core/src/sql/values.rs",
            &[(Kind::BooleanTest, 5), (Kind::BooleanTest, 9), (Kind::BooleanTest, 13)],
        ),
        (
            "slice_catch_all",
            "crates/core/src/sql/values.rs",
            &[(Kind::CatchAll, 7), (Kind::CatchAll, 14), (Kind::CatchAll, 22)],
        ),
        ("qualified_self", "crates/tui/src/app/tx.rs", &[(Kind::Alias, 5), (Kind::Alias, 10)]),
        (
            "assoc_type",
            "crates/tui/src/app/tx.rs",
            &[(Kind::Alias, 9), (Kind::Alias, 13), (Kind::Alias, 20), (Kind::Alias, 25)],
        ),
    ];
    let mut missed = Vec::new();
    for (name, path, expected) in cases {
        let found = scan_fixture(name, path);
        for &(kind, line) in expected {
            if !found.iter().any(|f| f.kind == kind && f.line == line) {
                let got: Vec<String> = found.iter().map(|f| format!("    {f}")).collect();
                missed.push(format!("{name}: no {kind:?} at line {line}; reported:\n{}", got.join("\n")));
            }
        }
    }
    assert!(missed.is_empty(), "violations the scan missed:\n{}", missed.join("\n"));
    // The second trait of that fixture (from line 12) has only a test-only method: no finding.
    let extra: Vec<String> =
        scan_fixture("trait_default_body", SPEC_FILE).iter().filter(|f| f.line >= 12).map(|f| f.to_string()).collect();
    assert!(extra.is_empty(), "a test-only trait method was reported: {extra:?}");
}

#[test]
fn the_clean_fixture_passes() {
    // Scanned as a seam module: variants may be named there, in exhaustive matches.
    let found = scan_fixture("clean", "crates/core/src/sql/clean.rs");
    let listed: Vec<String> = found.iter().map(|f| format!("    {f}")).collect();
    assert!(found.is_empty(), "the clean fixture was reported:\n{}", listed.join("\n"));
    // An enum of its own that shares variant names with an engine enum is no engine enum, even
    // under a one-letter name.
    let found = scan_fixture("unrelated_enum", "crates/tui/src/app/chart.rs");
    assert!(found.is_empty(), "an unrelated enum was reported: {found:?}");
    // Parts after a `..` are matched from the back: naming every engine there is exhaustive.
    let found = scan_fixture("rest_then_engine", "crates/core/src/sql/format.rs");
    assert!(found.is_empty(), "an exhaustive match after `..` was reported: {found:?}");
    // A test-only import names nothing for the code around it.
    let found = scan_fixture("test_import", "crates/tui/src/app/chart.rs");
    assert!(found.is_empty(), "a test-only import changed what product code names: {found:?}");
    // A file under an inner `#![cfg(test)]` is test code, whatever it holds.
    let found = scan_fixture("inner_cfg_test", "crates/tui/src/app/probe.rs");
    assert!(found.is_empty(), "a test-only file was reported: {found:?}");
    // Scanned as any other module, its variants are references, but its only legacy markers
    // are test-only.
    let markers: Vec<String> = scan_fixture("clean", "crates/tui/src/app/clean.rs")
        .iter()
        .filter(|f| f.kind == Kind::Marker)
        .map(|f| format!("    {f}"))
        .collect();
    assert!(markers.is_empty(), "test-only markers were reported:\n{}", markers.join("\n"));
}
