//! Terminal setup and restore for the binary: raw mode, then the modes of
//! [`datarig_tui::terminal::TermState`] (alternate screen, mouse, bracketed paste, the kitty
//! keyboard flags). One process-wide state is undone exactly once, by [`Guard`] (a normal exit,
//! an error, a failed setup) or by the panic hook, whichever comes first. Before any of it, the
//! terminal's background is asked for once ([`background`]).

use datarig_tui::terminal::{self, TermState};
use datarig_tui::theme::Background;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement};
use std::io::{self, IsTerminal, Stdout};
use std::time::Duration;

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

/// The longest the background query waits for an answer. A terminal that does not know the
/// query answers the device attributes query sent after it at once, so this only bounds one
/// that answers nothing (or a slow link); the first frame waits for it.
const BACKGROUND_WAIT: Duration = Duration::from_millis(100);

/// Whether the terminal's background is light or dark (OSC 11), asked once before the event
/// stream starts reading stdin. Not asked when stdout is not a terminal; no answer is
/// [`Background::Unknown`].
pub(crate) fn background() -> Background {
    if !io::stdout().is_terminal() {
        return Background::Unknown;
    }
    let mut options = terminal_colorsaurus::QueryOptions::default();
    options.timeout = BACKGROUND_WAIT;
    match terminal_colorsaurus::theme_mode(options) {
        Ok(terminal_colorsaurus::ThemeMode::Light) => Background::Light,
        Ok(terminal_colorsaurus::ThemeMode::Dark) => Background::Dark,
        Err(e) => {
            log::debug!("terminal background: {e}");
            Background::Unknown
        }
    }
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
