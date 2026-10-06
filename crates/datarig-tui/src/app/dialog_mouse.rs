//! The mouse on the dialogs. Every dialog keeps where its renderer drew what can be clicked
//! (buttons, rows, inputs, the profile form's parts), so a click hits what is on screen, and a
//! click does what the key for it does: a button presses its key, a row is picked as `Enter`
//! picks it. A button, a list row or a command line entry acts as a GUI button does: a press
//! arms it and the release over the same target acts, presses right after the dialog came on
//! top are ignored ([`super::overlay::ARM_DELAY`]) and a move with no button held drops an arm
//! (its release was lost). Focusing a field, placing the cursor and stepping a value act on
//! the press. A click outside a dialog does nothing (the menu alone closes on one). The pointer
//! (no button pressed) only highlights the button or the row under it: the focus and the
//! selection, what `Enter` acts on, stay where the keys put them (the menu and the keyboard
//! help, where it selects, aside); a change of the list drops the highlight. A move that changes
//! nothing draws no frame.

use super::profiles::FormHit;
use super::quick::QuickRow;
use super::*;
use ratatui::layout::Position;

/// A key press as a button gives it.
fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// A left click.
fn click(m: &MouseEvent) -> bool {
    m.kind == MouseEventKind::Down(MouseButton::Left)
}

/// A wheel notch: down (1) or up (-1).
fn wheel(m: &MouseEvent) -> Option<isize> {
    match m.kind {
        MouseEventKind::ScrollDown => Some(1),
        MouseEventKind::ScrollUp => Some(-1),
        _ => None,
    }
}

/// The row of a list drawn in `list` from row `first` under (x, y), if one of `n` rows is there.
fn row_at(list: Rect, first: usize, n: usize, x: u16, y: u16) -> Option<usize> {
    list.contains(Position::new(x, y)).then(|| first + usize::from(y - list.y)).filter(|i| *i < n)
}

impl App {
    /// The mouse while a dialog is on top: to that dialog.
    pub(super) fn overlay_mouse(&mut self, m: MouseEvent) {
        // Nothing is drawn on a screen too small: nothing to hit.
        if self.layout.too_small {
            return;
        }
        let Some(kind) = self.overlays.top().map(|o| o.kind()) else { return };
        match kind {
            OverlayKind::Help => self.help_mouse(m),
            OverlayKind::ScriptTree => self.script_tree_mouse(m),
            OverlayKind::ContextMenu => self.menu_mouse(m),
            OverlayKind::ProfileForm => self.form_mouse(m),
            OverlayKind::Confirm => self.confirm_mouse(m),
            OverlayKind::RunConfirm => self.run_confirm_mouse(m),
            OverlayKind::IconsAsk => self.icons_ask_mouse(m),
            OverlayKind::Password => self.prompt_mouse(m),
            OverlayKind::NameInput => self.name_mouse(m),
            OverlayKind::Chooser => self.chooser_mouse(m),
            OverlayKind::QuickConnect => self.quick_mouse(m),
            OverlayKind::TabList => self.tab_list_mouse(m),
            OverlayKind::Settings => self.settings_mouse(m),
            OverlayKind::Commands => self.command_mouse(m),
            OverlayKind::CellViewer | OverlayKind::WhichKey | OverlayKind::Busy => {}
        }
    }

