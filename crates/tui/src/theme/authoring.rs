//! Writing theme files: `:theme copy` writes every token of a theme, set, so the file stands on
//! its own; `:theme new` writes every token of the built-in theme it extends, commented out, to
//! be filled in. Each key has the description of its token above it ([`COLOR_ABOUT`],
//! [`STYLE_ABOUT`]), the same text `docs/themes.md` is generated from ([`doc`]).
//!
//! A file is created, never replaced ([`datarig_core::fsutil::create_new`]): a name that is
//! taken is an error, whatever the file holds.

use super::{
    ANSI, Background, COLOR_ABOUT, FAMILIES, NAMES, STYLE_ABOUT, Theme, ThemeError, builtin, color_of, modifier,
    resolve_with_base, style_of,
};
use datarig_core::fault::Fault;
use datarig_core::theme::{ANSI_NAMES, ModifierSpec};
use ratatui::style::{Color, Modifier, Style};
use std::path::{Path, PathBuf};

/// Why a theme file was not written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthorError {
    /// Not a name a theme file may have ([`datarig_core::theme::valid_name`]).
    BadName(String),
    /// The name of a built-in theme, ignoring case (a file of that name would never be used, and
    /// where the file system ignores case it would hide the built-in theme).
    BuiltinName(String),
    /// The theme to copy cannot be used (unknown, or a broken file).
    Source(ThemeError),
    /// A token of the theme has a color or modifier a theme file cannot hold.
    Unwritable { token: &'static str },
    /// There is a file of this name already (it is left as it is).
    Exists(PathBuf),
    /// The file could not be written.
    Write { path: PathBuf, fault: Fault },
    /// There is no themes folder: no config file is used.
    NoDir,
}

/// A name `:theme copy` and `:theme new` may write: a [`datarig_core::theme::valid_name`]
/// (ASCII letters, digits, `-` and `_`, at most 64) that is no built-in theme's and no Windows
/// device name (`con`, `prn`, `aux`, `nul`, `com1`-`com9`, `lpt1`-`lpt9`), ignoring case.
pub fn check_name(name: &str) -> Result<(), AuthorError> {
    if !datarig_core::theme::valid_name(name) {
        return Err(AuthorError::BadName(name.to_string()));
    }
    if NAMES.iter().any(|n| n.eq_ignore_ascii_case(name)) {
        return Err(AuthorError::BuiltinName(name.to_string()));
    }
    // A device name on Windows (`nul.toml` is the device there, whatever the extension).
    let lower = name.to_ascii_lowercase();
    let numbered = |prefix: &str| {
        lower.strip_prefix(prefix).is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit() && n != "0")
    };
    if ["con", "prn", "aux", "nul"].contains(&lower.as_str()) || numbered("com") || numbered("lpt") {
        return Err(AuthorError::BadName(name.to_string()));
    }
    Ok(())
}

/// `:theme copy <from> <name>`: write `<dir>/<name>.toml` with every token of theme `from` as it
/// is drawn now (a family as its variant for `background`), extending the built-in theme `from`
/// starts from (for the dimming behind dialogs, which is not a token). Returns the file's path.
pub fn copy_theme(dir: &Path, from: &str, name: &str, background: Background) -> Result<PathBuf, AuthorError> {
    check_name(name)?;
    let (th, base) = resolve_with_base(from, background, Some(dir)).map_err(AuthorError::Source)?;
    let header = format!(
        "# A datarig theme: a copy of \"{from}\" with every token set, so it stands on its own.\n\
         # `extends` only keeps the dimming behind dialogs of \"{base}\" (not a token).\n"
    );
    write(dir, name, &file_text(&header, base, &th, false)?)
}

/// `:theme new <name>`: write `<dir>/<name>.toml` extending the built-in theme `base`, with every
/// token commented out and showing the value it takes from `base` (for `background`). Returns
/// the file's path.
pub fn new_theme(dir: &Path, name: &str, base: &str, background: Background) -> Result<PathBuf, AuthorError> {
    check_name(name)?;
    let th = builtin(base, background)
        .ok_or_else(|| AuthorError::Source(ThemeError::Unknown { name: base.to_string(), dir: None }))?;
    // A family's variant follows the terminal's background: the values shown are the one drawn now.
    let shown = match FAMILIES.iter().find(|f| f.name == base) {
        Some(f) if background == Background::Light => format!("\"{base}\" (its light variant, {}, drawn now)", f.light),
        Some(f) => format!("\"{base}\" (its dark variant, {}, drawn now)", f.dark),
        None => format!("\"{base}\""),
    };
    let header = format!(
        "# A datarig theme extending \"{base}\". Every token is listed with the value it takes from\n\
         # {shown}, commented out: remove the \"# \" in front of a key to set it.\n"
    );
    write(dir, name, &file_text(&header, base, th, true)?)
}

