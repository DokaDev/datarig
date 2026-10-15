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

mod authoring;
mod builtins;
pub use authoring::*;
pub use builtins::*;

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const WHITE: Color = Color::Rgb(0xFF, 0xFF, 0xFF);
const BLACK: Color = Color::Rgb(0, 0, 0);

/// `a` with `keep` percent of it kept and the rest of `b`; `a` when either is not RGB.
fn blend(a: Color, b: Color, keep: u16) -> Color {
    let (Color::Rgb(r, g, bl), Color::Rgb(r2, g2, b2)) = (a, b) else { return a };
    let mix = |x: u8, y: u8| ((u16::from(x) * keep + u16::from(y) * (100 - keep)) / 100) as u8;
    Color::Rgb(mix(r, r2), mix(g, g2), mix(bl, b2))
}

/// The WCAG contrast ratio of two RGB colors (1 when either is not RGB).
pub fn contrast_ratio(a: Color, b: Color) -> f64 {
    let lum = |c: Color| {
        let Color::Rgb(r, g, b) = c else { return None };
        let ch = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        Some(0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b))
    };
    let (Some(x), Some(y)) = (lum(a), lum(b)) else { return 1.0 };
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
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
    /// What is under the pointer (a list row, a grid cell, an inspector entry, a tab, a button):
    /// a subtle background of its own, apart from the background, both surfaces, the cursor line,
    /// the selection (focused or not) and a selected range; never an underline.
    pub hover: Style,
    /// Cells a copy took from the result grid, for a moment after the copy reached the
    /// clipboard: a background (with its own text color) that stands apart from every other
    /// background of the grid; never an underline.
    pub yank_flash: Style,
    /// The active tab: a fill (its bg) that stands clearly apart from the bar's background, both
    /// surfaces, the hover, the selections and the copied cells' flash. Without an fg the tab's
    /// colors stay, moved where they would not read on it ([`Self::tab_active_bg`],
    /// [`Self::readable_on`]); an fg replaces every text color on it but the environment's
    /// badge (the terminal theme's, whose colors cannot be checked).
    pub tab_active: Style,
    /// A faint tint behind the statement a run would take.
    pub current_stmt: Style,
    /// The bar in the editor's gutter next to the statement a run would take (or the
    /// selection): subtle, but it stays visible where the tint does not (256 colors).
    pub current_stmt_bar: Color,
    /// A tint behind the statement that runs now, apart from [`Self::current_stmt`] (the
    /// built-ins blend 12% of [`Self::accent_warm`] into the background; `terminal` makes it bold).
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
    /// The opening and closing of an executable comment (MySQL's `/*!` … `*/`), whose inside
    /// is code: set apart from comments, as the server runs it.
    pub syn_exec_comment: Style,
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
    /// The mark of read-only rules: the lock on a profile's explorer row and its tabs, and the
    /// words "read-only" of an environment's rules. Blue in the built-ins (the theme's foreign
    /// key color, which reads on the background and both surfaces): information, not a warning.
    pub read_only_mark: Style,
    /// The mark of something that changes or deletes data, or of a production profile (not
    /// drawn yet).
    pub danger_mark: Style,
    /// The nodes of a query plan that take the most time.
    pub plan_hot: Style,
    /// How far a plan node's estimated rows are from the actual rows.
    pub plan_misestimate: Style,
    /// The estimates of an object among the largest of its schema in the explorer (1 MiB or
    /// more): a subtle warning, amber. With five levels (`[explorer] size_heat = "5"`) the
    /// tier of 100 MiB or more.
    pub size_warm: Style,
    /// The same for one of 1 GiB or more: red. With five levels the tier of 10 GiB or more.
    pub size_hot: Style,
    /// With five levels, the tier of 1 MiB or more: yellow.
    pub size_mild: Style,
    /// With five levels, the tier of 1 GiB or more: orange, between amber and red.
    pub size_heated: Style,
    /// The colors of a chart's series, in turn. Each reads on the background and stays apart
    /// from the others, also in 256 colors (theme tests).
    pub chart_1: Color,
    pub chart_2: Color,
    pub chart_3: Color,
    pub chart_4: Color,
    pub chart_5: Color,
    pub chart_6: Color,
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
    hover: Style::new().bg(rgb(0x2D3035)),
    yank_flash: Style::new().fg(rgb(0xD8DEE9)).bg(rgb(0x514233)),
    // 34% of the accent blended into the background.
    tab_active: Style::new().bg(rgb(0x284B4B)),
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
    syn_exec_comment: Style::new().fg(rgb(0xE6C35C)).add_modifier(Modifier::BOLD),
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
    // Blue (the foreign key's), bold: a state to notice, not a warning.
    read_only_mark: Style::new().fg(rgb(0x61AFEF)).add_modifier(Modifier::BOLD),
    danger_mark: Style::new().fg(rgb(0xE06C75)).add_modifier(Modifier::BOLD),
    plan_hot: Style::new().fg(rgb(0xE0A96D)).add_modifier(Modifier::BOLD),
    plan_misestimate: Style::new().fg(rgb(0xE6C35C)).add_modifier(Modifier::UNDERLINED),
    size_warm: Style::new().fg(rgb(0xE0A96D)),
    size_hot: Style::new().fg(rgb(0xE06C75)),
    size_mild: Style::new().fg(rgb(0xE8D65E)),
    size_heated: Style::new().fg(rgb(0xD86546)),
    chart_1: rgb(0x61AFEF),
    chart_2: rgb(0xE0A96D),
    chart_3: rgb(0x8FC77A),
    chart_4: rgb(0xC678DD),
    chart_5: rgb(0x4FB3A9),
    chart_6: rgb(0xE06C75),
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
    builtin_variant(name, background).map(|(_, th)| th)
}

