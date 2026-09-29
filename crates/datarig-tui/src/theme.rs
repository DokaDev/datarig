//! Color tokens, 24-bit truecolor.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub const BG: Color = rgb(0x14161B);
pub const SURFACE: Color = rgb(0x1B1E25);
pub const SURFACE_ALT: Color = rgb(0x20242C);
pub const BORDER: Color = rgb(0x2E3440);
pub const ACCENT: Color = rgb(0x4FB3A9);
pub const ACCENT_WARM: Color = rgb(0xE0A96D);
pub const FG: Color = rgb(0xD8DEE9);
pub const FG_MUTED: Color = rgb(0x8A93A6);
pub const FG_DIM: Color = rgb(0x5C6577);
pub const SELECTION_BG: Color = rgb(0x2A3B4D);
/// Cells of a selected range in the grid (the cursor's cell keeps `SELECTION_BG`).
pub const RANGE_BG: Color = rgb(0x1E3438);
pub const CURSOR_LINE_BG: Color = rgb(0x1F2530);
/// A faint tint behind the statement a run would take.
pub const CURRENT_STMT_BG: Color = rgb(0x1A2129);
/// The bar in the editor's gutter next to the statement a run would take (or the selection):
/// subtle, but it stays visible where the tint does not (256 colors).
pub const CURRENT_STMT_BAR: Color = rgb(0x3F8F87);
pub const SUCCESS: Color = rgb(0x8FC77A);
pub const WARNING: Color = rgb(0xE6C35C);
pub const ERROR: Color = rgb(0xE06C75);
pub const NULL_FG: Color = rgb(0x6B7280);
pub const SYN_KEYWORD: Color = rgb(0xC792EA);
pub const SYN_FUNCTION: Color = rgb(0x6FA8DC);
pub const SYN_STRING: Color = rgb(0x9CCB85);
pub const SYN_NUMBER: Color = rgb(0xE0A96D);
pub const SYN_COMMENT: Color = rgb(0x5C6577);
pub const SYN_OPERATOR: Color = rgb(0x8A93A6);
pub const SYN_IDENTIFIER: Color = rgb(0xD8DEE9);
pub const SYN_QUOTED_IDENT: Color = rgb(0x4FB3A9);
/// Key column marks: primary key, foreign key, unique key. Each reads on
/// the background and the grid's header surface and stays apart from the other two, also in
/// 256 colors.
pub const KEY_PK: Color = rgb(0xE5C07B);
pub const KEY_FK: Color = rgb(0x61AFEF);
pub const KEY_UQ: Color = rgb(0xC678DD);
/// The mode badge at the left of the status bar (lualine style): a background per mode with
/// [`MODE_FG`] bold on it. Each reads at 4.5:1 or more and stays apart from the others and
/// from the status bar, also in 256 colors (theme tests).
pub const MODE_NORMAL: Color = rgb(0x61AFEF);
pub const MODE_INSERT: Color = rgb(0x98C379);
pub const MODE_VISUAL: Color = rgb(0xC678DD);
/// While the `:` command line is open.
pub const MODE_COMMAND: Color = rgb(0xE5935A);
/// `[editor] mode = "standard"`: no modes to tell apart.
pub const MODE_NEUTRAL: Color = rgb(0x8A93A6);
pub const MODE_FG: Color = rgb(0x14161B);
/// The tone the screen behind a dialog is blended toward.
pub const BACKDROP: Color = rgb(0x0B0C10);
/// Percent of a color that stays when it is dimmed behind a dialog (the rest is `BACKDROP`).
/// Dimmed body text keeps a contrast of about 4.5:1, and every dimmed token still maps to a
/// different xterm-256 color than the dimmed background.
const DIM_KEEP: u16 = 55;

/// RGB of the 12 named profile colors, in the order of
/// [`datarig_core::profile::color::NAMES`]. Chosen to read on [`BG`] and to stay apart from
/// each other.
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

/// The color of a key column mark.
pub fn key_color(k: crate::icons::KeyMark) -> Color {
    match k {
        crate::icons::KeyMark::Pk => KEY_PK,
        crate::icons::KeyMark::Fk => KEY_FK,
        crate::icons::KeyMark::Uq => KEY_UQ,
    }
}

/// The color of a profile (its dot, tab label and status bar name).
pub fn profile_color(c: datarig_core::profile::color::ProfileColor) -> Color {
    use datarig_core::profile::color::ProfileColor;
    match c {
        ProfileColor::Named(i) => PROFILE_COLORS[usize::from(i) % PROFILE_COLORS.len()],
        ProfileColor::Hex(r, g, b) => Color::Rgb(r, g, b),
    }
}

pub fn base() -> Style {
    Style::new().fg(FG).bg(BG)
}

/// `c` blended toward [`BACKDROP`]; a terminal default color (not RGB) is treated as
/// `default` first.
fn dimmed(c: Color, default: Color) -> Color {
    let rgb = |c: Color| if let Color::Rgb(r, g, b) = c { Some((r, g, b)) } else { None };
    let (Some((r, g, b)), Some((br, bg, bb))) = (rgb(c).or(rgb(default)), rgb(BACKDROP)) else { return c };
    let mix = |a: u8, z: u8| ((u16::from(a) * DIM_KEEP + u16::from(z) * (100 - DIM_KEEP)) / 100) as u8;
    Color::Rgb(mix(r, br), mix(g, bg), mix(b, bb))
}

/// A text color of the screen behind a dialog.
pub fn dim_fg(c: Color) -> Color {
    dimmed(c, FG)
}

/// A background color of the screen behind a dialog.
pub fn dim_bg(c: Color) -> Color {
    dimmed(c, BG)
}

/// Dim what is drawn in `area` so a dialog drawn on top of it stands out while the screen
/// below stays readable. Symbols are kept.
pub fn dim_area(buf: &mut Buffer, area: Rect) {
    let area = area.intersection(buf.area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            cell.fg = dim_fg(cell.fg);
            cell.bg = dim_bg(cell.bg);
        }
    }
}

pub fn syntax(kind: datarig_core::sql::lexer::Tok, is_function: bool) -> Style {
    use datarig_core::sql::lexer::Tok;
    let s = Style::new();
    if is_function {
        return s.fg(SYN_FUNCTION);
    }
    match kind {
        Tok::Keyword => s.fg(SYN_KEYWORD).add_modifier(Modifier::BOLD),
        Tok::Str | Tok::Dollar => s.fg(SYN_STRING),
        Tok::Number => s.fg(SYN_NUMBER),
        Tok::LineComment | Tok::BlockComment => s.fg(SYN_COMMENT).add_modifier(Modifier::ITALIC),
        Tok::QuotedIdent => s.fg(SYN_QUOTED_IDENT),
        Tok::Ident | Tok::Param => s.fg(SYN_IDENTIFIER),
        Tok::Op | Tok::Semi | Tok::Dot | Tok::Comma | Tok::LParen | Tok::RParen => s.fg(SYN_OPERATOR),
        Tok::Whitespace => s.fg(FG),
    }
}

#[cfg(test)]
mod tests;