/// Create `<dir>/<name>.toml` (and `dir` when it is missing) with `text`.
fn write(dir: &Path, name: &str, text: &str) -> Result<PathBuf, AuthorError> {
    // Apart from the file, so a folder that cannot be made is never taken for a taken name.
    std::fs::create_dir_all(dir)
        .map_err(|e| AuthorError::Write { fault: Fault::io_at(&e, dir), path: dir.to_path_buf() })?;
    let path = datarig_core::theme::path(dir, name);
    match datarig_core::fsutil::create_new(&path, text.as_bytes()) {
        Ok(()) => Ok(path),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(AuthorError::Exists(path)),
        Err(e) => Err(AuthorError::Write { fault: Fault::io_at(&e, &path), path }),
    }
}

/// The lines `docs/themes.md` (as Markdown) and every written file (without the backticks)
/// explain the values with.
fn format_lines() -> [String; 3] {
    let code = |names: &mut dyn Iterator<Item = &str>| names.map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ");
    [
        format!(
            "A color is `#rrggbb`, an ANSI name ({}) or `default` (the terminal's own).",
            code(&mut ANSI_NAMES.iter().copied())
        ),
        "A style is `{ fg = <color>, bg = <color>, modifiers = [...] }`, each part optional; what it leaves out is \
         not set."
            .to_string(),
        format!("Modifiers: {}.", code(&mut ModifierSpec::ALL.iter().map(|m| m.0))),
    ]
}

/// The text of a theme file: `header`, `extends = "<base>"` and every token of `th` under its
/// description, commented out when `commented`.
fn file_text(header: &str, base: &str, th: &Theme, commented: bool) -> Result<String, AuthorError> {
    let mut out = header.to_string();
    for l in format_lines() {
        out.push_str(&format!("# {}\n", l.replace('`', "")));
    }
    out.push_str("# Every token is described in docs/themes.md of datarig's repository.\n");
    out.push_str(&format!("extends = \"{base}\"\n\n[colors]\n"));
    let off = if commented { "# " } else { "" };
    for (token, about) in COLOR_ABOUT {
        let value = color_of(th, token).and_then(color_text).ok_or(AuthorError::Unwritable { token })?;
        out.push_str(&format!("\n# {about}\n{off}{token} = {value}\n"));
    }
    out.push_str("\n[styles]\n");
    for (token, about) in STYLE_ABOUT {
        let value = style_of(th, token).and_then(style_text).ok_or(AuthorError::Unwritable { token })?;
        out.push_str(&format!("\n# {about}\n{off}{token} = {value}\n"));
    }
    Ok(out)
}

/// A color as a theme file writes it (quoted); `None` for one it cannot hold.
fn color_text(c: Color) -> Option<String> {
    let text = match c {
        Color::Reset => "default".to_string(),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        c => ANSI_NAMES[ANSI.iter().position(|a| *a == c)?].to_string(),
    };
    Some(format!("\"{text}\""))
}