/// [`builtin`] with the name of the variant it is (never a family's name).
fn builtin_variant(name: &str, background: Background) -> Option<(&'static str, &'static Theme)> {
    let name = match FAMILIES.iter().find(|f| f.name == name) {
        Some(f) if background == Background::Light => f.light,
        Some(f) => f.dark,
        None => name,
    };
    BUILTINS.iter().find(|(n, _)| *n == name).copied()
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
    resolve_with_base(name, background, dir).map(|(th, _)| th)
}

/// [`resolve`], with the built-in variant the theme starts from: itself when it is built in, else
/// its file's `extends` (`terminal` without one), from the one read of the file.
pub fn resolve_with_base(
    name: &str,
    background: Background,
    dir: Option<&Path>,
) -> Result<(Theme, &'static str), ThemeError> {
    if let Some((base, th)) = builtin_variant(name, background) {
        if let Some(path) = dir.map(|d| datarig_core::theme::path(d, name)).filter(|p| p.is_file()) {
            return Err(ThemeError::Shadows { path });
        }
        return Ok((th.clone(), base));
    }
    let Some(dir) = dir else { return Err(ThemeError::Unknown { name: name.to_string(), dir: None }) };
    let spec = datarig_core::theme::load(dir, name, TOKENS)?;
    let (base_name, base) = match &spec.extends {
        None => ("terminal", &TERMINAL),
        Some((base, line)) => builtin_variant(base, background).ok_or_else(|| ThemeError::File {
            path: datarig_core::theme::path(dir, name),
            line: Some(*line),
            problem: Problem::UnknownBase(base.clone()),
        })?,
    };
    Ok((from_spec(&spec, base.clone()), base_name))
}

/// `base` with the tokens of a theme file.
pub fn from_spec(spec: &ThemeSpec, mut base: Theme) -> Theme {
    // A file written before `tab_active` existed (or leaving it out) with its own background or
    // accent: a fill of those, not the one made for the theme it extends.
    let derive = !spec.styles.iter().any(|(n, _)| n == "tab_active")
        && spec.colors.iter().any(|(n, _)| n == "bg" || n == "accent");
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
    if derive {
        base.tab_active = base.tab_active_derived();
    }
    base
}

/// The colors of the 16 ANSI names of a theme file, in [`datarig_core::theme::ANSI_NAMES`] order.
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

/// The color of a theme file's color.
pub fn color(c: ColorSpec) -> Color {
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
        out = out.add_modifier(modifier(*m));
    }
    out
}