    /// The pointer moved (no button) while a dialog is on top: `true` when something on screen
    /// changed (a frame is needed).
    pub(super) fn overlay_hover(&mut self, m: MouseEvent) -> bool {
        if self.layout.too_small {
            return false;
        }
        let (x, y) = (m.column, m.row);
        // A move has no button held: a press it armed lost its release.
        if let Some(f) = self.overlays.form_mut() {
            f.press.disarm();
        }
        if let Some(c) = self.overlays.chooser_mut() {
            c.press.disarm();
        }
        if let Some(q) = self.overlays.quick_mut() {
            q.press.disarm();
        }
        if let Some(l) = self.overlays.tab_list_mut() {
            l.press.disarm();
        }
        if let Some(c) = self.overlays.command_line_mut() {
            c.press.disarm();
        }
        match self.overlays.top().map(|o| o.kind()) {
            Some(OverlayKind::ContextMenu) => self.menu_hover(m),
            Some(OverlayKind::Help) => self.help_hover(m),
            Some(OverlayKind::ProfileForm) => self.form_hover(m),
            Some(OverlayKind::Confirm) => self.overlays.confirm_mut().is_some_and(|c| c.buttons.hover(x, y)),
            Some(OverlayKind::RunConfirm) => self.overlays.run_confirm_mut().is_some_and(|c| c.buttons.hover(x, y)),
            Some(OverlayKind::IconsAsk) => self.overlays.icons_ask_mut().is_some_and(|q| q.buttons.hover(x, y)),
            Some(OverlayKind::Password) => self.overlays.prompt_mut().is_some_and(|p| p.buttons.hover(x, y)),
            Some(OverlayKind::NameInput) => self.overlays.name_input_mut().is_some_and(|n| n.buttons.hover(x, y)),
            Some(OverlayKind::Chooser) => {
                let Some(c) = self.overlays.chooser_mut() else { return false };
                // Over the selected row the highlight would not show.
                let h = row_at(c.list, c.scroll, c.visible().len(), x, y).filter(|i| *i != c.selected);
                std::mem::replace(&mut c.hover, h) != h
            }
            Some(OverlayKind::QuickConnect) => {
                let Some(q) = self.overlays.quick_mut() else { return false };
                let h = row_at(q.list, q.scroll, q.items.len(), x, y).filter(|i| *i != q.selected);
                std::mem::replace(&mut q.hover, h) != h
            }
            Some(OverlayKind::TabList) => self.tab_list_hover(m),
            Some(OverlayKind::Settings) => {
                let Some(s) = self.overlays.settings_mut() else { return false };
                let h = s.rows.iter().find(|r| r.line.contains(Position::new(x, y))).map(|r| r.row);
                let h = h.filter(|i| *i != s.selected);
                std::mem::replace(&mut s.hover, h) != h
            }
            _ => false,
        }
    }

    /// The mouse on the profile form: a section's tab shows it; a field's line focuses the
    /// field, its input puts the cursor under the pointer; a value shown side by side is
    /// picked; a selector's `‹`/`›` are the arrow keys and its value is `Space` (`Enter` on a
    /// picker opens its list); a button (on the release over the button pressed, as on the other
    /// dialogs) is pressed as `Enter` presses it, `[…]` opens the key file picker and "save as
    /// tunnel preset" asks for the preset's name. Nothing while it saves.
    pub(super) fn form_mouse(&mut self, m: MouseEvent) {
        let now = self.now();
        let Some(f) = self.overlays.form_mut().filter(|f| !f.saving) else { return };
        let at = f.hit_at(m.column, m.row);
        // Buttons act on the release; the other parts (they focus, place the cursor, pick a
        // value) on the press.
        let hit = match m.kind {
            MouseEventKind::Down(MouseButton::Left) if at.is_some_and(FormHit::is_button) => {
                f.press.press(m.kind, at, now);
                return;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                f.press.disarm();
                at
            }
            MouseEventKind::Up(MouseButton::Left) => f.press.press(m.kind, at, now),
            _ => None,
        };
        let Some(hit) = hit else { return };
        match hit {
            FormHit::Section(s) => f.open_section(s),
            FormHit::Field(field) => f.focus = field,
            FormHit::Input(field) => {
                f.focus = field;
                if let Some(input) = f.input_mut(field) {
                    input.click(m.column, m.row);
                }
            }
            FormHit::Choice(field, i) => {
                f.focus = field;
                f.pick(field, i);
            }
            FormHit::Prev(field) | FormHit::Next(field) | FormHit::Value(field) => {
                f.focus = field;
                let code = match hit {
                    FormHit::Prev(_) => KeyCode::Left,
                    FormHit::Next(_) => KeyCode::Right,
                    _ if field.is_picker() => KeyCode::Enter,
                    _ => KeyCode::Char(' '),
                };
                self.form_key(press(code), false);
            }
            FormHit::Button(b) => {
                f.focus = b;
                self.form_key(press(KeyCode::Enter), false);
            }
            FormHit::KeyFile => self.open_key_picker(),
            FormHit::SaveAsPreset => self.open_save_as_tunnel(),
        }
    }

