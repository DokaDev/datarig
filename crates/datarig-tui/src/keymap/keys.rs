//! Key chords and their notation. Config files and the default table use the lowercase
//! notation (`ctrl+e`, `shift+tab`, `f4`, `pagedown`, `g t`, `space c n`); the screen shows
//! the display form (`Ctrl+E`, `Shift+Tab`, `F4`, `PageDown`, `g t`, `Space c n`). A space
//! separates the keys of a sequence. `G` and `shift+g` are the same key.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// One key press, normalized so that equal keys compare equal: for characters Shift is part of
/// the character (`G`, `?`, `$`), and Ctrl+letter is always lowercase (terminals cannot tell
/// Ctrl+Shift+E from Ctrl+E without the kitty protocol).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

const MODS: KeyModifiers = KeyModifiers::CONTROL.union(KeyModifiers::ALT).union(KeyModifiers::SHIFT);

/// Named keys: (notation, display, code).
const NAMES: &[(&str, &str, KeyCode)] = &[
    ("space", "Space", KeyCode::Char(' ')),
    ("enter", "Enter", KeyCode::Enter),
    ("esc", "Esc", KeyCode::Esc),
    ("tab", "Tab", KeyCode::Tab),
    ("backspace", "Backspace", KeyCode::Backspace),
    ("delete", "Delete", KeyCode::Delete),
    ("insert", "Insert", KeyCode::Insert),
    ("up", "Up", KeyCode::Up),
    ("down", "Down", KeyCode::Down),
    ("left", "Left", KeyCode::Left),
    ("right", "Right", KeyCode::Right),
    ("home", "Home", KeyCode::Home),
    ("end", "End", KeyCode::End),
    ("pageup", "PageUp", KeyCode::PageUp),
    ("pagedown", "PageDown", KeyCode::PageDown),
];

/// Accepted spellings besides the canonical names.
const ALIASES: &[(&str, &str)] =
    &[("return", "enter"), ("escape", "esc"), ("del", "delete"), ("pgup", "pageup"), ("pgdn", "pagedown")];

impl KeyChord {
    pub fn new(code: KeyCode, mods: KeyModifiers) -> Self {
        let mut mods = mods & MODS;
        let code = match code {
            KeyCode::Char(c) => {
                let shift = mods.contains(KeyModifiers::SHIFT);
                mods.remove(KeyModifiers::SHIFT);
                if mods.contains(KeyModifiers::CONTROL) {
                    KeyCode::Char(c.to_ascii_lowercase())
                } else if shift {
                    KeyCode::Char(c.to_ascii_uppercase())
                } else {
                    KeyCode::Char(c)
                }
            }
            KeyCode::Tab if mods.contains(KeyModifiers::SHIFT) => {
                mods.remove(KeyModifiers::SHIFT);
                KeyCode::BackTab
            }
            KeyCode::BackTab => {
                mods.remove(KeyModifiers::SHIFT);
                KeyCode::BackTab
            }
            c => c,
        };
        Self { code, mods }
    }

    pub fn char(c: char) -> Self {
        Self::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    pub fn from_event(k: &KeyEvent) -> Self {
        Self::new(k.code, k.modifiers)
    }

    /// A key event widgets understand (Shift is implied by the character).
    pub fn to_event(self) -> KeyEvent {
        KeyEvent::new(self.code, self.mods)
    }

    /// A character typed without Ctrl/Alt (text in a text input, a one-letter command elsewhere).
    pub fn is_plain_char(&self) -> bool {
        matches!(self.code, KeyCode::Char(_)) && !self.mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    }

    fn with_mods(&self, name: &str, ctrl: &str, alt: &str, shift: &str) -> String {
        let mut s = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            s.push_str(ctrl);
        }
        if self.mods.contains(KeyModifiers::ALT) {
            s.push_str(alt);
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            s.push_str(shift);
        }
        s.push_str(name);
        s
    }

