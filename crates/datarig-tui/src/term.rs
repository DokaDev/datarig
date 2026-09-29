//! Terminal setup and restore for the binary: raw mode, then the modes of
//! [`datarig_tui::terminal::TermState`] (alternate screen, mouse, bracketed paste, the kitty
//! keyboard flags). One process-wide state is undone exactly once, by [`Guard`] (a normal exit,
//! an error, a failed setup) or by the panic hook, whichever comes first.

use datarig_tui::terminal::{self, TermState};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement};
use std::io::{self, Stdout};

static STATE: TermState = TermState::new();

fn raw_off() {
    let _ = disable_raw_mode();
}

/// Restore the terminal now (the panic hook, before the message is printed).
pub(crate) fn restore_terminal() {
    let _ = STATE.restore(&mut io::stdout(), raw_off);
}

/// Restores the terminal when dropped. Made before the terminal is set up, so a setup that
/// fails part way is undone as well.
pub(crate) type Guard = terminal::Guard<'static, Stdout, fn() -> Stdout>;

pub(crate) fn guard() -> Guard {
    terminal::Guard::new(&STATE, io::stdout, raw_off)
}

/// Raw mode, alternate screen, mouse, bracketed paste, and — when the terminal answers the
/// kitty keyboard protocol query — `DISAMBIGUATE_ESCAPE_CODES` so Ctrl+Enter is reported.
/// The query must run before the event stream starts reading stdin.
pub(crate) fn setup_terminal() -> io::Result<(Terminal<CrosstermBackend<Stdout>>, bool)> {
    enable_raw_mode()?;
    STATE.raw_on();
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    STATE.enter(&mut io::stdout(), enhanced)?;
    Ok((Terminal::new(CrosstermBackend::new(io::stdout()))?, enhanced))
}
