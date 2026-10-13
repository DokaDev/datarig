//! The theme setting: the name the config file has (`theme`, kept as written and saved back
//! even when it does not resolve), the theme drawn with, and the terminal's background that
//! families follow. A name that does not resolve — unknown, a broken theme file, a file with a
//! built-in name — never falls back silently: the app draws with `terminal` and says why, at
//! launch and on the settings screen.
//!
//! `:set theme=<name>` and the settings screen apply a theme through [`App::set_theme`], which
//! saves it like every other setting. On the settings screen `h`/`l` only preview a theme (the
//! frames are drawn with it); `Enter` keeps it, `Esc` goes back to the one before.
//!
//! `:theme copy <from> <name>`, `:theme new <name>` and `c` on the settings screen's theme row
//! write a new theme file ([`crate::theme::copy_theme`], [`crate::theme::new_theme`]), then use
//! it like `:set theme=<name>` and say where the file is.

use super::chooser::{NameInput, NamePurpose};
use super::*;
use crate::theme::{self, AuthorError, Background, Theme};
use datarig_core::theme::{Problem, ThemeError};

/// The theme row of the settings screen: the names found when the screen opened, and the theme
/// being previewed.
#[derive(Default)]
pub struct ThemeRow {
    pub names: Vec<String>,
    pub preview: Option<Preview>,
}

pub struct Preview {
    /// Index into [`ThemeRow::names`].
    pub index: usize,
    /// The theme drawn before the preview (`Esc` brings it back).
    pub before: Arc<Theme>,
    /// Why the previewed theme cannot be used (the screen keeps drawing with `before`).
    pub error: Option<Notice>,
}

/// The label of a built-in theme name.
pub fn builtin_label(name: &str) -> Option<Label> {
    Some(match name {
        "terminal" => Label::ThemeNameTerminal,
        "dark" => Label::ThemeNameDark,
        "light" => Label::ThemeNameLight,
        "high-contrast" => Label::ThemeNameHighContrast,
        "catppuccin" => Label::ThemeNameCatppuccin,
        "catppuccin-latte" => Label::ThemeNameCatppuccinLatte,
        "catppuccin-mocha" => Label::ThemeNameCatppuccinMocha,
        "tokyo-night" => Label::ThemeNameTokyoNight,
        "tokyo-night-day" => Label::ThemeNameTokyoNightDay,
        "tokyo-night-night" => Label::ThemeNameTokyoNightNight,
        "gruvbox" => Label::ThemeNameGruvbox,
        "gruvbox-light" => Label::ThemeNameGruvboxLight,
        "gruvbox-dark" => Label::ThemeNameGruvboxDark,
        "nord" => Label::ThemeNameNord,
        "dracula" => Label::ThemeNameDracula,
        _ => return None,
    })
}

/// Why a theme cannot be used, in words.
pub fn theme_error_msg(i18n: &I18n, e: &ThemeError) -> Msg {
    let shown = |p: &std::path::Path| p.display().to_string();
    match e {
        ThemeError::Unknown { name, dir: Some(dir) } => {
            Msg::ThemeUnknown { name: name.clone(), path: shown(&datarig_core::theme::path(dir, name)) }
        }
        ThemeError::Unknown { name, dir: None } => Msg::ThemeUnknownNoDir { name: name.clone() },
        ThemeError::Shadows { path } => Msg::ThemeShadows { path: shown(path) },
        ThemeError::Read { path, fault } => {
            Msg::ThemeReadFailed { path: shown(path), reason: i18n.msg(&persist::fault_reason(fault)).to_string() }
        }
        ThemeError::File { path, line, problem } => {
            let problem = match problem {
                Problem::Toml(m) => m.clone(),
                Problem::UnknownToken(name) => {
                    i18n.msg(&Msg::ThemeProblemUnknownToken { name: name.clone() }).to_string()
                }
                Problem::NotAColor(name) => i18n.msg(&Msg::ThemeProblemNotAColor { name: name.clone() }).to_string(),
                Problem::NotAStyle(name) => i18n.msg(&Msg::ThemeProblemNotAStyle { name: name.clone() }).to_string(),
                Problem::BadColor(value) => i18n.msg(&Msg::ThemeProblemBadColor { value: value.clone() }).to_string(),
                Problem::BadModifier(value) => {
                    i18n.msg(&Msg::ThemeProblemBadModifier { value: value.clone() }).to_string()
                }
                Problem::UnknownBase(name) => {
                    i18n.msg(&Msg::ThemeProblemUnknownBase { name: name.clone() }).to_string()
                }
            };
            match line {
                Some(line) => Msg::ThemeFileError { path: shown(path), line: line.to_string(), problem },
                None => Msg::ThemeFileErrorNoLine { path: shown(path), problem },
            }
        }
    }
}

