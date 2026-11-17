//! The engine seam check: what an engine is decides in a few seam modules only, and no code
//! sends the engines it does not name to a default branch.
//!
//! The scan parses every `.rs` file under the `src/` of every workspace member (test code
//! skipped: `tests.rs`, `tests/` and `#[cfg(test)]` items may name engines) and reports, for the
//! six engine enums (`EngineId`, `Dialect`, `EngineFacts`, `ExplainFormat`, `EditFormat`,
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
//! `Self::V` inside an `impl` of an engine enum, `use E::*`, `use E::{V}`, `use E as Alias` and
//! `type Alias = E` are resolved first. The sites that exist today are counted per area against
//! `engine_seam/budget.toml`: a count may only go down, and a count that went down lowers its
//! budget in the same change. The fixtures in `engine_seam/fixtures/` (read as text, never
//! compiled) prove that the scan reports each kind of violation.

use proc_macro2::{Span, TokenStream, TokenTree};
use std::collections::{BTreeMap, BTreeSet, HashMap};
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
}

impl Kind {
    const ALL: [Kind; 5] = [Kind::Reference, Kind::Marker, Kind::CatchAll, Kind::BooleanTest, Kind::DefaultBody];

    /// The section of the budget file.
    fn section(self) -> &'static str {
        match self {
            Kind::Reference => "references",
            Kind::Marker => "markers",
            Kind::CatchAll => "catch_alls",
            Kind::BooleanTest => "boolean_tests",
            Kind::DefaultBody => "default_bodies",
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

/// The files the scan reads, relative to the workspace root.
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

/// The names one file gives the engine enums and their variants.
#[derive(Default)]
struct Names {
    /// A name of an engine enum in this file (itself, `use E as A`, `type A = E`).
    enums: HashMap<String, &'static str>,
    /// A variant named without its enum (`use E::V`, `use E::*`).
    bare: HashMap<String, (&'static str, String)>,
}

impl Names {
    fn new(file: &syn::File, variants: &Variants) -> Names {
        let mut names = Names::default();
        for name in ENGINE_ENUMS {
            names.enums.insert(name.to_string(), name);
        }
        // Aliases first (twice: an alias may name another alias), then the variants they reach.
        for _ in 0..2 {
            Collect { names: &mut names, variants, variants_pass: false }.visit_file(file);
        }
        Collect { names: &mut names, variants, variants_pass: true }.visit_file(file);
        names
    }
}

struct Collect<'a> {
    names: &'a mut Names,
    variants: &'a Variants,
    variants_pass: bool,
}

impl Collect<'_> {
    fn tree(&mut self, prefix: &mut Vec<String>, tree: &syn::UseTree) {
        let owner = prefix.last().and_then(|p| self.names.enums.get(p).copied());
        match tree {
            syn::UseTree::Path(p) => {
                prefix.push(p.ident.to_string());
                self.tree(prefix, &p.tree);
                prefix.pop();
            }
            syn::UseTree::Group(g) => g.items.iter().for_each(|t| self.tree(prefix, t)),
            syn::UseTree::Name(n) => {
                if let (true, Some(e)) = (self.variants_pass, owner) {
                    let v = n.ident.to_string();
                    if self.variants[e].contains(&v) {
                        self.names.bare.insert(v.clone(), (e, v));
                    }
                }
            }
            syn::UseTree::Rename(r) => {
                let from = r.ident.to_string();
                if let Some(e) = self.names.enums.get(&from).copied() {
                    self.names.enums.insert(r.rename.to_string(), e);
                } else if let (true, Some(e)) = (self.variants_pass, owner)
                    && self.variants[e].contains(&from)
                {
                    self.names.bare.insert(r.rename.to_string(), (e, from));
                }
            }
            syn::UseTree::Glob(_) => {
                if let (true, Some(e)) = (self.variants_pass, owner) {
                    for v in &self.variants[e] {
                        self.names.bare.insert(v.clone(), (e, v.clone()));
                    }
                }
            }
        }
    }
}

impl<'ast> Visit<'ast> for Collect<'_> {
    fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
        self.tree(&mut Vec::new(), &u.tree);
    }
    fn visit_item_type(&mut self, t: &'ast syn::ItemType) {
        if let syn::Type::Path(p) = &*t.ty
            && let Some(last) = p.path.segments.last()
            && let Some(e) = self.names.enums.get(&last.ident.to_string()).copied()
        {
            self.names.enums.insert(t.ident.to_string(), e);
        }
    }
}

