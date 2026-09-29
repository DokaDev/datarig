//! Where copies go: the system clipboard, or the terminal
//! through OSC 52 (it works over SSH; tmux passes it on with `set -g set-clipboard on`).
//!
//! `clipboard = auto` uses OSC 52 in an SSH session (`SSH_TTY` / `SSH_CONNECTION`), else the
//! system clipboard, and OSC 52 when the system clipboard cannot be used (a headless Linux
//! box). `system` and `osc52` use only that one.
//!
//! Terminals and tmux drop an OSC 52 sequence that is too long without a word, so a copy whose
//! base64 payload is longer than `osc52_max_bytes` is never sent that way: `auto` then uses the
//! system clipboard when there is one (also over SSH, where it is the remote machine's), and
//! otherwise the copy fails with a message that says why. The app never writes to the terminal itself:
//! an OSC 52 sequence waits in the app until the binary writes it out
//! ([`crate::app::App::take_terminal_output`]).

use datarig_core::config::ClipboardSetting;

/// How a copy reached the clipboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    System,
    Osc52,
}

/// What to try, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Only OSC 52.
    Osc52,
    /// Only the system clipboard (a failure is an error).
    System,
    /// The system clipboard, else OSC 52.
    SystemThenOsc52,
}

/// The plan for `setting`; `ssh`: the app runs in an SSH session.
pub fn plan(setting: ClipboardSetting, ssh: bool) -> Plan {
    match setting {
        ClipboardSetting::Auto if ssh => Plan::Osc52,
        ClipboardSetting::Auto => Plan::SystemThenOsc52,
        ClipboardSetting::System => Plan::System,
        ClipboardSetting::Osc52 => Plan::Osc52,
    }
}

/// The app runs in an SSH session: `SSH_TTY` or `SSH_CONNECTION` is set.
pub fn in_ssh(env: impl Fn(&str) -> Option<String>) -> bool {
    ["SSH_TTY", "SSH_CONNECTION"].iter().any(|k| env(k).is_some_and(|v| !v.is_empty()))
}

/// The system clipboard (a trait so tests can use a fake one and never touch the real one).
pub trait SystemClipboard: Send {
    /// Put `text` on the clipboard; the OS's reason when it cannot (for the error log).
    fn set_text(&mut self, text: &str) -> Result<(), String>;
}

/// Opens the system clipboard (the OS's reason when there is none).
pub type Opener = std::sync::Arc<dyn Fn() -> Result<Box<dyn SystemClipboard>, String> + Send + Sync>;

/// No system clipboard: what the app has until the binary gives it the real one (so tests and
/// headless runs never touch the user's clipboard).
pub fn none() -> Opener {
    std::sync::Arc::new(|| Err("no system clipboard was set up".to_string()))
}

struct Arboard(arboard::Clipboard);

impl SystemClipboard for Arboard {
    fn set_text(&mut self, text: &str) -> Result<(), String> {
        self.0.set_text(text).map_err(|e| e.to_string())
    }
}

/// The OS clipboard (arboard).
pub fn system() -> Opener {
    std::sync::Arc::new(|| {
        arboard::Clipboard::new().map(|c| Box::new(Arboard(c)) as Box<dyn SystemClipboard>).map_err(|e| e.to_string())
    })
}

/// The length of the base64 payload of `text`'s OSC 52 sequence (what `osc52_max_bytes`
/// limits).
pub fn osc52_len(text: &str) -> usize {
    text.len().div_ceil(3) * 4
}

/// The OSC 52 sequence that puts `text` on the terminal's clipboard.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ABC[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests;
