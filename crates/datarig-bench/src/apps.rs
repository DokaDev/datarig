//! The app without a terminal, as the benchmarks drive it: a [`TestBackend`] of the size the
//! measurements use, key and mouse events, and a real PostgreSQL connection when one is needed.

use datarig_core::config::Config;
use datarig_core::i18n::Lang;
use datarig_core::profile::ConnectionConfig;
use datarig_core::secret::{MemoryStore, SecretStore};
use datarig_tui::app::{App, AppEvent, Focus, Startup, TabKind};
use datarig_tui::screens;
use datarig_tui::widgets::editor::Editor;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

/// The terminal size of the frame measurements.
pub const WIDTH: u16 = 160;
pub const HEIGHT: u16 = 45;

pub fn terminal() -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("a test backend")
}

pub fn draw(term: &mut Terminal<TestBackend>, app: &mut App) {
    term.draw(|f| screens::draw(f, app)).expect("drawing into a test backend");
}

pub fn key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.handle_event(Event::Key(KeyEvent::new(code, modifiers)));
}

pub fn char(app: &mut App, c: char) {
    key(app, KeyCode::Char(c), KeyModifiers::NONE);
}

pub fn scroll(app: &mut App, down: bool, column: u16, row: u16) {
    let kind = if down { MouseEventKind::ScrollDown } else { MouseEventKind::ScrollUp };
    app.handle_event(Event::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE }));
}

/// An app with one profile (never connected) and one console holding `text`, the editor
/// focused. `state` is its state directory (autosave writes there).
pub fn offline(text: &str, state: Option<std::path::PathBuf>) -> App {
    let cfg = Config {
        connections: vec![ConnectionConfig { name: "offline".into(), ..ConnectionConfig::default() }],
        ..Config::default()
    };
    let mut app = App::new(&cfg, None, Lang::En);
    app.set_secret_store(Arc::new(MemoryStore::new()) as Arc<dyn SecretStore>);
    app.set_paths(datarig_core::paths::Paths { data: None, state });
    let pid = app.profiles[0].id;
    app.tabs.open(TabKind::Console, Some(pid), Editor::new(text));
    app.focus = Focus::Editor;
    app
}

/// An app connected to `url` through the explorer, like a user would: its console open. Its
/// state directory (where results spill) is `state`.
pub async fn connected(url: &str, state: &std::path::Path) -> Result<(App, UnboundedReceiver<AppEvent>), String> {
    let d = datarig_core::profile::dsn::parse(url).map_err(|e| format!("{e:?}"))?;
    let p = ConnectionConfig {
        name: "bench-pg".into(),
        host: d.host.clone(),
        port: d.port.unwrap_or(5432),
        user: d.user.clone(),
        password: String::new(),
        database: d.database.clone(),
        sslmode: "disable".into(),
        ..ConnectionConfig::default()
    };
    let store = Arc::new(MemoryStore::new());
    store.set(&p.id.account(), d.password.as_deref().unwrap_or_default()).map_err(|e| format!("{e:?}"))?;
    let cfg = Config { connections: vec![p], ..Config::default() };
    let mut app = App::new(&cfg, None, Lang::En);
    app.set_secret_store(store as Arc<dyn SecretStore>);
    app.set_instance_tag(&format!("bench{}", std::process::id()));
    app.set_paths(datarig_core::paths::Paths { data: None, state: Some(state.to_path_buf()) });
    let (tx, mut rx) = unbounded_channel();
    app.start(tx, Startup::Normal);
    let id = app.profiles[0].id;
    key(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    pump(&mut app, &mut rx, Duration::from_secs(15), |a| a.conns.is_connected(id) && !a.tabs.is_empty()).await?;
    Ok((app, rx))
}

/// Feed background events to the app until `done` holds.
pub async fn pump(
    app: &mut App,
    rx: &mut UnboundedReceiver<AppEvent>,
    within: Duration,
    done: impl Fn(&App) -> bool,
) -> Result<(), String> {
    let deadline = Instant::now() + within;
    while !done(app) {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, rx.recv()).await {
            Ok(Some(ev)) => app.on_app_event(ev),
            Ok(None) => return Err("the event channel closed".into()),
            Err(_) => return Err(format!("nothing happened within {within:?} (status {:?})", app.status)),
        }
    }
    Ok(())
}
