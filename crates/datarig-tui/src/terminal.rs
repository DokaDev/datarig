//! What the binary changes on the terminal and undoes: the alternate
//! screen, mouse capture, bracketed paste, the kitty keyboard flags and the cursor's shape.
//! The escape sequences are written to any writer, so tests check them without a terminal.
//!
//! The cursor takes the shape of the mode ([`cursor_shape`]): a steady block in the editor's
//! Normal and Visual modes, a steady bar in Insert and in every text input (command line,
//! filters, forms, prompts), unless `[editor] cursor_shape = "off"`. Whatever happens, the
//! terminal gets its user's default shape back: [`TermState::restore`] runs on a normal exit,
//! when an error ends the program, when setting the terminal up fails part way, and from the
//! panic hook ([`Guard`] and the binary's hook share one [`TermState`], and only the first call
//! writes). There is no suspend today (raw mode turns `Ctrl+Z` into a key); one would call
//! `restore` before it stops and set up again after.

use crate::app::App;
use crate::input::kitty;
use crate::widgets::editor::Mode;
use datarig_core::config::CursorShape as CursorSetting;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture};
use ratatui::crossterm::queue;
use ratatui::crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

/// A shape of the terminal's cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    /// A steady block: the editor's Normal and Visual modes.
    Block,
    /// A steady bar: Insert, and every text input.
    Bar,
}

impl CursorShape {
    fn style(self) -> SetCursorStyle {
        match self {
            CursorShape::Block => SetCursorStyle::SteadyBlock,
            CursorShape::Bar => SetCursorStyle::SteadyBar,
        }
    }
}

/// The cursor shape for what has the keyboard now, or `None` with
/// `[editor] cursor_shape = "off"` (the terminal's own shape). The editor shows its mode (a
/// bar in Insert, a block otherwise); any other text input (the command line, a filter, a form,
/// a prompt) a bar; everything else a block (the cursor is hidden there).
pub fn cursor_shape(app: &App) -> Option<CursorShape> {
    if app.prefs.cursor_shape == CursorSetting::Off {
        return None;
    }
    let ctx = app.key_context();
    let bar = if ctx.is_editor() { app.tab().editor.mode == Mode::Insert } else { ctx.is_text_input() };
    Some(if bar { CursorShape::Bar } else { CursorShape::Block })
}

/// The cursor shape last written to the terminal, so only a change is written.
#[derive(Debug, Default)]
pub struct Cursor {
    /// `None`: the terminal's own shape (never changed, or given back).
    set: Option<CursorShape>,
}

impl Cursor {
    /// Write what gives the cursor shape `want` (`None`: the user's default shape back, once
    /// a shape was set) to `out`, if it is not the shape already set.
    pub fn apply(&mut self, out: &mut impl Write, want: Option<CursorShape>) -> io::Result<()> {
        if self.set == want {
            return Ok(());
        }
        match want {
            Some(shape) => queue!(out, shape.style())?,
            None => queue!(out, SetCursorStyle::DefaultUserShape)?,
        }
        out.flush()?;
        self.set = want;
        Ok(())
    }
}

/// What the binary turned on, so it is undone exactly once however the program ends.
#[derive(Debug, Default)]
pub struct TermState {
    /// Raw mode is on (it is not an escape sequence: [`TermState::restore`] calls back).
    raw: AtomicBool,
    /// The alternate screen, mouse and paste modes were (or were being) turned on.
    entered: AtomicBool,
    /// The kitty keyboard flags were pushed and must be popped.
    keys: AtomicBool,
}

impl TermState {
    pub const fn new() -> Self {
        Self { raw: AtomicBool::new(false), entered: AtomicBool::new(false), keys: AtomicBool::new(false) }
    }

    /// Raw mode was turned on.
    pub fn raw_on(&self) {
        self.raw.store(true, Ordering::SeqCst);
    }

    /// Enter the alternate screen and turn on mouse capture and bracketed paste, and with
    /// `enhanced` push the kitty keyboard flags (the main and alternate screens keep separate
    /// flag stacks: pushed after entering, popped before leaving). Marked first, so a failure
    /// part way is undone too.
    pub fn enter(&self, out: &mut impl Write, enhanced: bool) -> io::Result<()> {
        self.entered.store(true, Ordering::SeqCst);
        queue!(out, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
        out.flush()?;
        if enhanced {
            self.keys.store(true, Ordering::SeqCst);
            out.write_all(kitty::push_sequence().as_bytes())?;
            out.flush()?;
        }
        Ok(())
    }

    /// Undo what was turned on, once (later calls do nothing): `raw_off` for raw mode, then on
    /// `out` the kitty flags popped, bracketed paste and mouse capture off, the user's default
    /// cursor shape and the main screen. Every step is tried even when one fails; the first
    /// failure comes back.
    pub fn restore(&self, out: &mut impl Write, raw_off: impl FnOnce()) -> io::Result<()> {
        if self.raw.swap(false, Ordering::SeqCst) {
            raw_off();
        }
        if !self.entered.swap(false, Ordering::SeqCst) {
            return Ok(());
        }
        let popped = match self.keys.swap(false, Ordering::SeqCst) {
            true => out.write_all(kitty::pop_sequence().as_bytes()),
            false => Ok(()),
        };
        let rest = queue!(
            out,
            DisableBracketedPaste,
            DisableMouseCapture,
            SetCursorStyle::DefaultUserShape,
            LeaveAlternateScreen
        );
        let flushed = out.flush();
        popped.and(rest).and(flushed)
    }
}

/// Restores a [`TermState`] when dropped: a normal return, an error returned with `?`, or a
/// panic unwinding through it. `out` gives the writer to restore on, `raw_off` turns raw mode
/// off.
pub struct Guard<'a, W: Write, O: FnMut() -> W> {
    state: &'a TermState,
    out: O,
    raw_off: fn(),
}

impl<'a, W: Write, O: FnMut() -> W> Guard<'a, W, O> {
    pub fn new(state: &'a TermState, out: O, raw_off: fn()) -> Self {
        Self { state, out, raw_off }
    }
}

impl<W: Write, O: FnMut() -> W> Drop for Guard<'_, W, O> {
    fn drop(&mut self) {
        let _ = self.state.restore(&mut (self.out)(), self.raw_off);
    }
}

#[cfg(test)]
mod tests;