/// The modifier of a theme file's modifier.
fn modifier(m: ModifierSpec) -> Modifier {
    match m {
        ModifierSpec::Bold => Modifier::BOLD,
        ModifierSpec::Dim => Modifier::DIM,
        ModifierSpec::Italic => Modifier::ITALIC,
        ModifierSpec::Underlined => Modifier::UNDERLINED,
        ModifierSpec::Reversed => Modifier::REVERSED,
        ModifierSpec::CrossedOut => Modifier::CROSSED_OUT,
    }
}

/// The tokens a theme file sets by name: the color and the style fields of [`Theme`] (`dim` is
/// not one; a theme file keeps the dimming of the theme it extends), each with what it colors
/// and where. The descriptions are the comments of the files `:theme copy` and `:theme new`
/// write and the tables of `docs/themes.md`, so a token cannot be added without one.
macro_rules! tokens {
    (colors: $($c:ident => $cd:literal),* ; styles: $($s:ident => $sd:literal),* $(,)?) => {
        pub const COLOR_TOKENS: &[&str] = &[$(stringify!($c)),*];
        pub const STYLE_TOKENS: &[&str] = &[$(stringify!($s)),*];

        /// Every color token and what it colors, in [`COLOR_TOKENS`] order.
        pub const COLOR_ABOUT: &[(&str, &str)] = &[$((stringify!($c), $cd)),*];
        /// Every style token and what it styles, in [`STYLE_TOKENS`] order.
        pub const STYLE_ABOUT: &[(&str, &str)] = &[$((stringify!($s), $sd)),*];

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

        /// The color token `name` of `th`.
        pub fn color_of(th: &Theme, name: &str) -> Option<Color> {
            match name {
                $(stringify!($c) => Some(th.$c),)*
                _ => None,
            }
        }

        /// The style token `name` of `th`.
        pub fn style_of(th: &Theme, name: &str) -> Option<Style> {
            match name {
                $(stringify!($s) => Some(th.$s),)*
                _ => None,
            }
        }
    };
}

