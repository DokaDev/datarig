//! The theme: the color and style tokens every widget draws with.
//!
//! A [`Theme`] is a value. The built-ins are listed in [`BUILTINS`]: `terminal` (the default: the
//! terminal's own 16 colors), `dark` (24-bit truecolor), `light`, `high-contrast` and the
//! palettes of Catppuccin, Tokyo Night, Gruvbox, Nord and Dracula. The name of a family with a
//! light and a dark variant ([`FAMILIES`]: `catppuccin`, `tokyo-night`, `gruvbox`) follows the
//! terminal's background ([`Background`], asked once at startup); a variant's own name pins it.
//! A user theme is a file `themes/<name>.toml` next to the config file
//! ([`datarig_core::theme`]), a built-in theme with some tokens replaced ([`resolve`]).
//!
//! Widgets read the current theme once per render with [`cur`]. It is thread-local:
//! [`crate::screens::draw`] sets it from `App::theme` for the duration of a frame
//! ([`scope`]), and a thread that never set one (a test rendering a widget, a helper) gets
//! [`DARK`]. Tests run on parallel threads, so a theme set by one never reaches another.
//!
//! Profile colors ([`PROFILE_COLORS`], [`profile_color`]) are not part of a theme: a profile
//! keeps its color whatever the theme.

use datarig_core::theme::{ColorSpec, ModifierSpec, Problem, StyleSpec, ThemeError, ThemeSpec, Tokens};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use std::cell::RefCell;
use std::path::Path;
use std::sync::Arc;

mod builtins;
pub use builtins::*;

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// How the screen behind a dialog is dimmed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dim {
    /// Blend every RGB color toward `toward`, keeping `keep` percent of it.
    Blend { toward: Color, keep: u16 },
    /// Add [`Modifier::DIM`] and keep the colors (for themes of terminal colors).
    Modifier,
}

/// The tokens of a theme. Plain colors are roles a widget combines itself; a token that a
/// theme without RGB may need to show with a modifier (a selection as reversed text, a mark
/// in bold) is a [`Style`] and is applied with [`Style::patch`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    pub bg: Color,
    pub surface: Color,
    pub surface_alt: Color,
    pub border: Color,
    pub accent: Color,
    pub accent_warm: Color,
    pub fg: Color,
    pub fg_muted: Color,
    pub fg_dim: Color,
    /// The selected row or item, and the grid's cursor cell.
    pub selection: Style,
    /// Cells of a selected range in the grid (the cursor's cell keeps [`Self::selection`]).
    pub range: Style,
    /// The line or row under the cursor where the pane is not focused, and the editor's line.
    pub cursor_line: Style,
    /// A faint tint behind the statement a run would take.
    pub current_stmt: Style,
    /// The bar in the editor's gutter next to the statement a run would take (or the
    /// selection): subtle, but it stays visible where the tint does not (256 colors).
    pub current_stmt_bar: Color,
    /// A tint behind the statement that runs now, apart from [`Self::current_stmt`] (the
    /// built-ins blend 12% of [`Self::accent_warm`] into the background).
    pub running_stmt: Style,
    /// The bar and the spinner in the editor's gutter next to the statement that runs now.
    pub running_stmt_bar: Color,
    /// The hint after a statement's last line that says what its last run did: dim, never
    /// read as text of the statement.
    pub run_hint: Style,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub null_fg: Color,
    pub syn_keyword: Style,
    pub syn_function: Style,
    pub syn_string: Style,
    pub syn_number: Style,
    pub syn_comment: Style,
    pub syn_operator: Style,
    pub syn_identifier: Style,
    pub syn_quoted_ident: Style,
    /// Key column marks: primary key, foreign key, unique key. Each reads on the background
    /// and the grid's header surface and stays apart from the other two, also in 256 colors.
    pub key_pk: Color,
    pub key_fk: Color,
    pub key_uq: Color,
    /// The mode badge at the left of the status bar (lualine style): a background per mode
    /// with [`Self::mode_fg`] bold on it. Each reads at 4.5:1 or more and stays apart from the
    /// others and from the status bar, also in 256 colors (theme tests).
    pub mode_normal: Color,
    pub mode_insert: Color,
    pub mode_visual: Color,
    /// While the `:` command line is open.
    pub mode_command: Color,
    pub mode_fg: Color,
    /// A match of the editor's search (`/`, `n`, `*`, …) on the lines on screen.
    pub search_match: Style,
    /// The bracket matching the one at the editor's cursor (not drawn yet).
    pub match_paren: Style,
    /// The mark of a read-only connection or session (not drawn yet).
    pub read_only_mark: Style,
    /// The mark of something that changes or deletes data, or of a production profile (not
    /// drawn yet).
    pub danger_mark: Style,
    /// The node of a query plan that takes the most time (not drawn yet).
    pub plan_hot: Style,
    /// A plan node whose estimated rows are far from the actual rows (not drawn yet).
    pub plan_misestimate: Style,
    /// How the screen behind a dialog is dimmed.
    pub dim: Dim,
}