    /// The pointer over the profile form: the button under it is highlighted.
    fn form_hover(&mut self, m: MouseEvent) -> bool {
        let Some(f) = self.overlays.form_mut() else { return false };
        let h = f.hit_at(m.column, m.row).filter(|h| h.is_button());
        std::mem::replace(&mut f.hover, h) != h
    }

    /// A button of a confirmation presses its key (`y`, `n`, `r`, `o`, `Esc`).
    fn confirm_mouse(&mut self, m: MouseEvent) {
        let now = self.now();
        let Some(c) = self.overlays.confirm_mut() else { return };
        let Some(i) = c.buttons.press(m.kind, m.column, m.row, now) else { return };
        let Some(code) = c.buttons().0.get(i).map(|b| b.1) else { return };
        self.confirm_key(press(code), false);
    }

    /// Cancel or Run answers the run confirmation.
    fn run_confirm_mouse(&mut self, m: MouseEvent) {
        let now = self.now();
        let Some(c) = self.overlays.run_confirm_mut() else { return };
        let Some(i) = c.buttons.press(m.kind, m.column, m.row, now) else { return };
        self.run_confirm_key(press(if i == 1 { KeyCode::Char('y') } else { KeyCode::Char('n') }), false);
    }

    /// Yes or No answers the icons question.
    fn icons_ask_mouse(&mut self, m: MouseEvent) {
        let now = self.now();
        let Some(q) = self.overlays.icons_ask_mut() else { return };
        let Some(i) = q.buttons.press(m.kind, m.column, m.row, now) else { return };
        self.icons_ask_key(press(if i == 0 { KeyCode::Char('y') } else { KeyCode::Char('n') }), false);
    }

    /// The password prompt: the field takes the focus and the cursor, the checkbox is ticked or
    /// cleared (and takes the focus), OK sends as `Enter`, Cancel closes as `Esc`.
    fn prompt_mouse(&mut self, m: MouseEvent) {
        let now = self.now();
        let Some(p) = self.overlays.prompt_mut() else { return };
        let (x, y) = (m.column, m.row);
        match p.buttons.press(m.kind, x, y, now) {
            Some(0) => return self.prompt_key(press(KeyCode::Enter), false),
            Some(_) => return self.prompt_key(press(KeyCode::Esc), false),
            None if !click(&m) || p.buttons.at(x, y).is_some() => return,
            None => {}
        }
        if p.save_to.is_some() && p.checkbox.contains(Position::new(x, y)) {
            p.save_focus = true;
            p.save = !p.save;
        } else if p.input.click(x, y) {
            p.save_focus = false;
        }
    }

    /// The name input: the cursor under the pointer; OK as `Enter`, Cancel as `Esc`.
    fn name_mouse(&mut self, m: MouseEvent) {
        let now = self.now();
        let Some(n) = self.overlays.name_input_mut() else { return };
        match n.buttons.press(m.kind, m.column, m.row, now) {
            Some(0) => self.name_key(press(KeyCode::Enter), false),
            Some(_) => self.name_key(press(KeyCode::Esc), false),
            None if click(&m) => {
                n.input.click(m.column, m.row);
            }
            None => {}
        }
    }