tokens! {
    colors:
        bg => "The background of the panes: explorer, editor, results.",
        surface => "The background of dialogs, popups, menus, the command line and the status bar.",
        surface_alt => "A second surface: every other row of the result grid, form fields and buttons.",
        border => "Borders and rules between panes and around dialogs.",
        accent => "The main accent: the focused pane's border, dialog borders, labels and names to notice.",
        accent_warm => "The warm accent: values on the settings screen, an open transaction, keys in menus.",
        fg => "Body text.",
        fg_muted => "Secondary text: descriptions, labels and hints.",
        fg_dim => "Faint text: placeholders, empty lists and notes such as a database's default.",
        current_stmt_bar => "The bar in the editor's gutter next to the statement a run would take.",
        running_stmt_bar => "The bar and the spinner in the editor's gutter next to the statement that runs now.",
        success => "Success: rows a run returned, a connected tunnel, a success message.",
        warning => "Warnings.",
        error => "Errors.",
        null_fg => "NULL in the result grid and the inspector.",
        key_pk => "The mark of a primary key column.",
        key_fk => "The mark of a foreign key column.",
        key_uq => "The mark of a unique key column.",
        mode_normal => "The background of the status bar's mode badge in Normal mode.",
        mode_insert => "The background of the mode badge in Insert mode.",
        mode_visual => "The background of the mode badge in Visual mode.",
        mode_command => "The background of the mode badge while the command line is open.",
        mode_fg => "The text of the mode badge (bold, on the mode's color).",
        chart_1 => "The first series of a chart.",
        chart_2 => "The second series of a chart.",
        chart_3 => "The third series of a chart.",
        chart_4 => "The fourth series of a chart.",
        chart_5 => "The fifth series of a chart.",
        chart_6 => "The sixth series of a chart.";
    styles:
        selection => "The selected row or item, and the grid's cursor cell.",
        range => "Cells and headers of a selected range in the result grid (the cursor's cell keeps selection).",
        cursor_line => "The row under the cursor of an unfocused list or plan, the editor's line, and (its background) the chart's cursor while focused.",
        hover => "What is under the mouse pointer: a list row, a grid cell, a tab, a button.",
        yank_flash => "Cells copied from the result grid, for a moment after the copy was sent to the clipboard (set fg and bg: it covers the cursor's cell too).",
        current_stmt => "A faint tint behind the statement a run would take.",
        running_stmt => "A tint behind the statement that runs now.",
        run_hint => "The hint after a statement's last line that says what its last run did.",
        syn_keyword => "SQL keywords.",
        syn_function => "SQL function names.",
        syn_string => "SQL strings and dollar-quoted bodies.",
        syn_number => "SQL numbers.",
        syn_comment => "SQL comments.",
        syn_operator => "SQL operators and punctuation.",
        syn_identifier => "SQL names, parameters and variables.",
        syn_quoted_ident => "SQL quoted names.",
        syn_exec_comment => "The opening and closing of a MySQL executable comment (/*! ... */).",
        search_match => "A match of the editor's search on the lines on screen.",
        match_paren => "The bracket matching the one at the editor's cursor (not drawn yet).",
        read_only_mark => "The mark of read-only rules (its fg): the lock on a profile's row and tabs, and the rule's words.",
        danger_mark => "The mark of something that changes or deletes data, or of a production profile (not drawn yet).",
        plan_hot => "The plan nodes that take the most time, in every view of a query plan and its summary.",
        plan_misestimate => "How far a plan node's estimated rows are from the actual ones, in the plan, its summary, detail and funnel.",
        size_warm => "Explorer size estimates of 1 MiB or more (with five levels: 100 MiB or more).",
        size_hot => "Explorer size estimates of 1 GiB or more (with five levels: 10 GiB or more).",
        size_mild => "With five size levels, explorer size estimates of 1 MiB or more.",
        size_heated => "With five size levels, explorer size estimates of 1 GiB or more.",
        tab_active => "The active tab (set bg: surface_alt without one). An fg replaces every text color on it but the environment's badge; without one they stay, made to read on it. Left out, a file that sets bg or accent gets the accent blended into its bg.",
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
    /// The active tab's background: [`Self::tab_active`]'s, else the alternate surface.
    pub fn tab_active_bg(&self) -> Color {
        self.tab_active.bg.unwrap_or(self.surface_alt)
    }

    /// A fill for the active tab made of this theme's colors: 30% of the accent blended into
    /// the background where both are RGB, else [`Self::tab_active`] as it is.
    pub fn tab_active_derived(&self) -> Style {
        match (self.accent, self.bg) {
            (Color::Rgb(..), Color::Rgb(..)) => Style::new().bg(blend(self.accent, self.bg, 30)),
            _ => self.tab_active,
        }
    }

    /// `c` as text on `back` at a contrast of `min` or more: blended toward white on a dark
    /// `back`, toward black on a light one, in steps of 5%. Unchanged where either is not RGB
    /// (the terminal's palette decides).
    pub fn readable_on(c: Color, back: Color, min: f64) -> Color {
        if !matches!((c, back), (Color::Rgb(..), Color::Rgb(..))) {
            return c;
        }
        let toward = if contrast_ratio(back, WHITE) > contrast_ratio(back, BLACK) { WHITE } else { BLACK };
        (0..=20)
            .map(|step| blend(c, toward, 100 - step * 5))
            .find(|m| contrast_ratio(*m, back) >= min)
            .unwrap_or(toward)
    }

    /// The colors of a chart's series, in turn.
    pub fn chart(&self) -> [Color; 6] {
        [self.chart_1, self.chart_2, self.chart_3, self.chart_4, self.chart_5, self.chart_6]
    }

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
            Tok::ExecComment => self.syn_exec_comment,
            Tok::Directive => self.syn_keyword,
            Tok::Ident | Tok::Param | Tok::Variable => self.syn_identifier,
            Tok::Op | Tok::Semi | Tok::Dot | Tok::Comma | Tok::LParen | Tok::RParen => self.syn_operator,
            Tok::Whitespace => Style::new().fg(self.fg),
        }
    }

    /// The selected row or cell of a pane without the focus: the selection's background with a
    /// fifth of it blended toward the pane's background (as plain a selection, a little
    /// dimmer than the focused one); the selection with [`Modifier::DIM`] where either is not
    /// RGB (the terminal theme's reversed text).
    pub fn selection_unfocused(&self) -> Style {
        match (self.selection.bg, self.bg) {
            (Some(sel @ Color::Rgb(..)), Color::Rgb(..)) => {
                Style { bg: Some(blend(sel, self.bg, 80)), ..self.selection }
            }
            _ => self.selection.add_modifier(Modifier::DIM),
        }
    }

    /// The text of a profile's name in its environment's color `env` (none: body text), made
    /// readable as text: blended toward white on a dark theme, toward black on a light one, in
    /// steps of 5%, until it reads at 4.5:1 on the background and both surfaces. Unchanged on a
    /// theme without RGB backgrounds.
    pub fn name_color(&self, env: Option<Color>) -> Color {
        let Some(c) = env else { return self.fg };
        let toward = match self.bg {
            Color::Rgb(..) if contrast_ratio(self.bg, WHITE) > contrast_ratio(self.bg, BLACK) => WHITE,
            Color::Rgb(..) => BLACK,
            _ => return c,
        };
        // The RGB backgrounds only (a user theme may leave a surface to the terminal).
        let backs: Vec<Color> =
            [self.bg, self.surface, self.surface_alt].into_iter().filter(|c| matches!(c, Color::Rgb(..))).collect();
        (0..=20)
            .map(|step| blend(c, toward, 100 - step * 5))
            .find(|m| backs.iter().all(|b| contrast_ratio(*m, *b) >= 4.5))
            .unwrap_or(toward)
    }

    /// `c` (a name's color) muted, for a profile that is not connected: blended toward the
    /// background, keeping 60% of it, more where that would read at less than 3:1 on the
    /// background or a surface. [`Modifier::DIM`] does it on a theme without RGB backgrounds.
    pub fn name_muted(&self, c: Color) -> Style {
        if !matches!((c, self.bg), (Color::Rgb(..), Color::Rgb(..))) {
            return Style::new().fg(c).add_modifier(Modifier::DIM);
        }
        let backs: Vec<Color> =
            [self.bg, self.surface, self.surface_alt].into_iter().filter(|c| matches!(c, Color::Rgb(..))).collect();
        let muted = (0..=8)
            .map(|step| blend(c, self.bg, 60 + step * 5))
            .find(|m| backs.iter().all(|b| contrast_ratio(*m, *b) >= 3.0))
            .unwrap_or(c);
        Style::new().fg(muted)
    }

    /// A profile's name: in its environment's color `env` (body text without one), bold while
    /// it is connected, muted while it is not ([`Self::name_color`], [`Self::name_muted`]).
    pub fn name_style(&self, env: Option<Color>, connected: bool) -> Style {
        let c = self.name_color(env);
        if connected { Style::new().fg(c).add_modifier(Modifier::BOLD) } else { self.name_muted(c) }
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

/// A chip of color `c` (an environment's badge): bold text in near-black or white, whichever has
/// the higher contrast on `c` (near-black on a terminal color, which the terminal's palette
/// decides). A color of middle lightness that reads at less than 4.5:1 with either is moved
/// away from the text (darker under white, lighter under near-black) in steps of 5% until it
/// does. Every place that draws a badge draws it with this, as it is (never patched with the
/// background of the row it is on).
pub fn chip(c: Color) -> Style {
    const DARK_TEXT: Color = Color::Rgb(0x16, 0x16, 0x1E);
    let (fg, away) =
        if contrast_ratio(WHITE, c) > contrast_ratio(DARK_TEXT, c) { (WHITE, BLACK) } else { (DARK_TEXT, WHITE) };
    let bg = match c {
        Color::Rgb(..) => {
            (0..=20).map(|step| blend(c, away, 100 - step * 5)).find(|b| contrast_ratio(fg, *b) >= 4.5).unwrap_or(away)
        }
        _ => c,
    };
    Style::new().fg(fg).bg(bg).add_modifier(Modifier::BOLD)
}

#[cfg(test)]
mod tests;