/// The dark truecolor theme.
pub const DARK: Theme = Theme {
    bg: rgb(0x14161B),
    surface: rgb(0x1B1E25),
    surface_alt: rgb(0x20242C),
    border: rgb(0x2E3440),
    accent: rgb(0x4FB3A9),
    accent_warm: rgb(0xE0A96D),
    fg: rgb(0xD8DEE9),
    fg_muted: rgb(0x8A93A6),
    fg_dim: rgb(0x5C6577),
    selection: Style::new().bg(rgb(0x2A3B4D)),
    range: Style::new().bg(rgb(0x1E3438)),
    cursor_line: Style::new().bg(rgb(0x1F2530)),
    current_stmt: Style::new().bg(rgb(0x1A2129)),
    current_stmt_bar: rgb(0x3F8F87),
    running_stmt: Style::new().bg(rgb(0x2C2825)),
    running_stmt_bar: rgb(0xE0A96D),
    run_hint: Style::new().fg(rgb(0x8A93A6)).add_modifier(Modifier::ITALIC),
    success: rgb(0x8FC77A),
    warning: rgb(0xE6C35C),
    error: rgb(0xE06C75),
    null_fg: rgb(0x6B7280),
    syn_keyword: Style::new().fg(rgb(0xC792EA)).add_modifier(Modifier::BOLD),
    syn_function: Style::new().fg(rgb(0x6FA8DC)),
    syn_string: Style::new().fg(rgb(0x9CCB85)),
    syn_number: Style::new().fg(rgb(0xE0A96D)),
    syn_comment: Style::new().fg(rgb(0x5C6577)).add_modifier(Modifier::ITALIC),
    syn_operator: Style::new().fg(rgb(0x8A93A6)),
    syn_identifier: Style::new().fg(rgb(0xD8DEE9)),
    syn_quoted_ident: Style::new().fg(rgb(0x4FB3A9)),
    key_pk: rgb(0xE5C07B),
    key_fk: rgb(0x61AFEF),
    key_uq: rgb(0xC678DD),
    mode_normal: rgb(0x61AFEF),
    mode_insert: rgb(0x98C379),
    mode_visual: rgb(0xC678DD),
    mode_command: rgb(0xE5935A),
    mode_fg: rgb(0x14161B),
    // The accent-warm of numbers under the background's text: loud, as a search hit is.
    search_match: Style::new().fg(rgb(0x14161B)).bg(rgb(0xE0A96D)),
    // The border tone as a background, lighter than the cursor line, in bold.
    match_paren: Style::new().bg(rgb(0x2E3440)).add_modifier(Modifier::BOLD),
    // Accent, bold: a state to notice, not a warning.
    read_only_mark: Style::new().fg(rgb(0x4FB3A9)).add_modifier(Modifier::BOLD),
    danger_mark: Style::new().fg(rgb(0xE06C75)).add_modifier(Modifier::BOLD),
    plan_hot: Style::new().fg(rgb(0xE0A96D)).add_modifier(Modifier::BOLD),
    plan_misestimate: Style::new().fg(rgb(0xE6C35C)).add_modifier(Modifier::UNDERLINED),
    // 55% of a color stays. Dimmed body text keeps a contrast of about 4.5:1, and every
    // dimmed token still maps to a different xterm-256 color than the dimmed background.
    dim: Dim::Blend { toward: rgb(0x0B0C10), keep: 55 },
};