impl App {
    /// `themes/` next to the config file (none without a config file).
    pub fn themes_dir(&self) -> Option<PathBuf> {
        self.config_path.as_deref().and_then(datarig_core::theme::dir)
    }

    /// Every theme name `theme` takes now: the built-in ones, then the theme files.
    pub fn theme_names(&self) -> Vec<String> {
        theme::names(self.themes_dir().as_deref())
    }

    /// The theme `name` as it would be drawn now (the background decides a family's variant).
    pub fn resolve_theme(&self, name: &str) -> Result<Theme, Notice> {
        theme::resolve(name, self.background, self.themes_dir().as_deref())
            .map_err(|e| Notice::new(theme_error_msg(&self.i18n, &e), Level::Error))
    }

    /// Draw with the theme the config names; one that does not resolve draws `terminal` and
    /// keeps why (shown at launch and on the settings screen).
    pub(super) fn load_theme(&mut self) {
        match self.resolve_theme(&self.theme_name.clone()) {
            Ok(th) => {
                self.theme = Arc::new(th);
                self.theme_problem = None;
            }
            Err(e) => {
                let error = e.render(&self.i18n).to_string();
                self.theme = Arc::new(theme::TERMINAL);
                self.theme_problem = Some(Notice::new(Msg::ThemeNotUsed { error }, Level::Error));
            }
        }
    }

    /// The terminal's background, as the binary found it at startup (tests set it): a family's
    /// variant follows it.
    pub fn set_background(&mut self, background: Background) {
        self.background = background;
        self.load_theme();
    }

    /// Why the configured theme is not the one drawn, if it is not.
    pub fn theme_problem(&self) -> Option<&Notice> {
        self.theme_problem.as_ref()
    }

    /// `:set theme=<name>` and `Enter` on the settings screen: draw with theme `name` and save
    /// it. A name that does not resolve changes nothing and is the error.
    pub fn set_theme(&mut self, name: &str) -> Result<(), Notice> {
        let msg =
            Msg::SettingChanged { name: self.i18n.label(Label::SettingTheme).to_string(), value: name.to_string() };
        self.switch_theme(name, Notice::new(msg, Level::Info))
    }

    /// Draw with theme `name`, save it and say `done` (or why it was not saved).
    fn switch_theme(&mut self, name: &str, done: Notice) -> Result<(), Notice> {
        let th = self.resolve_theme(name)?;
        self.theme = Arc::new(th);
        self.theme_name = name.to_string();
        self.theme_problem = None;
        let saved = self.persist();
        self.flash(saved.unwrap_or(done));
        Ok(())
    }

    /// `:theme copy <from> <name>` and the settings screen's copy: write `themes/<name>.toml`
    /// with every token of `from`, then draw with it (saved) and say where it is.
    pub fn copy_theme(&mut self, from: &str, name: &str) -> Result<(), AuthorError> {
        let dir = self.themes_dir().ok_or(AuthorError::NoDir)?;
        let path = theme::copy_theme(&dir, from, name, self.background)?;
        let msg =
            Msg::ThemeAuthorCopied { from: from.to_string(), path: path.display().to_string(), name: name.into() };
        self.wrote_theme(name, msg);
        Ok(())
    }

    /// `:theme new <name>`: write `themes/<name>.toml` extending the current theme when it is
    /// built in (else `dark`) with every token commented out, then draw with it (saved) and say
    /// where it is.
    pub fn new_theme(&mut self, name: &str) -> Result<(), AuthorError> {
        let dir = self.themes_dir().ok_or(AuthorError::NoDir)?;
        let base =
            if theme::NAMES.contains(&self.theme_name.as_str()) { self.theme_name.clone() } else { "dark".into() };
        let path = theme::new_theme(&dir, name, &base, self.background)?;
        self.wrote_theme(name, Msg::ThemeAuthorCreated { path: path.display().to_string(), base, name: name.into() });
        Ok(())
    }

    /// A theme file was written: use it. It is said even when it cannot be used (the file stays).
    fn wrote_theme(&mut self, name: &str, done: Msg) {
        if let Err(e) = self.switch_theme(name, Notice::new(done, Level::Success)) {
            self.flash(e);
        }
        // The settings screen lists the new file, and a preview is over: the theme is chosen.
        let names = self.theme_names();
        if let Some(s) = self.overlays.settings_mut() {
            s.themes.names = names;
            s.themes.preview = None;
        }
    }

    /// Why a theme file was not written, in words.
    pub fn author_error_msg(&self, e: &AuthorError) -> Msg {
        let shown = |p: &std::path::Path| p.display().to_string();
        match e {
            AuthorError::BadName(_) => Msg::Label(Label::ThemeAuthorNameInvalid),
            AuthorError::BuiltinName(_) => Msg::Label(Label::ThemeAuthorNameBuiltin),
            AuthorError::NoDir => Msg::Label(Label::ThemeAuthorNoDir),
            AuthorError::Source(e) => theme_error_msg(&self.i18n, e),
            AuthorError::Unwritable { token } => Msg::ThemeAuthorUnwritable { token: token.to_string() },
            AuthorError::Exists(path) => Msg::ThemeAuthorExists { path: shown(path) },
            AuthorError::Write { path, fault } => Msg::ThemeAuthorWriteFailed {
                path: shown(path),
                reason: self.i18n.msg(&persist::fault_reason(fault)).to_string(),
            },
        }
    }