    /// Display form: `Ctrl+E`, `Shift+Tab`, `Space`, `G`.
    pub fn label(&self) -> String {
        let name = match self.code {
            KeyCode::BackTab => return self.with_mods("Shift+Tab", "Ctrl+", "Alt+", ""),
            KeyCode::F(n) => format!("F{n}"),
            KeyCode::Char(c) if c != ' ' => {
                if self.mods.contains(KeyModifiers::CONTROL) {
                    c.to_ascii_uppercase().to_string()
                } else {
                    c.to_string()
                }
            }
            code => {
                NAMES.iter().find(|(_, _, k)| *k == code).map_or_else(|| format!("{code:?}"), |(_, d, _)| d.to_string())
            }
        };
        self.with_mods(&name, "Ctrl+", "Alt+", "Shift+")
    }

    /// Config notation: `ctrl+e`, `shift+tab`, `space`, `G`.
    pub fn notation(&self) -> String {
        let name = match self.code {
            KeyCode::BackTab => return self.with_mods("shift+tab", "ctrl+", "alt+", ""),
            KeyCode::F(n) => format!("f{n}"),
            KeyCode::Char(c) if c != ' ' => c.to_string(),
            code => NAMES
                .iter()
                .find(|(_, _, k)| *k == code)
                .map_or_else(|| format!("{code:?}").to_lowercase(), |(n, _, _)| n.to_string()),
        };
        self.with_mods(&name, "ctrl+", "alt+", "shift+")
    }
}

/// Why a key in config notation could not be read (the UI turns it into a catalog message).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyError {
    /// No key at all (an empty string, or `ctrl+`).
    Empty,
    /// A shifted character written as `shift+…` (the part as written).
    Shifted(String),
    /// A name that is not a key (the part as written).
    Unknown(String),
}

/// Developer text (key map checks); the UI uses the catalog.
impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyError::Empty => f.write_str("empty key"),
            KeyError::Shifted(t) => write!(f, "write “{t}” as the character it types"),
            KeyError::Unknown(t) => write!(f, "unknown key “{t}”"),
        }
    }
}

fn parse_chord(tok: &str) -> Result<KeyChord, KeyError> {
    let mut mods = KeyModifiers::NONE;
    let mut rest = tok;
    loop {
        let lower = rest.to_ascii_lowercase();
        let m = [("ctrl+", KeyModifiers::CONTROL), ("alt+", KeyModifiers::ALT), ("shift+", KeyModifiers::SHIFT)]
            .into_iter()
            .find(|(p, _)| lower.starts_with(p) && rest.len() > p.len());
        match m {
            Some((p, m)) => {
                mods |= m;
                rest = &rest[p.len()..];
            }
            None => break,
        }
    }
    let mut chars = rest.chars();
    let code = match (chars.next(), chars.next()) {
        (Some(c), None) => {
            let shift = mods.contains(KeyModifiers::SHIFT);
            if shift && !mods.contains(KeyModifiers::CONTROL) && !c.is_ascii_alphabetic() {
                return Err(KeyError::Shifted(tok.to_string()));
            }
            KeyCode::Char(c)
        }
        (None, _) => return Err(KeyError::Empty),
        _ => {
            let lower = rest.to_ascii_lowercase();
            let name = ALIASES.iter().find(|(a, _)| *a == lower).map_or(lower.as_str(), |(_, n)| n);
            if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).filter(|n| (1..=24).contains(n))
            {
                KeyCode::F(n)
            } else {
                NAMES
                    .iter()
                    .find(|(n, _, _)| *n == name)
                    .map(|(_, _, k)| *k)
                    .ok_or_else(|| KeyError::Unknown(rest.to_string()))?
            }
        }
    };
    Ok(KeyChord::new(code, mods))
}

/// Parse a key sequence in config notation (`ctrl+e`, `g t`, `space c n`).
pub fn parse_keys(s: &str) -> Result<Vec<KeyChord>, KeyError> {
    let keys = s.split(' ').filter(|t| !t.is_empty()).map(parse_chord).collect::<Result<Vec<_>, _>>()?;
    if keys.is_empty() {
        return Err(KeyError::Empty);
    }
    Ok(keys)
}

/// Display form of a sequence: `Space c n`.
pub fn label(keys: &[KeyChord]) -> String {
    keys.iter().map(KeyChord::label).collect::<Vec<_>>().join(" ")
}

/// Config notation of a sequence: `space c n`.
pub fn notation(keys: &[KeyChord]) -> String {
    keys.iter().map(KeyChord::notation).collect::<Vec<_>>().join(" ")
}