/// The built-in themes by name.
pub const BUILTINS: &[(&str, &Theme)] = &[
    ("terminal", &TERMINAL),
    ("dark", &DARK),
    ("light", &LIGHT),
    ("high-contrast", &HIGH_CONTRAST),
    ("catppuccin-latte", &CATPPUCCIN_LATTE),
    ("catppuccin-mocha", &CATPPUCCIN_MOCHA),
    ("tokyo-night-day", &TOKYO_NIGHT_DAY),
    ("tokyo-night-night", &TOKYO_NIGHT_NIGHT),
    ("gruvbox-light", &GRUVBOX_LIGHT),
    ("gruvbox-dark", &GRUVBOX_DARK),
    ("nord", &NORD),
    ("dracula", &DRACULA),
];

/// A theme with a light and a dark variant: its own name picks the one that fits the terminal's
/// background (the dark one when that is not known).
#[derive(Clone, Copy, Debug)]
pub struct Family {
    pub name: &'static str,
    pub light: &'static str,
    pub dark: &'static str,
}

pub const FAMILIES: &[Family] = &[
    Family { name: "catppuccin", light: "catppuccin-latte", dark: "catppuccin-mocha" },
    Family { name: "tokyo-night", light: "tokyo-night-day", dark: "tokyo-night-night" },
    Family { name: "gruvbox", light: "gruvbox-light", dark: "gruvbox-dark" },
];

/// Every built-in name `theme` takes, in the order the settings screen and the completion list
/// them (a family before its variants).
pub const NAMES: &[&str] = &[
    "terminal",
    "dark",
    "light",
    "high-contrast",
    "catppuccin",
    "catppuccin-latte",
    "catppuccin-mocha",
    "tokyo-night",
    "tokyo-night-day",
    "tokyo-night-night",
    "gruvbox",
    "gruvbox-light",
    "gruvbox-dark",
    "nord",
    "dracula",
];

/// The terminal's background, as its answer to the OSC 11 query at startup says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Background {
    Light,
    Dark,
    /// No answer (or not asked: not a terminal).
    #[default]
    Unknown,
}

/// The built-in theme `name` (a family's variant for `background`).
pub fn builtin(name: &str, background: Background) -> Option<&'static Theme> {
    let name = match FAMILIES.iter().find(|f| f.name == name) {
        Some(f) if background == Background::Light => f.light,
        Some(f) => f.dark,
        None => name,
    };
    BUILTINS.iter().find(|(n, _)| *n == name).map(|(_, th)| *th)
}

/// Every theme name `theme` takes: the built-in ones, then the theme files of `dir` (a file
/// with a built-in name is left out: it is never used).
pub fn names(dir: Option<&Path>) -> Vec<String> {
    let mut out: Vec<String> = NAMES.iter().map(|n| n.to_string()).collect();
    if let Some(dir) = dir {
        out.extend(datarig_core::theme::names(dir).into_iter().filter(|n| !NAMES.contains(&n.as_str())));
    }
    out
}

/// The theme `name`: a built-in one, else the file `<dir>/<name>.toml`. A file with a built-in
/// name is an error (it would silently never be used).
pub fn resolve(name: &str, background: Background, dir: Option<&Path>) -> Result<Theme, ThemeError> {
    if let Some(th) = builtin(name, background) {
        if let Some(path) = dir.map(|d| datarig_core::theme::path(d, name)).filter(|p| p.is_file()) {
            return Err(ThemeError::Shadows { path });
        }
        return Ok(th.clone());
    }
    let Some(dir) = dir else { return Err(ThemeError::Unknown { name: name.to_string(), dir: None }) };
    let spec = datarig_core::theme::load(dir, name, TOKENS)?;
    let base = match &spec.extends {
        None => &TERMINAL,
        Some((base, line)) => builtin(base, background).ok_or_else(|| ThemeError::File {
            path: datarig_core::theme::path(dir, name),
            line: Some(*line),
            problem: Problem::UnknownBase(base.clone()),
        })?,
    };
    Ok(from_spec(&spec, base.clone()))
}

