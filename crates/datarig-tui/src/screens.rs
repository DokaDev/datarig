//! Rendering of [`App`] into a ratatui frame (layout and colors).
//!
//! [`draw`] draws the workspace and then the overlays on top of it, bottom to top
//! ([`crate::app::overlay::Overlays`]). An overlay never blanks the screen: it clears its own box
//! and the screen behind it is dimmed.

mod workspace;

use crate::app::overlay::{Overlay, OverlayKind};
use crate::app::{App, Layout};
use crate::text::{clip, width};
use crate::theme;
use crate::widgets::chooser::{draw_chooser, draw_name_input};
use crate::widgets::cmdline::draw_command_line;
use crate::widgets::dialog::{draw_busy, draw_confirm, draw_prompt};
use crate::widgets::form::draw_profile_form;
use crate::widgets::guide::{draw_help, draw_which_key};
use crate::widgets::menu::draw_context_menu;
use crate::widgets::popup::draw_viewer;
use crate::widgets::quick::draw_quick_connect;
use datarig_core::i18n::Msg;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use workspace::draw_workspace;

pub const MIN_W: u16 = 80;
pub const MIN_H: u16 = 24;

/// Draws a frame with `app.theme` as the current theme ([`theme::cur`]).
pub fn draw(f: &mut Frame, app: &mut App) {
    let _theme = theme::scope(app.theme.clone());
    let th = theme::cur();
    let area = f.area();
    app.note_top();
    f.buffer_mut().set_style(area, th.base());
    if area.width < MIN_W || area.height < MIN_H {
        app.layout = Layout { too_small: true, ..Layout::default() };
        let msg = app.i18n.msg(&Msg::ScreenTooSmall { width: MIN_W.to_string(), height: MIN_H.to_string() });
        let msg = clip(&msg, area.width as usize);
        let x = area.x + (area.width.saturating_sub(width(&msg) as u16)) / 2;
        let y = area.y + area.height / 2;
        f.buffer_mut().set_stringn(x, y, &msg, area.width as usize, Style::new().fg(th.warning).bg(th.bg));
        return;
    }
    let cursor = draw_screen(f, app);
    draw_overlays(f, app, cursor);
}

/// The workspace below the overlays; returns where the hardware cursor belongs.
pub fn draw_screen(f: &mut Frame, app: &mut App) -> Option<(u16, u16)> {
    let _theme = theme::scope(app.theme.clone());
    draw_workspace(f, app)
}

/// The overlays, bottom to top, on the screen drawn by [`draw_screen`]. Each one clears only
/// its own box; before the top one the rest of the frame is dimmed, so the screen stays
/// visible behind it. The top overlay owns the hardware cursor.
pub fn draw_overlays(f: &mut Frame, app: &mut App, screen_cursor: Option<(u16, u16)>) {
    let _theme = theme::scope(app.theme.clone());
    let th = theme::cur();
    let area = f.area();
    let content = Rect::new(area.x, area.y, area.width, area.height - 1);
    let mut hw_cursor = screen_cursor;
    let kinds: Vec<OverlayKind> = app.overlays.iter().map(Overlay::kind).collect();
    for (i, &kind) in kinds.iter().enumerate() {
        if i + 1 == kinds.len() {
            th.dim_area(f.buffer_mut(), area);
        }
        hw_cursor = match kind {
            OverlayKind::CellViewer => {
                draw_viewer(app, content, f.buffer_mut());
                None
            }
            OverlayKind::ProfileForm => draw_profile_form(app, content, f.buffer_mut()),
            OverlayKind::Settings => {
                crate::widgets::settings::draw_settings(app, content, f.buffer_mut());
                None
            }
            OverlayKind::QuickConnect => draw_quick_connect(app, content, f.buffer_mut()),
            OverlayKind::TabList => crate::widgets::tab_list::draw_tab_list(app, content, f.buffer_mut()),
            OverlayKind::NameInput => draw_name_input(app, content, f.buffer_mut()),
            OverlayKind::ScriptTree => crate::widgets::script_tree::draw_script_tree(app, content, f.buffer_mut()),
            OverlayKind::ContextMenu => draw_context_menu(app, content, f.buffer_mut()),
            OverlayKind::Chooser => draw_chooser(app, content, f.buffer_mut()),
            OverlayKind::Help => draw_help(app, content, f.buffer_mut()),
            OverlayKind::WhichKey => {
                draw_which_key(app, content, f.buffer_mut());
                None
            }
            OverlayKind::Confirm => {
                draw_confirm(app, content, f.buffer_mut());
                None
            }
            OverlayKind::RunConfirm => {
                crate::widgets::dialog::draw_run_confirm(app, content, f.buffer_mut());
                None
            }
            OverlayKind::IconsAsk => {
                crate::widgets::dialog::draw_icons_ask(app, content, f.buffer_mut());
                None
            }
            OverlayKind::Busy => {
                draw_busy(app, content, f.buffer_mut());
                None
            }
            OverlayKind::Password => draw_prompt(app, content, f.buffer_mut()),
            OverlayKind::Commands => draw_command_line(app, area, f.buffer_mut()),
        };
    }
    if let Some(c) = hw_cursor {
        f.set_cursor_position(c);
    }
}