/// A style as a theme file writes it (an inline table); `None` for one it cannot hold (a
/// removed modifier, an underline color, a modifier with no name).
fn style_text(s: Style) -> Option<String> {
    if !s.sub_modifier.is_empty() || s.underline_color.is_some() {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(fg) = s.fg {
        parts.push(format!("fg = {}", color_text(fg)?));
    }
    if let Some(bg) = s.bg {
        parts.push(format!("bg = {}", color_text(bg)?));
    }
    let named: Vec<&str> =
        ModifierSpec::ALL.iter().filter(|(_, m)| s.add_modifier.contains(modifier(*m))).map(|(n, _)| *n).collect();
    let all = ModifierSpec::ALL.iter().fold(Modifier::empty(), |acc, (_, m)| acc | modifier(*m));
    if !all.contains(s.add_modifier) {
        return None;
    }
    if !named.is_empty() {
        let quoted: Vec<String> = named.iter().map(|n| format!("\"{n}\"")).collect();
        parts.push(format!("modifiers = [{}]", quoted.join(", ")));
    }
    Some(if parts.is_empty() { "{}".to_string() } else { format!("{{ {} }}", parts.join(", ")) })
}

/// The small theme file `docs/themes.md` shows (a test reads it as a theme file).
pub const DOC_EXAMPLE: &str = "\
extends = \"dark\"

[colors]
accent = \"#89b4fa\"
warning = \"yellow\"

[styles]
selection = { bg = \"#45475a\", modifiers = [\"bold\"] }
syn_comment = { fg = \"bright-black\", modifiers = [\"italic\"] }
";

/// `docs/themes.md`: the file format, the commands that write theme files, and every token with
/// its description.
pub fn doc() -> String {
    let mut s = String::new();
    s.push_str("# datarig themes\n\n");
    s.push_str(
        "<!-- Generated from crates/tui/src/theme.rs and theme/authoring.rs. Do not edit;\n     \
         run `DATARIG_BLESS=1 cargo test -p datarig-tui --test themes_doc` at the repository root. -->\n\n",
    );
    s.push_str(
        "The setting `theme` (`:set theme=<name>`, or the settings screen, `Space ,`) names a built-in theme \
         or a theme file. A theme file is `themes/<name>.toml` next to the config file \
         (`$XDG_CONFIG_HOME/datarig/themes/`, else `~/.config/datarig/themes/`); its name is ASCII letters, \
         digits, `-` and `_` (at most 64), and never a built-in theme's.\n\n",
    );
    s.push_str("Built-in themes: ");
    s.push_str(&NAMES.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", "));
    s.push_str(".\n\n## The file\n\n");
    s.push_str(
        "`extends` names the built-in theme the file starts from (`terminal` without it). `[colors]` sets color \
         tokens and `[styles]` style tokens; a token the file does not set keeps the value of the theme it \
         extends, and a style it sets replaces that one whole. The dimming of the screen behind a dialog is \
         not a token: it is the extended theme's. An unknown token, color or modifier is an error with its \
         line, and the app then draws with `terminal` and says why.\n\n",
    );
    for l in format_lines() {
        s.push_str(&format!("- {l}\n"));
    }
    s.push_str("\n```toml\n# themes/mine.toml, used with theme = \"mine\"\n");
    s.push_str(DOC_EXAMPLE);
    s.push_str("```\n\n## Writing a theme file\n\n");
    s.push_str(
        "- `:theme copy <from> <name>` writes `themes/<name>.toml` with every token of the theme `<from>` (a \
         built-in theme or a theme file; a family such as `catppuccin` as the variant drawn now) set to its \
         value, each under a comment saying what it colors. `extends` names the built-in theme `<from>` \
         starts from, for the dimming behind dialogs; every token is in the file, so it stands on its own.\n\
         - `:theme new <name>` writes `themes/<name>.toml` extending the current theme when it is built in, \
         else `dark`, with every token commented out and showing the value it takes from there: remove the \
         `# ` in front of a key to set it.\n\
         - On the settings screen, `c` on the theme row copies the theme shown there (the one being \
         previewed, else the current one) and asks for the new name.\n\n\
         Each writes a new file and never replaces one: a name that is taken, that is not a valid file name, \
         that is a built-in theme's or a Windows device name such as `nul` (ignoring case) is refused. The themes folder is created when it is \
         missing. Then datarig draws with the new theme, saves `theme = \"<name>\"` to the config file and \
         shows the file's path.\n\n",
    );
    let table = |s: &mut String, title: &str, kind: &str, rows: &[(&str, &str)]| {
        s.push_str(&format!("## {title}\n\n| Token | {kind} |\n|---|---|\n"));
        for (token, about) in rows {
            s.push_str(&format!("| `{token}` | {about} |\n"));
        }
        s.push('\n');
    };
    table(&mut s, "Colors (`[colors]`)", "What it colors", COLOR_ABOUT);
    table(&mut s, "Styles (`[styles]`)", "What it styles", STYLE_ABOUT);
    s.pop();
    s
}

#[cfg(test)]
mod tests;
