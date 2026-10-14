//! Keyboard enhancement (kitty keyboard protocol) setup shared by `main.rs` and tests.
//!
//! Only `DISAMBIGUATE_ESCAPE_CODES` is requested: it is enough to tell Ctrl+Enter from Enter
//! and Esc from Alt+key, while text keys keep arriving as plain UTF-8. That keeps Korean IME
//! commits working (with `REPORT_ALL_KEYS_AS_ESCAPE_CODES` text would arrive as key codes)
//! and means terminals do not send release/repeat events (the app filters them anyway).

use ratatui::crossterm::Command;
use ratatui::crossterm::event::{KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags};

pub fn enhancement_flags() -> KeyboardEnhancementFlags {
    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
}

/// The run-statement key advertised in the status bar hint.
pub fn run_key_label(enhanced: bool) -> &'static str {
    if enhanced { "Ctrl+Enter" } else { "Ctrl+E" }
}

/// Escape sequence written when the terminal supports the protocol.
pub fn push_sequence() -> String {
    let mut s = String::new();
    // Writing into a String cannot fail.
    let _ = PushKeyboardEnhancementFlags(enhancement_flags()).write_ansi(&mut s);
    s
}

/// Escape sequence written on exit and from the panic hook (only if pushed).
pub fn pop_sequence() -> String {
    let mut s = String::new();
    let _ = PopKeyboardEnhancementFlags.write_ansi(&mut s);
    s
}

#[cfg(test)]
mod tests;