/// `base` with the tokens of a theme file.
pub fn from_spec(spec: &ThemeSpec, mut base: Theme) -> Theme {
    for (name, c) in &spec.colors {
        if let Some(slot) = color_token(&mut base, name) {
            *slot = color(*c);
        }
    }
    for (name, s) in &spec.styles {
        if let Some(slot) = style_token(&mut base, name) {
            *slot = style(s);
        }
    }
    base
}

/// The color of a theme file's color.
pub fn color(c: ColorSpec) -> Color {
    const ANSI: [Color; 16] = [
        Color::Black,
        Color::Red,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::Magenta,
        Color::Cyan,
        Color::Gray,
        Color::DarkGray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightYellow,
        Color::LightBlue,
        Color::LightMagenta,
        Color::LightCyan,
        Color::White,
    ];
    match c {
        ColorSpec::Default => Color::Reset,
        ColorSpec::Ansi(i) => ANSI[usize::from(i) % ANSI.len()],
        ColorSpec::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn style(s: &StyleSpec) -> Style {
    let mut out = Style::new();
    out.fg = s.fg.map(color);
    out.bg = s.bg.map(color);
    for m in &s.modifiers {
        out = out.add_modifier(match m {
            ModifierSpec::Bold => Modifier::BOLD,
            ModifierSpec::Dim => Modifier::DIM,
            ModifierSpec::Italic => Modifier::ITALIC,
            ModifierSpec::Underlined => Modifier::UNDERLINED,
            ModifierSpec::Reversed => Modifier::REVERSED,
            ModifierSpec::CrossedOut => Modifier::CROSSED_OUT,
        });
    }
    out
}

/// The tokens a theme file sets by name: the color and the style fields of [`Theme`] (`dim` is
/// not one; a theme file keeps the dimming of the theme it extends).
macro_rules! tokens {
    (colors: $($c:ident),* ; styles: $($s:ident),* $(,)?) => {
        pub const COLOR_TOKENS: &[&str] = &[$(stringify!($c)),*];
        pub const STYLE_TOKENS: &[&str] = &[$(stringify!($s)),*];

        fn color_token<'a>(th: &'a mut Theme, name: &str) -> Option<&'a mut Color> {
            match name {
                $(stringify!($c) => Some(&mut th.$c),)*
                _ => None,
            }
        }

        fn style_token<'a>(th: &'a mut Theme, name: &str) -> Option<&'a mut Style> {
            match name {
                $(stringify!($s) => Some(&mut th.$s),)*
                _ => None,
            }
        }
    };
}

tokens! {
    colors: bg, surface, surface_alt, border, accent, accent_warm, fg, fg_muted, fg_dim, current_stmt_bar, running_stmt_bar,
        success,
        warning, error, null_fg, key_pk, key_fk, key_uq, mode_normal, mode_insert, mode_visual, mode_command,
        mode_fg;
    styles: selection, range, cursor_line, current_stmt, running_stmt, run_hint, syn_keyword, syn_function, syn_string, syn_number,
        syn_comment, syn_operator, syn_identifier, syn_quoted_ident, search_match, match_paren, read_only_mark,
        danger_mark, plan_hot, plan_misestimate,
}

/// The token names of a theme file.
pub const TOKENS: Tokens = Tokens { colors: COLOR_TOKENS, styles: STYLE_TOKENS };

thread_local! {
    static CURRENT: RefCell<Arc<Theme>> = RefCell::new(Arc::new(DARK));
}

/// The theme of this thread: the one of the frame being drawn, else [`DARK`].
pub fn cur() -> Arc<Theme> {
    CURRENT.with(|c| c.borrow().clone())
}

/// Makes `theme` this thread's current theme until the returned guard drops (the previous one
/// comes back).
pub fn scope(theme: Arc<Theme>) -> Scope {
    Scope(Some(CURRENT.with(|c| c.replace(theme))))
}

/// Restores the previous current theme when dropped; see [`scope`].
#[must_use]
pub struct Scope(Option<Arc<Theme>>);

impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(prev) = self.0.take() {
            let _ = CURRENT.try_with(|c| *c.borrow_mut() = prev);
        }
    }
}