    /// The chooser: a click on a row picks it (as `Enter`, on the release over the row pressed),
    /// one on the filter line types the filter there; the wheel moves the selection.
    fn chooser_mouse(&mut self, m: MouseEvent) {
        let now = self.now();
        let Some(c) = self.overlays.chooser_mut() else { return };
        if let Some(d) = wheel(&m) {
            c.hover = None;
            return c.step(d);
        }
        let row = row_at(c.list, c.scroll, c.visible().len(), m.column, m.row);
        if let Some(i) = c.press.press(m.kind, row, now) {
            c.selected = i;
            self.chooser_pick();
        } else if click(&m) && row.is_none() && c.filter.click(m.column, m.row) {
            c.filtering = true;
        }
    }

    /// Quick connect: a click on a row picks it (as `Enter`), one on its `▸`/`▾` lists or hides
    /// what is under it (as `→`/`←`), both on the release over what was pressed; one on the
    /// input puts the cursor there; the wheel moves the selection.
    fn quick_mouse(&mut self, m: MouseEvent) {
        let now = self.now();
        let Some(q) = self.overlays.quick_mut() else { return };
        let (x, y) = (m.column, m.row);
        if let Some(d) = wheel(&m) {
            return self.quick_key(press(if d > 0 { KeyCode::Down } else { KeyCode::Up }), false);
        }
        let arrow = q.arrows.iter().find(|(r, _)| r.contains(Position::new(x, y))).map(|a| (a.1, true));
        let hit = arrow.or_else(|| row_at(q.list, q.scroll, q.items.len(), x, y).map(|i| (i, false)));
        match q.press.press(m.kind, hit, now) {
            Some((i, true)) => {
                q.selected = i;
                let open = match q.items.get(i) {
                    Some(QuickRow::Profile(p)) => q.open.contains(p),
                    Some(QuickRow::Database(p, db)) => q.open_db.contains(&(*p, db.clone())),
                    _ => return,
                };
                self.quick_key(press(if open { KeyCode::Left } else { KeyCode::Right }), false);
            }
            Some((i, false)) => {
                q.selected = i;
                self.quick_key(press(KeyCode::Enter), false);
            }
            None if click(&m) && hit.is_none() => {
                q.input.click(x, y);
            }
            None => {}
        }
    }

    /// The settings: a click on a row selects it; on its `‹` or `›` the previous or next value
    /// (`h`/`l`), on the value itself what `Enter` does; the wheel moves the selection one row,
    /// stopping at the ends.
    fn settings_mouse(&mut self, m: MouseEvent) {
        let Some(s) = self.overlays.settings_mut() else { return };
        if let Some(d) = wheel(&m) {
            // One row a notch, stopping at the ends.
            let last = super::settings::order().len().saturating_sub(1);
            s.selected = s.selected.saturating_add_signed(d).min(last);
            return;
        }
        let at = Position::new(m.column, m.row);
        let Some(r) = s.rows.iter().find(|r| r.line.contains(at)).filter(|_| click(&m)).cloned() else { return };
        s.selected = r.row;
        let code = if r.prev.contains(at) {
            KeyCode::Char('h')
        } else if r.next.contains(at) {
            KeyCode::Char('l')
        } else if r.value.contains(at) {
            KeyCode::Enter
        } else {
            return;
        };
        self.settings_key(press(code), false);
    }

    /// The command line: a click on an entry runs it (as `Enter` on it, on the release over the
    /// entry pressed), one on the input puts the cursor there; the wheel moves the selection.
    fn command_mouse(&mut self, m: MouseEvent) {
        let now = self.now();
        let Some(c) = self.overlays.command_line_mut() else { return };
        if let Some(d) = wheel(&m) {
            return self.command_key(press(if d > 0 { KeyCode::Down } else { KeyCode::Up }), false);
        }
        let row = row_at(c.list, c.offset, c.items.len(), m.column, m.row);
        if let Some(i) = c.press.press(m.kind, row, now) {
            c.selected = i;
            c.picked = true;
            self.command_enter();
        } else if click(&m) && row.is_none() {
            c.input.click(m.column, m.row);
        }
    }
}