    /// `c` on the settings screen's theme row: ask the name of a copy of the theme shown there
    /// (the one previewed, else the configured one). A theme that cannot be used says why.
    pub(super) fn open_copy_theme(&mut self) {
        let Some(row) = self.overlays.settings().map(|s| &s.themes) else { return };
        let names = row.names.clone();
        let shown = match &row.preview {
            Some(p) => p.error.clone().map_or_else(|| Ok(names.get(p.index).cloned().unwrap_or_default()), Err),
            None => self.theme_problem.clone().map_or_else(|| Ok(self.theme_name.clone()), Err),
        };
        let from = match shown {
            Ok(from) => from,
            Err(e) => return self.flash(e),
        };
        // `<from>-copy`, or the first `<from>-copy-<n>` no theme has.
        let free = (1..100)
            .map(|n| if n == 1 { format!("{from}-copy") } else { format!("{from}-copy-{n}") })
            .find(|n| !names.contains(n))
            .unwrap_or_default();
        self.overlays.push(Overlay::NameInput(NameInput {
            title: Msg::ThemeAuthorCopyTitle { from: from.clone() },
            input: crate::widgets::text_input::TextInput::new(&free),
            error: None,
            purpose: NamePurpose::CopyTheme { from },
            buttons: Default::default(),
        }));
    }

    /// `Enter` in the name of the settings screen's copy. A name that cannot be used stays in the
    /// input with why (`Err`); anything else closes it and is said.
    pub(super) fn copy_theme_named(&mut self, from: &str, name: &str) -> Result<(), Label> {
        match self.copy_theme(from, name) {
            Ok(()) => Ok(()),
            Err(AuthorError::BadName(_)) => Err(Label::ThemeAuthorNameInvalid),
            Err(AuthorError::BuiltinName(_)) => Err(Label::ThemeAuthorNameBuiltin),
            Err(AuthorError::Exists(_)) => Err(Label::ThemeAuthorTaken),
            Err(e) => {
                let msg = self.author_error_msg(&e);
                self.flash(Notice::new(msg, Level::Error));
                Ok(())
            }
        }
    }

    /// `h`/`l` on the settings screen's theme row: draw with the previous or next theme, without
    /// saving it.
    pub(super) fn preview_theme(&mut self, step: isize) {
        let Some(row) = self.overlays.settings().map(|s| &s.themes) else { return };
        let n = row.names.len().max(1) as isize;
        let now = match &row.preview {
            Some(p) => p.index,
            None => row.names.iter().position(|x| *x == self.theme_name).unwrap_or(0),
        } as isize;
        let index = (now + step).rem_euclid(n) as usize;
        let Some(name) = row.names.get(index).cloned() else { return };
        let before = row.preview.as_ref().map_or_else(|| self.theme.clone(), |p| p.before.clone());
        let error = match self.resolve_theme(&name) {
            Ok(th) => {
                self.theme = Arc::new(th);
                None
            }
            Err(e) => {
                self.theme = before.clone();
                Some(e)
            }
        };
        if let Some(s) = self.overlays.settings_mut() {
            s.themes.preview = Some(Preview { index, before, error });
        }
    }

    /// `Enter`/`Space` on the theme row: keep the previewed theme (saved). One that cannot be used
    /// says why again and stays previewed.
    pub(super) fn keep_preview(&mut self) {
        let Some(p) = self.overlays.settings_mut().and_then(|s| s.themes.preview.take()) else { return };
        if let Some(e) = p.error.clone() {
            if let Some(s) = self.overlays.settings_mut() {
                s.themes.preview = Some(p);
            }
            return self.flash(e);
        }
        let name = self.overlays.settings().and_then(|s| s.themes.names.get(p.index).cloned()).unwrap_or_default();
        if let Err(e) = self.set_theme(&name) {
            self.theme = p.before;
            self.flash(e);
        }
    }

    /// The settings screen closes: a theme only previewed goes back to the one before.
    pub(super) fn end_preview(&mut self) {
        if let Some(p) = self.overlays.settings_mut().and_then(|s| s.themes.preview.take()) {
            self.theme = p.before;
        }
    }

    /// The label of theme `name` on the settings screen and in the completions.
    pub fn theme_label(&self, name: &str) -> Localized {
        match builtin_label(name) {
            Some(l) => self.i18n.label(l),
            None => self.i18n.label(Label::SettingThemeUser),
        }
    }
}

#[cfg(test)]
mod tests;