impl Theme {
    pub fn base(&self) -> Style {
        Style::new().fg(self.fg).bg(self.bg)
    }

    /// The color of a key column mark.
    pub fn key_color(&self, k: crate::icons::KeyMark) -> Color {
        match k {
            crate::icons::KeyMark::Pk => self.key_pk,
            crate::icons::KeyMark::Fk => self.key_fk,
            crate::icons::KeyMark::Uq => self.key_uq,
        }
    }

    pub fn syntax(&self, kind: datarig_core::sql::lexer::Tok, is_function: bool) -> Style {
        use datarig_core::sql::lexer::Tok;
        if is_function {
            return self.syn_function;
        }
        match kind {
            Tok::Keyword => self.syn_keyword,
            Tok::Str | Tok::Dollar => self.syn_string,
            Tok::Number => self.syn_number,
            Tok::LineComment | Tok::BlockComment => self.syn_comment,
            Tok::QuotedIdent => self.syn_quoted_ident,
            Tok::Ident | Tok::Param => self.syn_identifier,
            Tok::Op | Tok::Semi | Tok::Dot | Tok::Comma | Tok::LParen | Tok::RParen => self.syn_operator,
            Tok::Whitespace => Style::new().fg(self.fg),
        }
    }

    /// `c` blended toward the dim tone; a terminal default color (not RGB) is treated as
    /// `default` first. Unchanged when the theme dims with a modifier.
    fn dimmed(&self, c: Color, default: Color) -> Color {
        let Dim::Blend { toward, keep } = self.dim else { return c };
        let rgb = |c: Color| if let Color::Rgb(r, g, b) = c { Some((r, g, b)) } else { None };
        let (Some((r, g, b)), Some((br, bg, bb))) = (rgb(c).or(rgb(default)), rgb(toward)) else { return c };
        let mix = |a: u8, z: u8| ((u16::from(a) * keep + u16::from(z) * (100 - keep)) / 100) as u8;
        Color::Rgb(mix(r, br), mix(g, bg), mix(b, bb))
    }

    /// A text color of the screen behind a dialog.
    pub fn dim_fg(&self, c: Color) -> Color {
        self.dimmed(c, self.fg)
    }

    /// A background color of the screen behind a dialog.
    pub fn dim_bg(&self, c: Color) -> Color {
        self.dimmed(c, self.bg)
    }

    /// Dim what is drawn in `area` so a dialog drawn on top of it stands out while the screen
    /// below stays readable. Symbols are kept.
    pub fn dim_area(&self, buf: &mut Buffer, area: Rect) {
        let area = area.intersection(buf.area);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                let cell = &mut buf[(x, y)];
                match self.dim {
                    Dim::Blend { .. } => {
                        cell.fg = self.dim_fg(cell.fg);
                        cell.bg = self.dim_bg(cell.bg);
                    }
                    Dim::Modifier => cell.modifier.insert(Modifier::DIM),
                }
            }
        }
    }
}

/// RGB of the 12 named profile colors, in the order of
/// [`datarig_core::profile::color::NAMES`]. Chosen to read on the dark background and to stay
/// apart from each other.
pub const PROFILE_COLORS: [Color; 12] = [
    rgb(0xE06C75), // red
    rgb(0xE5935A), // orange
    rgb(0xE6C35C), // yellow
    rgb(0x8FC77A), // green
    rgb(0x4FB3A9), // teal
    rgb(0x5CC6D9), // cyan
    rgb(0x5B8DEF), // blue
    rgb(0x8C8CF0), // indigo
    rgb(0xC792EA), // purple
    rgb(0xF08CC0), // pink
    rgb(0xB8906A), // brown
    rgb(0x9AA3B5), // gray
];

/// The color of a profile (its dot, tab label and status bar name).
pub fn profile_color(c: datarig_core::profile::color::ProfileColor) -> Color {
    use datarig_core::profile::color::ProfileColor;
    match c {
        ProfileColor::Named(i) => PROFILE_COLORS[usize::from(i) % PROFILE_COLORS.len()],
        ProfileColor::Hex(r, g, b) => Color::Rgb(r, g, b),
    }
}

#[cfg(test)]
mod tests;
