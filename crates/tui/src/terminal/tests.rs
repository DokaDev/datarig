use super::*;
use std::cell::Cell;
use std::sync::{Arc, Mutex};

/// A writer tests read back; `fail` makes its next write fail (a terminal that went away).
#[derive(Clone, Default)]
struct Screen {
    bytes: Arc<Mutex<Vec<u8>>>,
    fail: Arc<AtomicBool>,
}

impl Write for Screen {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if self.fail.swap(false, Ordering::SeqCst) {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "gone"));
        }
        self.bytes.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Screen {
    fn text(&self) -> String {
        String::from_utf8(self.bytes.lock().unwrap().clone()).unwrap()
    }
}

const BLOCK: &str = "\x1b[2 q";
const BAR: &str = "\x1b[6 q";
const DEFAULT_SHAPE: &str = "\x1b[0 q";
const ENTER: &str = "\x1b[?1049h";
const LEAVE: &str = "\x1b[?1049l";
const PASTE_OFF: &str = "\x1b[?2004l";
/// Mouse capture off. On Windows crossterm turns mouse capture on and off in the console mode
/// (WinAPI), never in the output, so there is nothing to find there.
const MOUSE_OFF: Option<&str> = if cfg!(windows) { None } else { Some("\x1b[?1000l") };

thread_local! {
    /// How often raw mode was turned off on this thread.
    static RAW_OFFS: Cell<usize> = const { Cell::new(0) };
}

fn raw_off() {
    RAW_OFFS.with(|c| c.set(c.get() + 1));
}

/// The terminal's whole restore, in order: the kitty flags popped, paste and mouse off, the
/// user's cursor shape, the main screen.
fn restore_sequence() -> String {
    let mut out = Vec::new();
    let state = TermState::new();
    state.enter(&mut Vec::new(), true).unwrap();
    state.restore(&mut out, || {}).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn restoring_pops_the_keys_and_gives_back_the_users_cursor_before_leaving() {
    let r = restore_sequence();
    assert!(r.starts_with(&kitty::pop_sequence()), "{r:?}");
    let at = |s: &str| r.find(s).unwrap_or_else(|| panic!("{s:?} missing from {r:?}"));
    assert!(at(PASTE_OFF) < at(DEFAULT_SHAPE) && MOUSE_OFF.is_none_or(|m| at(m) < at(DEFAULT_SHAPE)), "{r:?}");
    assert!(r.ends_with(&format!("{DEFAULT_SHAPE}{LEAVE}")), "{r:?}");
    // Without the kitty flags there is nothing to pop.
    let state = TermState::new();
    let mut out = Vec::new();
    state.enter(&mut Vec::new(), false).unwrap();
    state.restore(&mut out, || {}).unwrap();
    assert!(String::from_utf8(out).unwrap().starts_with(PASTE_OFF));
}

#[test]
fn the_cursor_shape_is_written_only_when_it_changes() {
    let mut out = Vec::new();
    let mut c = Cursor::default();
    for want in [Some(CursorShape::Bar), Some(CursorShape::Bar), Some(CursorShape::Block), None, None] {
        c.apply(&mut out, want).unwrap();
    }
    assert_eq!(String::from_utf8(out).unwrap(), format!("{BAR}{BLOCK}{DEFAULT_SHAPE}"));
    // Off from the start: the terminal's own shape is never touched.
    let mut out = Vec::new();
    Cursor::default().apply(&mut out, None).unwrap();
    assert!(out.is_empty());
}

/// How the binary runs: the guard first, then raw mode and the screen, then the event loop
/// (`body`), which sets cursor shapes on `out`.
fn program(
    state: &TermState,
    screen: &Screen,
    body: impl FnOnce(&mut Cursor, &mut Screen) -> io::Result<()>,
) -> io::Result<()> {
    let _guard = Guard::new(state, || screen.clone(), raw_off);
    state.raw_on();
    let mut out = screen.clone();
    state.enter(&mut out, true)?;
    let mut cursor = Cursor::default();
    body(&mut cursor, &mut out)
}

/// The screen ends with exactly one restore, after the shapes the program set, and raw mode
/// was turned off once.
fn assert_restored_once(screen: &Screen, raw_before: usize, what: &str) {
    let text = screen.text();
    let r = restore_sequence();
    // (A setup that failed before the kitty flags were pushed pops nothing.)
    let tail = &r[kitty::pop_sequence().len()..];
    assert!(text.ends_with(tail), "{what}: ends with the restore: {text:?}");
    for part in [Some(PASTE_OFF), MOUSE_OFF, Some(DEFAULT_SHAPE), Some(LEAVE)].into_iter().flatten() {
        assert_eq!(text.matches(part).count(), 1, "{what}: {part:?} once: {text:?}");
    }
    assert_eq!(RAW_OFFS.with(Cell::get), raw_before + 1, "{what}: raw mode off once");
}

#[test]
fn every_exit_path_gives_the_cursor_back() {
    let shapes = |c: &mut Cursor, out: &mut Screen| -> io::Result<()> {
        c.apply(out, Some(CursorShape::Bar))?;
        c.apply(out, Some(CursorShape::Block))
    };
    // A normal exit.
    let (state, screen, raw) = (TermState::new(), Screen::default(), RAW_OFFS.with(Cell::get));
    program(&state, &screen, shapes).unwrap();
    assert!(screen.text().starts_with(ENTER) && screen.text().contains(&format!("{BAR}{BLOCK}")));
    assert_restored_once(&screen, raw, "normal exit");

    // An error ends the event loop.
    let (state, screen, raw) = (TermState::new(), Screen::default(), RAW_OFFS.with(Cell::get));
    let failed = program(&state, &screen, |c, out| {
        shapes(c, out)?;
        Err(io::Error::other("the event stream failed"))
    });
    assert!(failed.is_err());
    assert_restored_once(&screen, raw, "error");

    // Writing a cursor shape fails (the terminal went away for a moment).
    let (state, screen, raw) = (TermState::new(), Screen::default(), RAW_OFFS.with(Cell::get));
    let failed = program(&state, &screen, |c, out| {
        c.apply(out, Some(CursorShape::Bar))?;
        out.fail.store(true, Ordering::SeqCst);
        c.apply(out, Some(CursorShape::Block))
    });
    assert!(failed.is_err());
    assert_restored_once(&screen, raw, "a failed write");

    // Setting the terminal up fails part way.
    let (state, screen, raw) = (TermState::new(), Screen::default(), RAW_OFFS.with(Cell::get));
    screen.fail.store(true, Ordering::SeqCst);
    assert!(program(&state, &screen, shapes).is_err());
    assert_restored_once(&screen, raw, "setup failed");

    // A panic unwinds through the guard.
    let (state, screen, raw) = (TermState::new(), Screen::default(), RAW_OFFS.with(Cell::get));
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = program(&state, &screen, |c, out| {
            shapes(c, out)?;
            panic!("a bug");
        });
    }));
    assert!(panicked.is_err());
    assert_restored_once(&screen, raw, "panic");

    // The panic hook restores first (so its message is readable); the guard then writes nothing.
    let (state, screen, raw) = (TermState::new(), Screen::default(), RAW_OFFS.with(Cell::get));
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = program(&state, &screen, |c, out| {
            shapes(c, out)?;
            // What the binary's panic hook does.
            state.restore(&mut screen.clone(), raw_off).unwrap();
            panic!("a bug");
        });
    }));
    assert!(panicked.is_err());
    assert_restored_once(&screen, raw, "panic hook, then the guard");
}

