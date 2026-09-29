//! The settings screen: every setting of the `:set` table
//! ([`command::SETTINGS`]) in one list under its category (display, editor, results,
//! clipboard, language, secrets), with its current value and a localized description of the
//! selected one. A change applies at once and is saved to the config file through the same
//! path as `:set` (toml_edit keeps comments and order), so the screen, `:set` and the file
//! always agree. A config file with errors is said so on top (nothing is saved then).
//!
//! It is a large modal like the profile form rather than a workspace tab: settings are global
//! and belong to no connection, so a tab would need exceptions in the tab binding, restore
//! and autosave rules for nothing.

use super::command::{SETTINGS, Setting, SettingGroup};
use super::*;

/// The screen's state: the selected row of [`order`].
pub struct SettingsScreen {
    pub selected: usize,
}

/// The rows: indices into [`SETTINGS`], by category, then in table order.
pub fn order() -> Vec<usize> {
    let mut v: Vec<usize> = (0..SETTINGS.len()).collect();
    v.sort_by_key(|&i| (SETTINGS[i].group, i));
    v
}

/// The categories in screen order, each with its rows (indices into [`SETTINGS`]).
pub fn groups() -> Vec<(SettingGroup, Vec<usize>)> {
    let mut out: Vec<(SettingGroup, Vec<usize>)> = Vec::new();
    for i in order() {
        match out.last_mut() {
            Some((g, v)) if *g == SETTINGS[i].group => v.push(i),
            _ => out.push((SETTINGS[i].group, vec![i])),
        }
    }
    out
}

impl App {
    /// `Space ,` / `:settings`.
    pub(super) fn open_settings(&mut self) {
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.key_state.clear();
        self.overlays.push(Overlay::Settings(SettingsScreen { selected: 0 }));
    }

    /// The value setting `k` of [`SETTINGS`] has now (an index into its values).
    pub fn setting_value(&self, k: usize) -> Option<usize> {
        let now = |s: &Setting| match s {
            Setting::Language(l) => *l == self.lang_setting,
            Setting::Editor(m) => *m == self.editor_mode,
            Setting::Icons(i) => *i == self.icons,
            Setting::DefaultSource(d) => *d == self.default_source,
            Setting::CommandsPosition(p) => *p == self.prefs.commands_position,
            Setting::DetailView(v) => *v == self.prefs.detail_view,
            Setting::Clipboard(c) => *c == self.prefs.clipboard,
            Setting::CopyHeader(c) => *c == self.prefs.copy_header,
            Setting::CursorShape(c) => *c == self.prefs.cursor_shape,
        };
        SETTINGS.get(k)?.values.iter().position(|v| now(&v.1))
    }

    /// The config file settings are saved to, if there is one.
    pub fn config_file(&self) -> Option<String> {
        self.config_path.as_ref().map(|p| p.display().to_string())
    }

    /// Why changes are not saved, if the config file could not be used (its error, typed).
    pub fn config_problem(&self) -> Option<&Notice> {
        self.config_problem.as_ref()
    }

    /// Keys of the settings screen: move, change the selected setting (previous / next value),
    /// close.
    pub(super) fn settings_key(&mut self, key: KeyEvent, repeat: bool) {
        let rows = order();
        let Some(s) = self.overlays.settings_mut() else { return };
        let n = rows.len();
        let step = match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                s.selected = (s.selected + 1) % n;
                return;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                s.selected = (s.selected + n - 1) % n;
                return;
            }
            KeyCode::Esc | KeyCode::Char('q') if !repeat => {
                self.overlays.close(OverlayKind::Settings);
                return;
            }
            KeyCode::Char('h') | KeyCode::Left => -1,
            KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ') => 1,
            _ => return,
        };
        if repeat {
            return;
        }
        let k = rows[s.selected];
        let values = SETTINGS[k].values;
        let now = self.setting_value(k).unwrap_or(0) as isize;
        let next = (now + step).rem_euclid(values.len() as isize) as usize;
        self.apply_setting(values[next].1);
    }
}
