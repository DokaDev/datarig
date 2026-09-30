//! User theme files: `<config dir>/themes/<name>.toml`, next to the config file.
//!
//! ```toml
//! extends = "dark"                 # optional: a built-in theme to start from
//!
//! [colors]                         # color tokens
//! bg = "#1e1e2e"
//! warning = "yellow"               # an ANSI color: follows the terminal's palette
//!
//! [styles]                         # style tokens: fg, bg, modifiers (each optional)
//! selection = { bg = "#45475a" }
//! danger_mark = { fg = "red", modifiers = ["bold"] }
//! ```
//!
//! A color is `#rrggbb`, one of the 16 ANSI names ([`ANSI_NAMES`]) or `default` (the
//! terminal's own foreground or background). A style token given in the file replaces the one of
//! the base theme: what it leaves out is not set. The token names belong to the UI, which passes
//! them in ([`Tokens`]); here a file is only read and checked, strictly: an unknown key, token,
//! color or modifier is an error with its line. Nothing here knows how a color is drawn.

use crate::fault::Fault;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use toml::Spanned;

/// The theme when the config file names none (`theme` is written only when it differs).
pub const DEFAULT: &str = "terminal";

/// The 16 ANSI colors in palette order (index 0–15).
pub const ANSI_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright-black",
    "bright-red",
    "bright-green",
    "bright-yellow",
    "bright-blue",
    "bright-magenta",
    "bright-cyan",
    "bright-white",
];

/// A color of a theme file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSpec {
    /// `default`: the terminal's own color.
    Default,
    /// An ANSI color by its index in [`ANSI_NAMES`].
    Ansi(u8),
    Rgb(u8, u8, u8),
}

impl ColorSpec {
    /// `#rrggbb`, an ANSI name or `default` (names ignore case).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if let Some(hex) = s.strip_prefix('#') {
            if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let v = u32::from_str_radix(hex, 16).ok()?;
            return Some(ColorSpec::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8));
        }
        let lower = s.to_ascii_lowercase();
        if lower == "default" {
            return Some(ColorSpec::Default);
        }
        ANSI_NAMES.iter().position(|n| *n == lower).map(|i| ColorSpec::Ansi(i as u8))
    }
}

/// A text modifier of a style token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierSpec {
    Bold,
    Dim,
    Italic,
    Underlined,
    Reversed,
    CrossedOut,
}

impl ModifierSpec {
    pub const ALL: [(&'static str, ModifierSpec); 6] = [
        ("bold", ModifierSpec::Bold),
        ("dim", ModifierSpec::Dim),
        ("italic", ModifierSpec::Italic),
        ("underlined", ModifierSpec::Underlined),
        ("reversed", ModifierSpec::Reversed),
        ("crossed-out", ModifierSpec::CrossedOut),
    ];

    pub fn parse(s: &str) -> Option<Self> {
        let lower = s.trim().to_ascii_lowercase();
        Self::ALL.iter().find(|(n, _)| *n == lower).map(|(_, m)| *m)
    }
}

/// A style token of a theme file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StyleSpec {
    pub fg: Option<ColorSpec>,
    pub bg: Option<ColorSpec>,
    pub modifiers: Vec<ModifierSpec>,
}

/// The token names a theme file may set: colors under `[colors]`, styles under `[styles]`.
#[derive(Clone, Copy, Debug)]
pub struct Tokens<'a> {
    pub colors: &'a [&'a str],
    pub styles: &'a [&'a str],
}

/// A theme file, read and checked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThemeSpec {
    /// The built-in theme it starts from, and the line that names it.
    pub extends: Option<(String, usize)>,
    /// In file order.
    pub colors: Vec<(String, ColorSpec)>,
    pub styles: Vec<(String, StyleSpec)>,
}

/// What is wrong in a theme file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// Not TOML, an unknown key or a value of the wrong type (the parser's words).
    Toml(String),
    /// Not a token of any theme.
    UnknownToken(String),
    /// A style token under `[colors]`.
    NotAColor(String),
    /// A color token under `[styles]`.
    NotAStyle(String),
    BadColor(String),
    BadModifier(String),
    /// `extends` names no built-in theme.
    UnknownBase(String),
}

/// Why a theme cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeError {
    /// No built-in theme and no theme file of this name (`dir`: where files were looked for).
    Unknown { name: String, dir: Option<PathBuf> },
    /// A theme file has the name of a built-in theme (it is never read).
    Shadows { path: PathBuf },
    /// The file could not be read.
    Read { path: PathBuf, fault: Fault },
    /// The file has an error (`line`: where, when known).
    File { path: PathBuf, line: Option<usize>, problem: Problem },
}

/// A name a theme file may have: letters, digits, `-` and `_` (so it is a plain file name).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The theme directory of the config file `config`: `themes/` next to it.
pub fn dir(config: &Path) -> Option<PathBuf> {
    config.parent().map(|p| p.join("themes"))
}

