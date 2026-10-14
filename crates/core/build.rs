//! Checks the locale catalogs and generates the typed message API (`i18n::Label`,
//! `i18n::Msg`) into `OUT_DIR/messages.rs`, which `src/i18n.rs` includes. A catalog problem
//! (missing, extra or mistyped key, placeholder mismatch) fails the build with the file, the key
//! and the problem. The rules live in `build/catalog.rs`.

#[path = "build/catalog.rs"]
mod catalog;

use catalog::{Locale, Policy};
use std::path::PathBuf;

/// Locales compiled into the binary, the source catalog first. Each needs a `Lang` variant of
/// the same name (`ko` -> `Lang::Ko`). To ship a partial translation, mark it
/// `Policy::Optional`: its missing keys then fall back to English with a build warning.
const LOCALES: &[(&str, Policy)] = &[("en", Policy::Source), ("ko", Policy::Required)];

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo")).join("../../locales");
    println!("cargo::rerun-if-changed=build/catalog.rs");
    let mut locales = Vec::new();
    let mut errors = Vec::new();
    for &(code, policy) in LOCALES {
        let path = dir.join(format!("{code}.toml"));
        println!("cargo::rerun-if-changed={}", path.display());
        let file = format!("locales/{code}.toml");
        let src = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                errors.push(format!("{file}: cannot read {}: {e}", path.display()));
                continue;
            }
        };
        match catalog::parse(&file, &src) {
            Ok((entries, one)) => locales.push(Locale { code: code.to_string(), file, policy, entries, one }),
            Err(ps) => errors.extend(ps.iter().map(ToString::to_string)),
        }
    }
    if errors.is_empty() {
        let (errs, warnings) = catalog::check(&locales);
        for w in &warnings {
            println!("cargo::warning={w} (falls back to English)");
        }
        errors.extend(errs.iter().map(ToString::to_string));
    }
    if !errors.is_empty() {
        // Each `cargo::error` fails the build and is printed as `error: datarig-core@…: <problem>`.
        for e in &errors {
            println!("cargo::error={e}");
        }
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("set by cargo")).join("messages.rs");
    std::fs::write(&out, catalog::generate(&locales)).expect("write OUT_DIR/messages.rs");
}