/// The scan of one file.
struct Scan<'a> {
    rel: &'a str,
    seam: bool,
    names: Names,
    variants: &'a Variants,
    /// The type of each enclosing `impl` (`None` inside a trait).
    selves: Vec<Option<String>>,
    findings: Vec<Finding>,
}

impl Scan<'_> {
    fn report(&mut self, span: Span, kind: Kind, what: String) {
        self.findings.push(Finding { path: self.rel.to_string(), line: line(span), kind, what });
    }

    /// The engine enum `Self` stands for here.
    fn self_enum(&self) -> Option<&'static str> {
        let ty = self.selves.last()?.as_ref()?;
        self.names.enums.get(ty).copied()
    }

    /// The engine variant a path names (`E::V`, `Alias::V`, `Self::V`, an imported `V`).
    fn variant(&self, path: &syn::Path) -> Option<(&'static str, String)> {
        let segs: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        match segs.as_slice() {
            [] => None,
            [one] => self.names.bare.get(one).cloned(),
            [.., owner, last] => {
                let e = if owner == "Self" { self.self_enum()? } else { self.names.enums.get(owner).copied()? };
                self.variants[e].contains(last).then(|| (e, last.clone()))
            }
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
            fn visit_pat_ident(&mut self, p: &'ast syn::PatIdent) {
                if self.found.is_none() && p.subpat.is_none() {
                    self.found = self.scan.names.bare.get(&p.ident.to_string()).cloned();
                }
                visit::visit_pat_ident(self, p);
            }
        }
        let mut find = Find { scan: self, found: None };
        find.visit_pat(pat);
        find.found
    }

    /// Whether a pattern matches every value (`_`, a binding, a tuple of those).
    fn catch_all(&self, pat: &Pat) -> bool {
        match pat {
            Pat::Wild(_) | Pat::Rest(_) => true,
            Pat::Ident(i) => match &i.subpat {
                Some((_, sub)) => self.catch_all(sub),
                // A binding is lower case; `None` or an imported variant names one value.
                None => {
                    let name = i.ident.to_string();
                    name.starts_with(|c: char| c.is_lowercase() || c == '_') && !self.names.bare.contains_key(&name)
                }
            },
            Pat::Tuple(t) => t.elems.iter().all(|p| self.catch_all(p)),
            Pat::Paren(p) => self.catch_all(&p.pat),
            Pat::Reference(r) => self.catch_all(&r.pat),
            Pat::Type(t) => self.catch_all(&t.pat),
            Pat::Or(o) => o.cases.iter().any(|p| self.catch_all(p)),
            _ => false,
        }
    }

    /// The engine variant an expression is (`E::V`, `E::V(..)`, `E::V { .. }`).
    fn expr_variant(&self, expr: &Expr) -> Option<(&'static str, String)> {
        match expr {
            Expr::Path(p) => self.variant(&p.path),
            Expr::Call(c) => match &*c.func {
                Expr::Path(p) => self.variant(&p.path),
                _ => None,
            },
            Expr::Struct(s) => self.variant(&s.path),
            Expr::Paren(p) => self.expr_variant(&p.expr),
            Expr::Group(g) => self.expr_variant(&g.expr),
            Expr::Reference(r) => self.expr_variant(&r.expr),
            _ => None,
        }
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
        self.tokens(mac.tokens.clone());
    }

    /// Engine variants and markers in tokens that do not parse as Rust.
    fn tokens(&mut self, tokens: TokenStream) {
        let flat: Vec<TokenTree> = tokens.into_iter().collect();
        for (i, t) in flat.iter().enumerate() {
            match t {
                TokenTree::Group(g) => self.tokens(g.stream()),
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
                    if let Some(v) = path_after {
                        let e = if s == "Self" { self.self_enum() } else { self.names.enums.get(&s).copied() };
                        if let Some(e) = e.filter(|e| self.variants[e].contains(&v)) {
                            self.reference(id.span(), e, &v);
                        }
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
        let attrs: &[Attribute] = match item {
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
            _ => &[],
        };
        if !is_test_only(attrs) {
            visit::visit_item(self, item);
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
        let test_only = match stmt {
            syn::Stmt::Local(l) => is_test_only(&l.attrs),
            syn::Stmt::Macro(m) => is_test_only(&m.attrs),
            _ => false,
        };
        if !test_only {
            visit::visit_stmt(self, stmt);
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
            for item in &t.items {
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
            let owner = prefix.last().and_then(|p| scan.names.enums.get(p).copied());
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
                    }
                }
                syn::UseTree::Glob(g) => {
                    if let Some(e) = owner {
                        scan.reference(g.star_token.spans[0], e, "*");
                    }
                }
            }
        }
        walk(self, &mut Vec::new(), &u.tree);
    }

    fn visit_path(&mut self, p: &'ast syn::Path) {
        if let Some((e, v)) = self.variant(p) {
            let span = p.segments.last().map(|s| s.ident.span()).unwrap_or_else(Span::call_site);
            self.reference(span, e, &v);
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

    fn visit_pat_ident(&mut self, p: &'ast syn::PatIdent) {
        if p.subpat.is_none()
            && let Some((e, v)) = self.names.bare.get(&p.ident.to_string()).cloned()
        {
            self.reference(p.ident.span(), e, &v);
        }
        visit::visit_pat_ident(self, p);
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
        if let Some(name) = &f.ident
            && (name == "mysql" || name == "pg")
            && is_bool(&f.ty)
        {
            self.marker(name.span(), &format!("{name}: bool"));
        }
        visit::visit_field(self, f);
    }

    fn visit_pat_type(&mut self, p: &'ast syn::PatType) {
        if let Pat::Ident(i) = &*p.pat
            && (i.ident == "mysql" || i.ident == "pg")
            && is_bool(&p.ty)
        {
            self.marker(i.ident.span(), &format!("{}: bool", i.ident));
        }
        visit::visit_pat_type(self, p);
    }

    fn visit_expr_match(&mut self, m: &'ast syn::ExprMatch) {
        let named = m.arms.iter().find_map(|a| self.pat_variant(&a.pat));
        if let Some((e, v)) = named
            && let Some(arm) = m.arms.iter().find(|a| self.catch_all(&a.pat))
        {
            let span = match &arm.pat {
                Pat::Wild(w) => w.underscore_token.span,
                Pat::Ident(i) => i.ident.span(),
                _ => m.match_token.span,
            };
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

fn is_bool(ty: &syn::Type) -> bool {
    matches!(ty, syn::Type::Path(p) if p.qself.is_none() && p.path.is_ident("bool"))
}

/// Everything the scan reports in one file, read from `rel` (a path relative to the workspace
/// root, which decides whether the file is a seam module).
fn scan_file(rel: &str, src: &str, variants: &Variants) -> Vec<Finding> {
    let file = parse(rel, src);
    let names = Names::new(&file, variants);
    let mut scan = Scan { rel, seam: in_seam(rel), names, variants, selves: Vec::new(), findings: Vec::new() };
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
    check_budget(&[Kind::Reference, Kind::Marker]);
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
    let cases: [Case; 10] = [
        ("driver_engine_branch", "crates/drivers/mysql/src/session.rs", &[(Kind::Reference, 6), (Kind::Reference, 7)]),
        ("tui_if_let_dialect", "crates/tui/src/app/editor.rs", &[(Kind::BooleanTest, 6), (Kind::Reference, 6)]),
        ("catch_all_explain", "crates/core/src/sql/plan.rs", &[(Kind::CatchAll, 9)]),
        ("matches_dialect", "crates/core/src/sql/split.rs", &[(Kind::BooleanTest, 6)]),
        ("self_eq_in_impl", "crates/core/src/sql/dialect.rs", &[(Kind::BooleanTest, 7)]),
        ("alias_use", "crates/core/src/sql/lexer.rs", &[(Kind::BooleanTest, 7)]),
        ("glob_use", "crates/drivers/postgres/src/connect.rs", &[(Kind::Reference, 3), (Kind::CatchAll, 9)]),
        ("trait_default_body", SPEC_FILE, &[(Kind::DefaultBody, 7)]),
        (
            "non_wildcard_engine_ref",
            "crates/tui/src/app/monitor.rs",
            &[(Kind::Reference, 8), (Kind::Reference, 9), (Kind::Marker, 13), (Kind::Marker, 14)],
        ),
        ("cfg_not_only_test", "crates/core/src/sql/split.rs", &[(Kind::BooleanTest, 7)]),
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
}

#[test]
fn the_clean_fixture_passes() {
    // Scanned as a seam module: variants may be named there, in exhaustive matches.
    let found = scan_fixture("clean", "crates/core/src/sql/clean.rs");
    let listed: Vec<String> = found.iter().map(|f| format!("    {f}")).collect();
    assert!(found.is_empty(), "the clean fixture was reported:\n{}", listed.join("\n"));
}
