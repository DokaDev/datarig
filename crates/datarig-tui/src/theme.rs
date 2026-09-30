//! The theme: the color and style tokens every widget draws with.
//!
//! A [`Theme`] is a value; the built-ins are listed in [`BUILTINS`] (`dark`, 24-bit truecolor).
//! Widgets read the current theme once per render with [`cur`]. It is thread-local:
//! [`crate::screens::draw`] sets it from `App::theme` for the duration of a frame
//! ([`scope`]), and a thread that never set one (a test rendering a widget, a helper) gets
//! [`DARK`]. Tests run on parallel threads, so a theme set by one never reaches another.
//!
//! Profile colors ([`PROFILE_COLORS`], [`profile_color`]) are not part of a theme: a profile
//! keeps its color whatever the theme.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use std::cell::RefCell;
use std::sync::Arc;

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
    /// `[editor] mode = "standard"`: no modes to tell apart.
    pub mode_neutral: Color,
    pub mode_fg: Color,
    /// A match of a search in the editor or the grid (not drawn yet).
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
    mode_neutral: rgb(0x8A93A6),
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
pub const BUILTINS: &[(&str, &Theme)] = &[("dark", &DARK)];

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
