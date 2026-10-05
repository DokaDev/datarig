//! The settings screen: every setting of the `:set` table
//! ([`command::SETTINGS`]) in one list under its category (display, editor, results,
//! clipboard, language, secrets), with its current value and a localized description of the
//! selected one. A change applies at once and is saved to the config file through the same
//! path as `:set` (toml_edit keeps comments and order), so the screen, `:set` and the file
//! always agree. A config file with errors is said so on top (nothing is saved then).
//!
//! The theme row lists the built-in themes and the theme files found when the screen opens;
//! moving over its values previews a theme (see [`super::themes`]).
//!
//! It is a large modal like the profile form rather than a workspace tab: settings are global
//! and belong to no connection, so a tab would need exceptions in the tab binding, restore
//! and autosave rules for nothing.

use super::command::{SETTINGS, Setting, SettingGroup, Values};
use super::themes::ThemeRow;
use super::*;

/// The screen's state: the selected row of [`order`], and the theme row's names and preview.
pub struct SettingsScreen {
    pub selected: usize,
    pub themes: ThemeRow,
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
        let themes = ThemeRow { names: self.theme_names(), preview: None };
        self.overlays.push(Overlay::Settings(SettingsScreen { selected: 0, themes }));
    }

    /// The value setting `k` of [`SETTINGS`] has now (an index into its values).
    pub fn setting_value(&self, k: usize) -> Option<usize> {
        let now = |s: &Setting| match s {
            Setting::Language(l) => *l == self.lang_setting,
            Setting::Icons(i) => *i == self.icons,
            Setting::DefaultSource(d) => *d == self.default_source,
            Setting::CommandsPosition(p) => *p == self.prefs.commands_position,
            Setting::DetailView(v) => *v == self.prefs.detail_view,
            Setting::Clipboard(c) => *c == self.prefs.clipboard,
            Setting::CopyHeader(c) => *c == self.prefs.copy_header,
            Setting::CursorShape(c) => *c == self.prefs.cursor_shape,
            Setting::EditorClipboard(c) => *c == self.prefs.editor_clipboard,
            Setting::FormatCase(c) => *c == self.prefs.format_case,
            Setting::FormatIndent(i) => *i == self.prefs.format_indent,
            Setting::AutoPairs(a) => *a == self.prefs.auto_pairs,
        };
        SETTINGS.get(k)?.values.fixed().iter().position(|v| now(&v.1))
    }

    /// What the settings screen shows for setting `k`: its value's name and label. The theme
    /// row shows the theme being previewed, else the configured one.
    pub fn setting_shown(&self, k: usize) -> Option<(String, Localized)> {
        let spec = SETTINGS.get(k)?;
        match spec.values {
            Values::Fixed(values) => {
                let (name, _, label) = values.get(self.setting_value(k)?)?;
                Some((name.to_string(), self.i18n.label(*label)))
            }
            Values::Themes => {
                let row = &self.overlays.settings()?.themes;
                let name = match &row.preview {
                    Some(p) => row.names.get(p.index)?.clone(),
                    None => self.theme_name.clone(),
                };
                let label = if row.preview.is_none() && self.theme_problem().is_some() {
                    self.i18n.label(Label::SettingThemeNotUsed)
                } else {
                    self.theme_label(&name)
                };
                Some((name, label))
            }
        }
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
                self.end_preview();
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
        let Values::Fixed(values) = SETTINGS[k].values else {
            // The theme row: h/l preview, Enter/Space keep the previewed theme.
            match key.code {
                KeyCode::Char('h' | 'l') | KeyCode::Left | KeyCode::Right => self.preview_theme(step),
                _ => self.keep_preview(),
            }
            return;
        };
        let now = self.setting_value(k).unwrap_or(0) as isize;
        let next = (now + step).rem_euclid(values.len() as isize) as usize;
        self.apply_setting(values[next].1);
    }
}
