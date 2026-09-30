//! `datarig` binary: parses the command line, owns the terminal and runs the event loop.

mod bench;
mod cli;
mod term;

use cli::{Cli, HELP, parse_args};
use datarig_core::secret::{DeletionLog, FileStore, STORE_ENV, StoreKind};
use datarig_core::{config, i18n};
use datarig_tui::app::{App, Startup};
use datarig_tui::screens;
use datarig_tui::terminal::{Cursor, cursor_shape};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::EventStream;
use std::io::{self, IsTerminal, Stdout, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

fn main() -> ExitCode {
    let stats = bench::Stats::from_env();
    let (cli_path, profile) = match parse_args(std::env::args().skip(1)) {
        Ok(Cli::Run { config, profile }) => (config, profile),
        Ok(Cli::Help) => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Ok(Cli::Version) => {
            println!("datarig {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    // Before anything else: which password store this process may touch.
    let store = match StoreKind::from_env(std::env::var(STORE_ENV).ok().as_deref()) {
        Ok(kind) => kind,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    log::debug!("secret store: {}", store.as_str());
    let (cfg, cfg_err) = config::load(cli_path);
    let lang = i18n::detect_lang(&cfg.language, |k| std::env::var(k).ok());
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    // Every thread's panic comes here (the hook is process-wide): the terminal is restored
    // before the message is printed. The one exception is the classifier's parse thread, whose
    // panic is caught and only makes the statement unreadable (it asks, or a read-only policy
    // refuses it); the app goes on, so the screen stays as it is and nothing is printed over it.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some(datarig_core::sql::risk::THREAD) {
            return;
        }
        term::restore_terminal();
        default_hook(info);
    }));
    let result = rt.block_on(async {
        let mut app = App::new(&cfg, cfg_err, lang);
        let paths = datarig_core::paths::Paths::from_env();
        let log = Arc::new(DeletionLog::new(paths.secrets_log()));
        let secrets_file = cfg.path.as_deref().map(FileStore::next_to);
        app.set_secret_stores(store.open_stores(secrets_file, log));
        app.set_paths(paths);
        // Whether the icons show is asked once, in a terminal only.
        app.set_ask_icons(std::io::stdout().is_terminal());
        app.set_clipboard(datarig_tui::clipboard::system());
        // SSH tunnels.
        app.set_tunnels(Arc::new(datarig_tui::app::tunnel::SshTunnels));
        // The terminal first: keychain access may wait on an OS permission dialog, and the
        // user must see why. `start` never blocks on the keychain (the password move runs in
        // the background behind a notice; a direct connection waits for the first frame).
        // Restores the terminal however this block ends (also a setup that fails part way).
        let _restore = term::guard();
        // The background decides the variant of a theme family; asked before the terminal is set
        // up and the event stream reads stdin.
        app.set_background(term::background());
        let (mut terminal, enhanced) = term::setup_terminal()?;
        app.set_keyboard_enhanced(enhanced);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        app.start(tx, profile.map_or(Startup::Normal, Startup::Profile));
        run(&mut terminal, app, rx, stats).await
    });
    // Do not wait for in-flight DB tasks (e.g. a running slow query) on exit.
    rt.shutdown_background();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    mut app: App,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<datarig_tui::app::AppEvent>,
    mut stats: Option<bench::Stats>,
) -> io::Result<()> {
    let mut events = EventStream::new();
    let mut signals = Signals::new()?;
    let mut cursor = Cursor::default();
    let mut first_frame = true;
    // A frame is drawn after anything that may change the screen; a mouse move (reported
    // with every motion) changes nothing unless it selects another item of a menu or the help.
    let mut redraw = true;
    while !app.quit {
        if redraw {
            // A block in Normal and Visual, a bar in Insert and text inputs (unless
            // `editor.cursor_shape = off`).
            cursor.apply(&mut io::stdout(), cursor_shape(&app))?;
            // OSC 52 copies go to the terminal between frames.
            let out = app.take_terminal_output();
            if !out.is_empty() {
                let mut stdout = io::stdout();
                for s in out {
                    stdout.write_all(s.as_bytes())?;
                }
                stdout.flush()?;
            }
            terminal.draw(|f| screens::draw(f, &mut app))?;
            if let Some(s) = stats.as_mut() {
                s.drawn();
            }
            if first_frame {
                first_frame = false;
                app.first_frame_drawn();
                continue;
            }
        }
        if let Some(s) = stats.as_mut() {
            s.woke();
        }
        if wait(&mut app, &mut events, &mut rx, &mut signals).await? {
            break;
        }
        redraw = !app.take_idle_event();
    }
    if let Some(s) = stats.as_mut() {
        s.write();
    }
    Ok(())
}

/// Wait for the next input event, background event or timer deadline ([`App::next_tick`]) and
/// hand it to the app. `true` when the input stream ended.
async fn wait(
    app: &mut App,
    events: &mut EventStream,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<datarig_tui::app::AppEvent>,
    signals: &mut Signals,
) -> io::Result<bool> {
    let deadline = app.next_tick(Instant::now());
    let timer = async {
        match deadline {
            Some(at) => tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await,
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        ev = events.next() => match ev {
            Some(Ok(ev)) => app.handle_event(ev),
            Some(Err(e)) => return Err(e),
            None => return Ok(true),
        },
        Some(ev) = rx.recv() => {
            app.on_app_event(ev);
            while let Ok(ev) = rx.try_recv() {
                app.on_app_event(ev);
            }
        }
        _ = timer => app.on_tick(Instant::now()),
        // Asked to end from outside: quit like `:qa!` after writing what can be written; the
        // loop ends and the terminal is restored on the way out, as on a normal quit.
        _ = signals.recv() => app.quit_on_signal(),
    }
    Ok(false)
}

/// The signals that end the process (Unix): SIGTERM, SIGHUP (the terminal went away), SIGINT
/// (sent from outside: in raw mode Ctrl+C is a key) and SIGQUIT. Once caught they no longer
/// kill the process on the spot, so the terminal is restored and the workspace written first.
struct Signals {
    #[cfg(unix)]
    all: Vec<tokio::signal::unix::Signal>,
}

impl Signals {
    fn new() -> io::Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let kinds = [SignalKind::terminate(), SignalKind::hangup(), SignalKind::interrupt(), SignalKind::quit()];
            Ok(Self { all: kinds.into_iter().map(signal).collect::<io::Result<_>>()? })
        }
        #[cfg(not(unix))]
        Ok(Self {})
    }

    /// The next of them.
    async fn recv(&mut self) {
        #[cfg(unix)]
        {
            let each = self.all.iter_mut().map(|s| Box::pin(s.recv()));
            futures::future::select_all(each).await;
        }
        #[cfg(not(unix))]
        std::future::pending::<()>().await
    }
}