/// The names of the theme files in `dir` (`<name>.toml` with a [`valid_name`]), sorted. A
/// missing or unreadable directory has none.
pub fn names(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("toml") || !path.is_file() {
                return None;
            }
            path.file_stem().and_then(|s| s.to_str()).filter(|s| valid_name(s)).map(str::to_string)
        })
        .collect();
    out.sort();
    out
}

/// The file of theme `name` in `dir`.
pub fn path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.toml"))
}

/// Read theme `name` from `dir`. A file that is not there is [`ThemeError::Unknown`].
pub fn load(dir: &Path, name: &str, tokens: Tokens) -> Result<ThemeSpec, ThemeError> {
    if !valid_name(name) {
        return Err(ThemeError::Unknown { name: name.to_string(), dir: Some(dir.to_path_buf()) });
    }
    let path = path(dir, name);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ThemeError::Unknown { name: name.to_string(), dir: Some(dir.to_path_buf()) });
        }
        Err(e) => return Err(ThemeError::Read { fault: Fault::io_at(&e, &path), path }),
    };
    parse(&text, tokens).map_err(|(line, problem)| ThemeError::File { path, line, problem })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    extends: Option<Spanned<String>>,
    #[serde(default)]
    colors: BTreeMap<Spanned<String>, Spanned<String>>,
    #[serde(default)]
    styles: BTreeMap<Spanned<String>, Spanned<StyleFile>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StyleFile {
    #[serde(default)]
    fg: Option<Spanned<String>>,
    #[serde(default)]
    bg: Option<Spanned<String>>,
    #[serde(default)]
    modifiers: Vec<Spanned<String>>,
}

/// Check the text of a theme file. `Err`: the first problem in the file and its line.
pub fn parse(text: &str, tokens: Tokens) -> Result<ThemeSpec, (Option<usize>, Problem)> {
    let line = |at: usize| text[..at.min(text.len())].matches('\n').count() + 1;
    let f: File = toml::from_str(text)
        .map_err(|e| (e.span().map(|s| line(s.start)), Problem::Toml(e.message().trim().to_string())))?;
    // Every problem with where it is; the first one in the file is reported.
    let mut problems: Vec<(usize, Problem)> = Vec::new();
    let token = |k: &Spanned<String>, mine: &[&str], other: &[&str], wrong: fn(String) -> Problem| {
        let name = k.get_ref();
        match () {
            _ if mine.contains(&name.as_str()) => None,
            _ if other.contains(&name.as_str()) => Some((k.span().start, wrong(name.clone()))),
            _ => Some((k.span().start, Problem::UnknownToken(name.clone()))),
        }
    };
    let color = |c: &Spanned<String>, problems: &mut Vec<(usize, Problem)>| {
        let parsed = ColorSpec::parse(c.get_ref());
        if parsed.is_none() {
            problems.push((c.span().start, Problem::BadColor(c.get_ref().clone())));
        }
        parsed
    };
    let mut colors: Vec<(usize, String, ColorSpec)> = Vec::new();
    for (k, v) in &f.colors {
        if let Some(p) = token(k, tokens.colors, tokens.styles, Problem::NotAColor) {
            problems.push(p);
        } else if let Some(c) = color(v, &mut problems) {
            colors.push((k.span().start, k.get_ref().clone(), c));
        }
    }
    let mut styles: Vec<(usize, String, StyleSpec)> = Vec::new();
    for (k, v) in &f.styles {
        if let Some(p) = token(k, tokens.styles, tokens.colors, Problem::NotAStyle) {
            problems.push(p);
            continue;
        }
        let s = v.get_ref();
        let fg = s.fg.as_ref().and_then(|c| color(c, &mut problems));
        let bg = s.bg.as_ref().and_then(|c| color(c, &mut problems));
        let mut modifiers = Vec::new();
        for m in &s.modifiers {
            match ModifierSpec::parse(m.get_ref()) {
                Some(parsed) => modifiers.push(parsed),
                None => problems.push((m.span().start, Problem::BadModifier(m.get_ref().clone()))),
            }
        }
        styles.push((k.span().start, k.get_ref().clone(), StyleSpec { fg, bg, modifiers }));
    }
    if let Some((at, p)) = problems.into_iter().min_by_key(|p| p.0) {
        return Err((Some(line(at)), p));
    }
    colors.sort_by_key(|c| c.0);
    styles.sort_by_key(|s| s.0);
    Ok(ThemeSpec {
        extends: f.extends.map(|e| (e.get_ref().clone(), line(e.span().start))),
        colors: colors.into_iter().map(|(_, n, c)| (n, c)).collect(),
        styles: styles.into_iter().map(|(_, n, s)| (n, s)).collect(),
    })
}

#[cfg(test)]
mod tests;