/// Handing the terminal to an external editor or suspending: `restore`, then raw mode and
/// `enter` again. A panic while it is handed over (the editor's path) leaves it restored and
/// writes nothing more; one after it was taken back restores it once more; and the screen
/// always ends restored, raw mode off as often as it was turned on.
#[test]
fn the_terminal_is_restored_after_a_panic_while_handed_over_or_after() {
    let handover = |state: &TermState, out: &mut Screen| -> io::Result<()> {
        state.restore(out, raw_off)?;
        Ok(())
    };
    let take_back = |state: &TermState, out: &mut Screen| -> io::Result<()> {
        state.raw_on();
        state.enter(out, true)
    };
    let count = |s: &Screen, part: &str| s.text().matches(part).count();

    // A panic while the editor has the terminal: the guard finds nothing to undo.
    let (state, screen, raw) = (TermState::new(), Screen::default(), RAW_OFFS.with(Cell::get));
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = program(&state, &screen, |_, out| {
            handover(&state, out)?;
            panic!("a bug in the editor's path");
        });
    }));
    assert!(panicked.is_err());
    assert_restored_once(&screen, raw, "panic while handed over");

    // Taken back, then a panic: restored again, once.
    let (state, screen, raw) = (TermState::new(), Screen::default(), RAW_OFFS.with(Cell::get));
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = program(&state, &screen, |_, out| {
            handover(&state, out)?;
            take_back(&state, out)?;
            panic!("a bug after the editor");
        });
    }));
    assert!(panicked.is_err());
    let text = screen.text();
    assert_eq!((count(&screen, ENTER), count(&screen, LEAVE)), (2, 2), "{text:?}");
    assert!(text.ends_with(&restore_sequence()), "ends restored, the kitty flags popped: {text:?}");
    assert_eq!(RAW_OFFS.with(Cell::get), raw + 2);

    // Taken back, then a normal exit.
    let (state, screen, raw) = (TermState::new(), Screen::default(), RAW_OFFS.with(Cell::get));
    program(&state, &screen, |_, out| {
        handover(&state, out)?;
        take_back(&state, out)
    })
    .unwrap();
    assert_eq!((count(&screen, ENTER), count(&screen, LEAVE)), (2, 2));
    assert!(screen.text().ends_with(&restore_sequence()));
    assert_eq!(RAW_OFFS.with(Cell::get), raw + 2);
}
