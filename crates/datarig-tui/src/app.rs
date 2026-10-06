//! Application state and update logic. No terminal I/O happens here: the binary feeds
//! crossterm events and driver events in, and `screens::draw` renders the state. Tests drive the
//! exact same entry points ([`App::handle_event`], [`App::on_db_event`], [`App::on_app_event`]).
//!
//! Keys and the `:` command line both resolve to an [`Action`] and run through
//! [`App::dispatch`]. Keys are resolved by the context keymap
//! ([`crate::keymap`]) in the context of [`App::key_context`].
//!
//! Startup: [`App::launch`] moves plaintext passwords from the config
//! file to the keychain and opens the workspace. With no profile the new-profile form opens over
//! the welcome panel; otherwise the explorer has the focus with its cursor on the last used
//! profile and nothing connects; `datarig <profile>` connects to that profile and opens a
//! console. With a running event loop ([`App::start`]) the password move runs off the UI thread
//! behind a notice, because the OS may hold the keychain call while it asks the user for
//! permission.
//!
//! Sessions: connecting to a profile opens its metadata session
//! ([`ConnectionManager`]: schema tree, completion catalog), one per profile. Several profiles can
//! be connected at once. Each workspace tab ([`TabManager`]) belongs to one profile and opens its
//! own query session when it first runs a statement, so transactions and cancel stay in their
//! tab. Driver events come back tagged with an [`EventTarget`] and the generation of the session
//! that sent them; events of closed or replaced sessions are dropped.

pub mod action;
pub mod chooser;
pub mod cmdline;
pub mod command;
mod conn;
mod connection;
pub mod copy;
mod ddl;
mod dispatch;
pub mod effects;
mod execution;
pub mod explorer;
mod format;
pub mod guide;
pub mod key_picker;
mod keychain;
pub mod menu;
pub mod overlay;
pub mod pages;
pub mod paging;
pub mod pane;
mod password;
mod persist;
pub mod plan;
pub mod presets;
pub mod profiles;
pub mod quick;
pub mod runlog;
pub mod safety;
mod script_ops;
pub mod script_tree;
pub mod settings;
mod tab_ops;
pub mod tabs;
pub mod themes;
pub mod tunnel;
mod update;

pub use conn::{AUX_IDLE, AuxMeta, Connecting, ConnectionManager, Keys, NodeState, ProfileConn, Queued};
pub use paging::Paging;
pub use password::{EnvLookup, PromptPurpose, Secret};
pub use persist::fault_reason;
pub use tabs::{SessionState, Tab, TabId, TabKind, TabManager};

use crate::app::profiles::{FormOutcome, ProfileForm, copy_name};
use crate::drivers::{DriverLookup, driver_for};
use crate::input::hangul;
use crate::input::keyboard::{self, KeyUse};
use crate::input::kitty;
use crate::keymap::{Ctx, IssueKind, KeyChord, KeyError, KeyState, Keymap, Resolved};
use crate::widgets::editor::{EdEvent, Editor, Mode};
use crate::widgets::grid::{GridState, ResultSet, viewer_text};
use crate::widgets::text_input::TextInput;
use crate::widgets::tree::TreeAction;
use action::{Action, ExplorerAction, GridAction, LangSetting, REGISTRY};
use datarig_core::config::{self, Config, ConfigError, IconsSetting, Profiles, Settings};
use datarig_core::driver::{ConnectOptions, DbCommand, DbEvent, Outcome, PingError, PingInfo, Session, SessionRole};
use datarig_core::i18n::{I18n, Label, Lang, Localized, Msg, detect_lang};
use datarig_core::migrate::{self, Keychain};
use datarig_core::paths::Paths;
use datarig_core::policy::Policies;
use datarig_core::profile::folder::{FolderPath, Folders};
use datarig_core::profile::{ConnectionConfig, ProfileId};
use datarig_core::secret::{DefaultSource, MemoryStore, SecretStore, Secrets, SourceError, SourceKind, Stores};
use datarig_core::sql::complete::{Candidate, complete_in};
use datarig_core::sql::split::split;
use explorer::Explorer;
use overlay::{Busy, Confirm, ConfirmAction, Overlay, OverlayKind, Overlays};
pub use presets::PROBE_MAX;
use quick::QuickPurpose;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio::task::AbortHandle;

const TRANSIENT: Duration = Duration::from_secs(3);
/// The timer period while something counts in tenths of a second or animates.
pub const FINE_TICK: Duration = Duration::from_millis(100);
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const WHEEL_STEP: isize = 3;
/// Auto-completion waits this long after the last keystroke before popping up.
pub const COMPLETION_DEBOUNCE: Duration = Duration::from_millis(120);
/// Identifier characters typed before the popup opens by itself (`.` opens it at once).
pub const COMPLETION_MIN_PREFIX: usize = 2;
pub const TEST_TIMEOUT: Duration = Duration::from_secs(5);
/// A connection attempt gives up after this long (unreachable host, a server that accepts TCP
/// but never answers).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// After "quit anyway" the app waits this long for the running query to be cancelled before it
/// closes the connection regardless (the server then cancels and rolls back on its own).
pub const QUIT_GRACE: Duration = Duration::from_secs(3);
/// A cancelled statement whose session has not answered after this long (a wedged driver or
/// connection) is marked cancelled anyway, and the tab's connection is closed (the server
/// cancels and rolls back on its own; the next run connects again).
pub const CANCEL_GRACE: Duration = Duration::from_secs(5);
/// Autosave: a tab's text is written at most this long after an edit, and the
/// workspace state after a cursor or tab change.
pub const AUTOSAVE: Duration = Duration::from_secs(1);
/// Texts larger than this are saved when typing pauses (at most [`AUTOSAVE_LARGE`] late).
pub const LARGE_TEXT: usize = 1 << 20;
/// The longest a large text's edit waits for its autosave.
pub const AUTOSAVE_LARGE: Duration = Duration::from_secs(5);

/// What `datarig` was asked to do on launch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Startup {
    /// No arguments: the workspace, the explorer on the last used profile, nothing connected.
    Normal,
    /// `datarig <profile>`: connect to this profile and open a console.
    Profile(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Editor,
    Results,
    /// The result inspector panel (after a click on it); `Results` when it is not shown.
    Inspector,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Success,
    Warning,
    Error,
}

/// A catalog message with a severity. It keeps the message (not its text), so a language
/// switch re-renders it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub msg: Msg,
    pub level: Level,
}

impl Notice {
    pub fn new(msg: impl Into<Msg>, level: Level) -> Self {
        Self { msg: msg.into(), level }
    }
    pub fn render(&self, i18n: &I18n) -> Localized {
        i18n.msg(&self.msg)
    }
}

pub enum Results {
    Empty,
    Rows(ResultSet),
    Message(Notice),
    Error(String),
    Cancelled,
}

#[derive(Clone, Copy, Debug)]
pub struct Running {
    pub id: u64,
    pub started: Instant,
    pub fetch: bool,
    /// A count of the rows the user asked for.
    pub count: bool,
    /// When the user asked to cancel it (by the app's clock).
    pub cancelling: Option<Instant>,
}

pub struct Popup {
    pub items: Vec<Candidate>,
    pub selected: usize,
    pub replace_start: usize,
    /// Bytes after the cursor the accepted label replaces too (a closing `"`).
    pub trail: usize,
}

pub struct Viewer {
    pub column: String,
    pub text: String,
    pub scroll: usize,
    pub view_h: usize,
}

/// One entry of the command line's list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandItem {
    /// An action of [`REGISTRY`] (by index).
    Action(usize),
    /// A command of [`command::COMMANDS`] (by index).
    Command(usize),
    /// A completion of the argument of the command `command` (index into `COMMANDS`).
    Arg { command: usize, arg: command::ArgCompletion },
    /// The text typed, as an editor command (a line number, `:s`).
    Ex,
}

/// The `:` command line: an input line and the matching commands, arguments and actions (a
/// popup near the top by default, the last line with `commands.position = bottom`).
pub struct CommandLine {
    pub input: TextInput,
    /// Best match first.
    pub items: Vec<CommandItem>,
    pub selected: usize,
    /// Why the last `Enter` did nothing (unknown command, bad argument); typing clears it.
    pub error: Option<Notice>,
    /// `Tab`/`↑`/`↓` picked an entry since the input last changed. Until then `Enter` on a
    /// `:use` argument runs what was typed, never the top completion.
    pub picked: bool,
}

/// Password asked at connect time: a `prompt` profile (every time), or no/wrong stored
/// password (keychain unavailable, nothing saved yet).
/// What the prompt's field is called: a label, or text a server gave (a keyboard-interactive
/// question, shown as it is).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PromptField {
    Label(Label),
    Text(String),
}

impl From<Label> for PromptField {
    fn from(l: Label) -> Self {
        PromptField::Label(l)
    }
}

pub struct PasswordPrompt {
    pub id: ProfileId,
    /// The profile's name (the title).
    pub profile: String,
    pub input: TextInput,
    pub error: Notice,
    /// The "save" checkbox (saved only after the connection succeeds).
    pub save: bool,
    /// The checkbox has the focus (`Tab` toggles focus, `Space` toggles the box).
    pub save_focus: bool,
    /// Where the checkbox saves to (the profile's store); `None` hides the checkbox.
    pub save_to: Option<SourceKind>,
    /// Shown instead of the checkbox when there is none.
    pub note: Label,
    /// What `Enter` does with the password.
    pub purpose: PromptPurpose,
    /// The title when it is not the profile's database password (an SSH tunnel's secret).
    pub title: Option<Msg>,
    /// The field's name.
    pub field: PromptField,
    /// Show what is typed (a keyboard-interactive question that says so).
    pub echo: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestState {
    Running,
    Ok(PingInfo),
    Failed(String),
    Timeout(Duration),
    Cancelled,
    /// The password source failed before the server was asked.
    Source(SourceError),
    /// A tunnel preset's test: what it reached through the tunnel, each address with its time
    /// or why not (in words).
    Reached(Vec<(String, Result<Duration, String>)>),
}

pub struct ConnTest {
    pub seq: u64,
    pub started: Instant,
    pub state: TestState,
    abort: Option<AbortHandle>,
    /// Through the profile's SSH tunnel: how far that got.
    pub tunnel: Option<TunnelTest>,
    /// A tunnel preset's test: after the tunnel, channels to its profiles' database addresses
    /// are tried instead of a database.
    pub probe: bool,
}

/// The SSH tunnel of a test connection: each stage shows while it runs,
/// and the tunnel's time is reported apart from the database's.
pub struct TunnelTest {
    pub host: String,
    /// The tested profile's name and tunnel settings (the questions it asks name them).
    pub name: String,
    pub settings: datarig_core::profile::ssh::SshSettings,
    pub stage: Option<datarig_ssh::tunnel::Stage>,
    /// The tunnel opened after this long.
    pub opened: Option<Duration>,
    /// How it opened: the host key taken and the login that worked.
    pub how: Option<datarig_ssh::tunnel::Opened>,
    /// Why it did not open (the SSH stage's line).
    pub failure: Option<String>,
}

/// Events delivered to the app from background tasks.
#[derive(Debug)]
pub enum AppEvent {
    /// Driver event of session `generation` (events of replaced sessions are dropped), from the
    /// session `target` names.
    Db { target: EventTarget, generation: u64, ev: DbEvent },
    /// Result of test connection `seq`.
    Ping { seq: u64, result: Result<PingInfo, PingError> },
    /// The launch-time config and keychain migration finished.
    Migrated(migrate::Report),
    /// The password command of connection attempt `generation` of profile `profile` finished.
    Resolved { profile: ProfileId, generation: u64, result: Result<Secret, SourceError> },
    /// The password source of test connection `seq` failed.
    TestSource { seq: u64, error: SourceError },
    /// The launch-time keychain probe: `Some(reason)` when the keychain does not work.
    KeychainProbed(Option<datarig_core::fault::Fault>),
    /// A keychain call made on a worker answered.
    Keychain(keychain::KeychainDone),
    /// Profile `profile`'s tunnel of attempt `generation`.
    Tunnel { profile: ProfileId, generation: u64, ev: tunnel::TunnelEvent },
    /// The tunnel of test connection `seq`.
    TestTunnel { seq: u64, ev: tunnel::TunnelEvent },
    /// The shared connection `serial` of a tunnel preset.
    SharedTunnel { serial: u64, ev: tunnel::TunnelEvent },
    /// What test `seq` of a tunnel preset reached through it.
    TestProbe { seq: u64, probes: Vec<tunnel::Probe> },
}

/// Which session a driver event comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventTarget {
    /// The profile's metadata session (tree, catalog).
    Meta(ProfileId),
    /// A tab's query session (statements).
    Tab(TabId),
    /// A profile's metadata session in another database, by its id.
    Aux(u64),
}

/// Where the app reads the time for its timers and countdowns (tests use a fake one).
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

#[derive(Default, Clone, Copy, Debug)]
pub struct Layout {
    pub tree: Rect,
    pub editor: Rect,
    pub results: Rect,
    /// The inspector panel next to the results (empty when it is not shown).
    pub detail: Rect,
    /// The editor's text (inside its border, below a banner), as drawn last.
    pub editor_text: Rect,
    /// Where the editor's cursor was drawn last (the action menu opens there).
    pub editor_cursor: (u16, u16),
    /// The editor and the results pane together (a query tab), for resizing the pane.
    pub body: Rect,
    /// The results pane's top border while both panes are drawn: dragging it resizes them.
    pub divider: Rect,
    /// The result tab strip of a query tab's results pane.
    pub strip: Rect,
    /// The document tab bar above the panes (a click activates a tab).
    pub tab_bar: Rect,
    /// The previous and next page marks in the results title.
    pub page_prev: Rect,
    pub page_next: Rect,
    pub too_small: bool,
}

/// A left-button drag: where it started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    /// In the editor, from this text position (line, grapheme).
    Editor { anchor: (usize, usize) },
    /// In the grid, from this cell (row, column), selecting cells, whole rows (from the
    /// row-number gutter) or whole columns (from a header).
    Grid { anchor: (usize, usize), shape: crate::widgets::grid::Shape },
    /// The divider between the editor and the results pane.
    Divider,
}

/// The tabs of the result inspector.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DetailTab {
    /// The selected cell's whole value, its type and length.
    #[default]
    Cell,
    /// Every column of the selected row, `name: value`.
    Row,
}

impl DetailTab {
    pub fn other(self) -> Self {
        match self {
            DetailTab::Cell => DetailTab::Row,
            DetailTab::Row => DetailTab::Cell,
        }
    }
}

/// The result detail: shown or not (one key toggles it; `detail_view` decides
/// whether it is a panel or a status bar preview) and the panel's tab. It follows the grid's
/// selection.
#[derive(Clone, Copy, Debug)]
pub struct Detail {
    pub visible: bool,
    pub tab: DetailTab,
}

impl Default for Detail {
    fn default() -> Self {
        Self { visible: true, tab: DetailTab::Cell }
    }
}

pub struct App {
    pub i18n: I18n,
    /// The theme the frames are drawn with (`crate::theme::cur` while drawing).
    pub theme: Arc<crate::theme::Theme>,
    /// `theme` as the config has it (saved back as it is, also when it does not resolve).
    pub theme_name: String,
    /// Why `theme_name` is not the theme drawn (`terminal` is, then).
    theme_problem: Option<Notice>,
    /// The terminal's background (a theme family's variant follows it).
    pub background: crate::theme::Background,
    pub page_size: usize,
    /// Rows of a result kept in memory, and the config's limit of a result's spill file (a
    /// policy may set its own).
    pub result_window_rows: usize,
    pub spill_limit: datarig_core::policy::SpillLimit,
    /// Where results spill (`<state>/spill`); `None` without a state directory (rows then stay
    /// in memory).
    pub spill: Option<Arc<datarig_core::results::spill::SpillDir>>,
    pub focus: Focus,
    /// The connection of each profile: metadata session, tree, catalog.
    pub conns: ConnectionManager,
    /// The workspace tabs: editor, results and query session each.
    pub tabs: TabManager,
    /// The explorer's cursor and filter.
    pub explorer: Explorer,
    /// All profiles (passwords excluded unless still in the file as legacy plaintext).
    pub profiles: Vec<ConnectionConfig>,
    /// The tunnel presets (`[tunnels.<name>]`), by name.
    pub presets: Vec<datarig_core::profile::tunnel::TunnelPreset>,
    /// The presets' shared SSH connections.
    pub shared: tunnel::SharedTunnels,
    pub status: Option<Notice>,
    pub transient: Option<(Notice, Instant)>,
    pub quit: bool,
    pub layout: Layout,
    pub keymap: Keymap,
    /// An unfinished key sequence (`g …`, `Space c …`).
    pub key_state: KeyState,
    /// When the pending leader sequence got its last key (the which-key popup follows).
    leader_at: Option<Instant>,
    /// The last input event changed nothing (see [`App::take_idle_event`]).
    idle_event: bool,
    /// Problems with the user's `[keymap.*]` entries, shown at launch.
    keymap_notices: Vec<Notice>,
    /// When Hangul was last read as QWERTY keys (the status bar shows the Hangul/Latin
    /// indicator `status.hangul` for a while).
    pub hangul_hint: Option<Instant>,
    pub lang_setting: LangSetting,
    /// `[secrets] default_source`: the password source a new profile starts with.
    pub default_source: DefaultSource,
    /// Step 2's settings: the command line's position, the result inspector's form, the
    /// clipboard and the header row of copies.
    pub prefs: datarig_core::config::Prefs,
    /// The result detail: shown or not, and the inspector's tab.
    pub detail: Detail,
    /// Opens the system clipboard (none until the binary sets the OS one).
    clipboard_opener: crate::clipboard::Opener,
    /// The system clipboard, once opened.
    clipboard: Option<Box<dyn crate::clipboard::SystemClipboard>>,
    /// Text for the terminal (OSC 52 copies) the binary writes before the next frame.
    terminal_out: Vec<String>,
    /// What the binary is to do with the terminal handed over (an external editor, a
    /// suspend).
    effect: Option<effects::Effect>,
    /// The tab the external editor is open for.
    editing: Option<effects::PendingEdit>,
    /// Why editor yanks did not reach the clipboard, as said so far.
    yank_notices: copy::YankNotices,
    /// A large copy waiting for its confirmation.
    pending_copy: Option<(copy::Intent, copy::CopyRequest)>,
    /// "Fetch every row, then copy" waiting for its confirmation: the format.
    pending_fetch_copy: Option<(copy::Intent, copy::CopyFormat)>,
    /// A copy waiting for the rest of a tab's rows.
    fetch_copy: Option<(copy::Intent, copy::CopyRequest)>,
    /// Why the config file could not be used (the settings screen says so; nothing is saved).
    config_problem: Option<Notice>,
    /// `[policy.<name>]` tables; the connected profile's policy applies to its tabs.
    pub policies: Policies,
    clock: Clock,
    pub secrets: Secrets,
    /// Dialogs on top of the workspace (command line, profile form, password prompt, …).
    pub overlays: Overlays,
    pub conn_test: Option<ConnTest>,
    /// The terminal speaks the kitty keyboard protocol (Ctrl+Enter is available).
    pub enhanced_keys: bool,
    /// Launch messages (migration result, key map problems, an unknown profile on the command
    /// line), shown in the results pane (or the welcome panel) until the next connection.
    pub notices: Vec<Notice>,
    /// Profile the explorer's cursor starts on (persisted as `last_used`).
    pub last_used: Option<ProfileId>,
    /// Every profile folder (nested, `/`-separated) and which are expanded.
    pub folders: Folders,
    /// `icons` as set (`auto`, `on`, `off`).
    pub icons: IconsSetting,
    /// Ask once whether the icons show when the config has not decided: the
    /// binary sets it when stdout is a terminal; tests never do (they set `icons`).
    ask_icons: bool,
    /// Format version of the config file (1 until the launch-time migration finishes). While
    /// it is 1, a password missing under `profile:<uuid>` is also looked up by profile name.
    pub config_version: u32,
    /// Save the password typed at the prompt for this profile, in this store, once it connects.
    save_on_connect: Option<(ProfileId, SourceKind)>,
    /// How long a keychain call may take.
    keychain_timeout: Duration,
    /// Keychain calls of one account run one at a time, in the order asked.
    keychain_queue: keychain::KeychainQueue,
    /// The keychain's guard, whose late answers come back through `tx`.
    keychain_guard: Option<Arc<datarig_core::secret::Guarded>>,
    /// Keychain accounts whose removal was refused while a call hung: removed when it returns.
    keychain_unremoved: Vec<String>,
    /// Password typed at the prompt of a `prompt` profile, used by the next attempt only.
    prompted: Option<(ProfileId, Secret)>,
    /// Password prompts of other profiles waiting for the open one (one prompt at a time).
    prompt_queue: VecDeque<(ProfileId, Notice)>,
    /// A profile form save waiting for the source-change confirmation.
    pending_save: bool,
    /// Environment variables (the `env` source); tests replace it.
    env: EnvLookup,
    /// The launch-time migration still to run.
    migration: Option<migrate::Input>,
    /// Data and state directories (the one-time notices live in the state directory).
    paths: Paths,
    /// The launch-time migration runs in the background; config saves wait for it
    /// (`persist_deferred`).
    migrating: bool,
    persist_deferred: bool,
    /// `datarig <profile>` waits for the first frame and the password move.
    deferred_startup: Option<Startup>,
    /// The event loop has drawn its first frame.
    drawn: bool,
    /// "Quit anyway" was confirmed while a query ran: quit once it is cancelled (or after
    /// [`QUIT_GRACE`]).
    quitting: Option<Instant>,
    auto_lang: Lang,
    config_path: Option<PathBuf>,
    config_broken: bool,
    /// Why the config file could not be read, for the error log (once the state directory is
    /// known).
    config_fault: Option<datarig_core::fault::Fault>,
    tx: Option<UnboundedSender<AppEvent>>,
    /// Drivers set by a test; `None`: the built-in registry.
    drivers: Option<DriverLookup>,
    /// How SSH tunnels open (the binary: real ones; tests: a fake); `None`: a profile with a
    /// tunnel cannot connect.
    tunnels: Option<Arc<dyn tunnel::Tunnels>>,
    /// Headless: the tunnel attempts asked for (profile, generation), for the tests to answer.
    pub tunnel_requests: Vec<(ProfileId, u64)>,
    /// Headless: the test connections that asked for a tunnel.
    pub test_tunnel_requests: Vec<u64>,
    /// Headless: the shared connections of presets asked for (by serial).
    pub shared_tunnel_requests: Vec<u64>,
    /// A secret typed for a preset's shared connection with "save", written once it opened:
    /// (store, secret) by serial.
    shared_saves: std::collections::HashMap<u64, (SourceKind, String)>,
    /// The explorer's "Tunnels" section is open.
    pub tunnels_expanded: bool,
    /// The presets whose profiles the explorer lists under them.
    pub tunnels_open: std::collections::BTreeSet<datarig_core::profile::tunnel::TunnelId>,
    /// The profile form's tunnel secret, applied once its save went through.
    pending_tunnel_secret: Option<tunnel::FormSecret>,
    /// Host key questions of tunnels, the first one shown.
    host_keys: VecDeque<tunnel::HostKeyWait>,
    /// Secrets tunnels wait for, the first one prompted.
    secret_waits: VecDeque<tunnel::SecretWait>,
    /// A tunnel secret typed with "save", written once its tunnel opened: (attempt, store,
    /// secret).
    tunnel_saves: std::collections::HashMap<ProfileId, (u64, SourceKind, String)>,
    /// Instance tag in the sessions' `application_name` (the process id; tests set their own).
    instance: String,
    test_seq: u64,
    query_seq: u64,
    /// Last DDL request id handed out (a DDL tab waits for the answer of its own).
    ddl_seq: u64,
    last_click: Option<(Instant, u16, u16)>,
    /// Clicks in a row at the same place (1, 2 for a double click, 3 for a triple click).
    clicks: u8,
    /// A left-button drag in progress.
    drag: Option<Drag>,
    /// Where the pointer was at the last move of a grid drag (the wheel extends from there).
    drag_at: Option<(u16, u16)>,
    /// Saved queries (`None` without a data directory).
    pub scripts: Option<datarig_core::scripts::ScriptStore>,
    /// The saved queries as last scanned (the explorer's "Saved queries" section).
    pub script_list: Vec<datarig_core::scripts::Entry>,
    /// The explorer's "Saved queries" section is open.
    pub scripts_expanded: bool,
    /// Open folders of the saved queries.
    pub script_folders: std::collections::BTreeSet<String>,
    /// The instance lock of this process (the workspace is ours while it is held).
    lock: Option<datarig_core::workspace::InstanceLock>,
    /// Another datarig holds the workspace: nothing is restored and neither
    /// `workspace.toml` nor the console files are written; saved queries still save. Also set
    /// once a `workspace.toml` of a later version was restored ([`App::workspace_newer`]).
    pub read_only: bool,
    /// The version of a `workspace.toml` written by a later datarig: its tabs were restored
    /// as far as this version knows them, and the workspace is not written (`read_only`), so
    /// nothing of it is lost; the banner says so.
    pub workspace_newer: Option<i64>,
    /// When the workspace state is written next (cursor moves, restored layout).
    workspace_due: Option<Instant>,
    /// The focus after the last input (a change of focus refreshes or connects).
    last_focus: Focus,
    /// Tabs of `workspace.toml` of a kind this version does not know: written back as they
    /// were read.
    unknown_tabs: Vec<String>,
    /// Where each result tab of the strip was drawn last: its columns and the statement it
    /// shows (`None`: Messages).
    pub(crate) strip_hits: Vec<(u16, u16, Option<usize>, tabs::ResultView)>,
    /// The folder of the saved queries saved into last this run (the tree dialog's default).
    pub(crate) last_save_folder: Option<String>,
    /// The database and schema `:use` named for a tab, while the switch waits for its confirm.
    pending_context: Option<(TabId, datarig_core::driver::SessionContext)>,
    /// A `:use` whose name a list read earlier does not have, while the server is asked again.
    pending_use: Option<cmdline::PendingUse>,
    /// Where each tab and scroll mark of the document tab bar was drawn last.
    pub(crate) tab_hits: Vec<(u16, u16, crate::widgets::tabbar::TabHit)>,
}

fn test_msg(i18n: &I18n, t: &ConnTest) -> Notice {
    let (state, started) = (&t.state, t.started);
    // Through a tunnel: the tunnel's stage while it opens, then its time next to the database's
    // result, so it shows which one failed.
    if let Some(tt) = &t.tunnel {
        let host = tt.host.clone();
        match (state, tt.opened) {
            (TestState::Running, Some(ssh)) if t.probe => {
                return Notice::new(Msg::TestSshProbing { host, elapsed: ssh }, Level::Info);
            }
            (TestState::Reached(v), Some(ssh)) if v.is_empty() => {
                return Notice::new(Msg::TestTunnelOkNoTargets { host, elapsed: ssh }, Level::Success);
            }
            (TestState::Reached(v), Some(ssh)) => {
                return match reached_failures(v) {
                    None => Notice::new(
                        Msg::TestTunnelOk { host, elapsed: ssh, targets: reached_targets(v) },
                        Level::Success,
                    ),
                    Some(error) => Notice::new(Msg::TestTunnelProbeFailed { host, elapsed: ssh, error }, Level::Error),
                };
            }
            (TestState::Running, None) => {
                let stage = i18n.label(tunnel::stage_label(tt.stage.unwrap_or(datarig_ssh::tunnel::Stage::Connecting)));
                return Notice::new(
                    Msg::TestSshRunning { host, stage: stage.to_string(), elapsed: started.elapsed() },
                    Level::Info,
                );
            }
            (TestState::Running, Some(ssh)) => {
                return Notice::new(Msg::TestSshDb { host, elapsed: ssh }, Level::Info);
            }
            (TestState::Ok(info), Some(ssh)) => {
                return Notice::new(
                    Msg::TestOkSsh { host, elapsed: ssh, version: info.server_version.clone(), latency: info.latency },
                    Level::Success,
                );
            }
            (TestState::Failed(e), Some(ssh)) => {
                return Notice::new(Msg::TestFailedDb { host, elapsed: ssh, error: e.clone() }, Level::Error);
            }
            _ => {}
        }
    }
    match state {
        TestState::Running => Notice::new(Msg::TestRunning { elapsed: started.elapsed() }, Level::Info),
        TestState::Ok(info) => {
            Notice::new(Msg::TestOk { version: info.server_version.clone(), latency: info.latency }, Level::Success)
        }
        TestState::Failed(e) => Notice::new(Msg::TestFailed { error: e.clone() }, Level::Error),
        TestState::Timeout(d) => Notice::new(Msg::TestTimeout { elapsed: *d }, Level::Error),
        TestState::Cancelled => Notice::new(Label::TestCancelled, Level::Warning),
        TestState::Source(e) => Notice::new(password::source_error_msg(i18n, e), Level::Error),
        TestState::Reached(v) => match reached_failures(v) {
            None => Notice::new(Msg::TestReached { targets: reached_targets(v) }, Level::Success),
            Some(error) => Notice::new(Msg::TestFailed { error }, Level::Error),
        },
    }
}

/// The addresses a preset's test reached: `host:port, …`.
fn reached_targets(v: &[(String, Result<Duration, String>)]) -> String {
    v.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>().join(", ")
}

/// The addresses a preset's test did not reach, each with why (`None`: it reached them all).
fn reached_failures(v: &[(String, Result<Duration, String>)]) -> Option<String> {
    let failed: Vec<String> = v.iter().filter_map(|(t, r)| r.as_ref().err().map(|e| format!("{t}: {e}"))).collect();
    (!failed.is_empty()).then(|| failed.join("; "))
}

/// Startup notice for a `[keymap.*]` entry that was skipped (or replaced a default).
fn issue_msg(i: &crate::keymap::Issue) -> Notice {
    let (context, key) = (i.ctx.clone(), i.key.clone());
    let warn = |m: Msg| Notice::new(m, Level::Warning);
    match &i.kind {
        IssueKind::UnknownContext => warn(Msg::KeymapUnknownContext { context }),
        IssueKind::BadKey(KeyError::Empty) => warn(Msg::KeymapBadKeyEmpty { context, key }),
        IssueKind::BadKey(KeyError::Shifted(part)) => {
            warn(Msg::KeymapBadKeyShifted { context, key, part: part.clone() })
        }
        IssueKind::BadKey(KeyError::Unknown(part)) => {
            warn(Msg::KeymapBadKeyUnknown { context, key, part: part.clone() })
        }
        IssueKind::UnknownAction(a) => warn(Msg::KeymapUnknownAction { context, key, action: a.clone() }),
        IssueKind::TextKey => warn(Msg::KeymapTextKey { context, key }),
        IssueKind::Reserved => warn(Msg::KeymapReserved { context, key }),
        IssueKind::Protected => warn(Msg::KeymapProtected { context, key }),
        IssueKind::Prefix(other) => warn(Msg::KeymapPrefix { context, key, other: other.clone() }),
        IssueKind::Replaced { action, old } => {
            Notice::new(Msg::KeymapReplaced { context, key, action: action.clone(), old: old.clone() }, Level::Info)
        }
    }
}

fn is_ident_char(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

impl App {
    pub fn new(cfg: &Config, config_error: Option<ConfigError>, lang: Lang) -> Self {
        let (keymap, keymap_issues) = Keymap::with_user(&cfg.keymap);
        let status = config_error.as_ref().map(|e| {
            let error = I18n::new(lang).msg(&config_error_msg(&I18n::new(lang), e)).to_string();
            Notice::new(Msg::ErrorConfig { error }, Level::Error)
        });
        let config_fault = match &config_error {
            Some(ConfigError::Read { fault, .. } | ConfigError::Syntax(fault)) => Some(fault.clone()),
            _ => None,
        };
        // No tab yet: connecting to a profile opens its console, a restored
        // workspace brings its tabs back.
        let tabs = TabManager::default();
        let mut folders = cfg.folders.clone();
        for f in cfg.connections.iter().filter_map(ConnectionConfig::folder_path) {
            folders.insert(&f);
        }
        let mut app = Self {
            i18n: I18n::new(lang),
            theme: Arc::new(crate::theme::TERMINAL),
            theme_name: cfg.theme.clone(),
            theme_problem: None,
            background: crate::theme::Background::Unknown,
            page_size: cfg.page_size,
            result_window_rows: cfg.result_window_rows,
            spill_limit: cfg.spill_limit,
            spill: None,
            focus: Focus::Tree,
            conns: ConnectionManager::default(),
            tabs,
            explorer: Explorer::default(),
            profiles: cfg.connections.clone(),
            presets: cfg.tunnels.clone(),
            shared: tunnel::SharedTunnels::default(),
            status: status.clone(),
            transient: None,
            quit: false,
            layout: Layout::default(),
            keymap,
            key_state: KeyState::default(),
            leader_at: None,
            idle_event: false,
            keymap_notices: keymap_issues.iter().map(issue_msg).collect(),
            hangul_hint: None,
            lang_setting: LangSetting::parse(&cfg.language),
            default_source: cfg.default_source,
            prefs: cfg.prefs,
            detail: Detail::default(),
            clipboard_opener: crate::clipboard::none(),
            clipboard: None,
            terminal_out: Vec::new(),
            effect: None,
            editing: None,
            yank_notices: copy::YankNotices::default(),
            pending_copy: None,
            pending_fetch_copy: None,
            fetch_copy: None,
            config_problem: status.clone(),
            policies: cfg.policies.clone(),
            clock: Arc::new(Instant::now),
            secrets: Secrets::new(Arc::new(MemoryStore::new())),
            overlays: Overlays::default(),
            conn_test: None,
            enhanced_keys: false,
            notices: Vec::new(),
            last_used: cfg.last_used,
            folders,
            icons: cfg.icons,
            ask_icons: false,
            config_version: cfg.version,
            save_on_connect: None,
            keychain_timeout: datarig_core::secret::KEYCHAIN_TIMEOUT,
            keychain_queue: keychain::KeychainQueue::default(),
            keychain_guard: None,
            keychain_unremoved: Vec::new(),
            prompted: None,
            prompt_queue: VecDeque::new(),
            pending_save: false,
            env: password::process_env(),
            migration: (config_error.is_none() && cfg.needs_migration()).then(|| migrate::Input::new(cfg)),
            paths: Paths::default(),
            migrating: false,
            persist_deferred: false,
            deferred_startup: None,
            drawn: false,
            quitting: None,
            auto_lang: detect_lang("auto", |k| std::env::var(k).ok()),
            config_path: cfg.path.clone(),
            config_broken: config_error.is_some(),
            config_fault,
            tx: None,
            drivers: None,
            tunnels: None,
            tunnel_requests: Vec::new(),
            test_tunnel_requests: Vec::new(),
            shared_tunnel_requests: Vec::new(),
            shared_saves: std::collections::HashMap::new(),
            tunnels_expanded: true,
            tunnels_open: std::collections::BTreeSet::new(),
            pending_tunnel_secret: None,
            host_keys: VecDeque::new(),
            secret_waits: VecDeque::new(),
            tunnel_saves: std::collections::HashMap::new(),
            instance: std::process::id().to_string(),
            test_seq: 0,
            query_seq: 0,
            ddl_seq: 0,
            last_click: None,
            clicks: 0,
            drag: None,
            drag_at: None,
            scripts: None,
            script_list: Vec::new(),
            scripts_expanded: true,
            script_folders: std::collections::BTreeSet::new(),
            lock: None,
            read_only: false,
            workspace_newer: None,
            workspace_due: None,
            last_focus: Focus::Tree,
            unknown_tabs: Vec::new(),
            strip_hits: Vec::new(),
            tab_hits: Vec::new(),
            pending_context: None,
            pending_use: None,
            last_save_folder: None,
        };
        app.load_theme();
        app
    }

    /// Use `store` for passwords. The binary passes the store `DATARIG_SECRET_STORE` picks
    /// (the OS keychain unless it says `memory`); without this call the app keeps an empty
    /// in-memory store, which the tests rely on.
    pub fn set_secret_store(&mut self, store: Arc<dyn SecretStore>) {
        self.set_secret_stores(Stores::with_keychain(store));
    }

    /// Use `stores` for the keychain and the secrets file (the binary passes the stores
    /// `DATARIG_SECRET_STORE` picks). Every keychain call ends within the keychain's time
    /// limit ([`App::set_keychain_timeout`]).
    pub fn set_secret_stores(&mut self, mut stores: Stores) {
        let guard = keychain::guarded(stores.keychain, self.keychain_timeout);
        stores.keychain = guard.clone();
        self.keychain_guard = Some(guard);
        self.hear_late_keychain();
        self.secrets = Secrets::with_stores(stores);
    }

    /// Where the app keeps its data and state (the binary passes [`Paths::from_env`]; without
    /// this call nothing is written there).
    pub fn set_paths(&mut self, paths: Paths) {
        self.spill = paths.state.as_deref().map(|s| Arc::new(datarig_core::results::spill::SpillDir::new(s)));
        self.paths = paths;
        if let Some(f) = self.config_fault.take() {
            datarig_core::fault::ErrorLog::new(self.paths.errors_log()).record("error.config", &f);
        }
    }

    /// Where the `env` password source reads variables (the process environment unless a test
    /// replaces it).
    pub fn set_env_lookup(&mut self, env: EnvLookup) {
        self.env = env;
    }

    /// Use `drivers` instead of the built-in driver registry. Sessions of such drivers open
    /// headless too (tests read their command channels and feed their events).
    pub fn set_drivers(&mut self, drivers: DriverLookup) {
        self.drivers = Some(drivers);
    }

    /// The driver of `name` (a profile's `driver`).
    pub(crate) fn driver(&self, name: &str) -> Option<Arc<dyn datarig_core::driver::Driver>> {
        match &self.drivers {
            Some(d) => d(name),
            None => driver_for(name),
        }
    }

    /// The instance tag of the sessions' `application_name` (`datarig-<role>-<tag>`); the
    /// process id unless a test sets a unique one to find its connections on the server.
    pub fn set_instance_tag(&mut self, tag: &str) {
        self.instance = tag.to_string();
    }

    /// Read the time from `clock` (tests pass a fake one; `on_tick` still gets its time from
    /// the caller).
    pub fn set_clock(&mut self, clock: Clock) {
        self.clock = clock;
    }

    /// The current time of the app's clock.
    pub fn now(&self) -> Instant {
        (self.clock)()
    }

    /// `paging` of the policy of tab `t`'s profile: whether a result with more rows keeps its
    /// portal (and the transaction that holds it) open.
    pub fn paging_mode(&self, t: &Tab) -> datarig_core::driver::PagingMode {
        let policy = t.profile.and_then(|id| self.profile(id)).and_then(|c| c.policy.as_deref());
        self.policies.get(policy).paging
    }

    /// `paging_idle_timeout` of the policy of tab `t`'s profile (`None`: never close).
    pub fn paging_timeout(&self, t: &Tab) -> Option<Duration> {
        let policy = t.profile.and_then(|id| self.profile(id)).and_then(|c| c.policy.as_deref());
        self.policies.get(policy).paging_idle_timeout
    }

    /// How many rows of tab `t`'s results stay in memory and how large their spill file may
    /// grow (its profile's policy, else the config).
    pub fn result_limits(&self, t: &Tab) -> datarig_core::results::Limits {
        let policy = t.profile.and_then(|id| self.profile(id)).and_then(|c| c.policy.as_deref());
        let cap = self.policies.get(policy).spill_limit.unwrap_or(self.spill_limit);
        datarig_core::results::Limits { window: self.result_window_rows, cap: cap.0 }
    }

    /// Time left before the active tab's open portal is closed for being idle.
    pub fn paging_left(&self) -> Option<Duration> {
        self.tab().exec.paging.left(self.now(), self.paging_timeout(self.tab()))
    }

    /// Ask whether the icons show when the config has not decided (`icons = auto`, or no key),
    /// once the workspace is up ([`App::start`]). Only for a terminal.
    pub fn set_ask_icons(&mut self, ask: bool) {
        self.ask_icons = ask;
    }

    /// Nerd Font icons are drawn (`icons = on`; not decided yet draws text marks).
    pub fn icons_on(&self) -> bool {
        self.icons.on()
    }

    pub fn set_keyboard_enhanced(&mut self, on: bool) {
        self.enhanced_keys = on;
    }

    /// Key shown for "run statement" in hints.
    pub fn run_key(&self) -> &'static str {
        kitty::run_key_label(self.enhanced_keys)
    }

    /// Launch with background work enabled: events of connections, tests and the password move
    /// come back through `tx`. See [`App::launch`]. Nothing here waits for the keychain: the
    /// password move runs in the background and `datarig <profile>` connects only after
    /// [`App::first_frame_drawn`].
    pub fn start(&mut self, tx: UnboundedSender<AppEvent>, startup: Startup) {
        self.tx = Some(tx);
        self.hear_late_keychain();
        self.launch(startup);
    }

    /// Startup flow: the launch-time migration (config v2, keychain entries by id,
    /// plaintext passwords; see [`migrate`]), then the workspace with the explorer on the last
    /// used profile (the new-profile form when there is none), or a connection attempt to the
    /// profile named on the command line (an unknown name is reported in the workspace).
    /// Without `start`'s sender (headless tests) everything runs at once, and connection
    /// attempts are recorded but not made.
    pub fn launch(&mut self, startup: Startup) {
        let migration = self.migration.take();
        let background = migration.is_some() && self.tx.is_some();
        if let Some(input) = migration.clone().filter(|_| !background) {
            let stores = self.secrets.stores().clone();
            let report = migrate::run(stores.keychain.as_ref(), stores.file.as_ref(), input);
            let notices = self.migrated(report);
            self.notices.extend(notices);
        }
        self.notices.append(&mut self.keymap_notices);
        if let Some(n) = self.theme_problem.clone() {
            self.notices.push(n);
        }
        if let Some(e) = &self.status
            && matches!(e.msg, Msg::ErrorConfig { .. })
        {
            self.notices.push(e.clone());
        }
        if self.config_broken {
            // The profiles are unknown, not gone: every binding is kept as it is.
            self.notices.push(Notice::new(Label::ConfigConnectionsUnavailable, Level::Warning));
        }
        self.probe_keychain();
        self.open_workspace();
        if let (true, Some(tx), Some(input)) = (background, self.tx.clone(), migration) {
            self.migrating = true;
            self.overlays.push(Overlay::Busy(Busy { title: Label::MigrateTitle, text: Label::MigrateRunning }));
            let stores = self.secrets.stores().clone();
            tokio::task::spawn_blocking(move || {
                let report = migrate::run(stores.keychain.as_ref(), stores.file.as_ref(), input);
                let _ = tx.send(AppEvent::Migrated(report));
            });
        }
        match startup {
            Startup::Profile(_) if self.tx.is_some() => {
                self.deferred_startup = Some(startup);
                self.resume_startup();
            }
            s => self.run_startup(s),
        }
        // Not with a config that has errors: the answer could not be saved (asked on the first
        // launch after the file is fixed).
        if self.ask_icons && self.icons == IconsSetting::Auto && !self.config_broken {
            self.overlays.push(Overlay::IconsAsk(overlay::IconsAsk::default()));
        }
    }

    /// The event loop drew its first frame: a deferred `datarig <profile>` may connect now
    /// (its keychain read cannot freeze a blank screen any more).
    pub fn first_frame_drawn(&mut self) {
        self.drawn = true;
        self.resume_startup();
    }

    fn resume_startup(&mut self) {
        if self.drawn
            && !self.migrating
            && let Some(s) = self.deferred_startup.take()
        {
            self.run_startup(s);
        }
    }

    fn run_startup(&mut self, startup: Startup) {
        self.focus = Focus::Tree;
        let last = self.last_used.filter(|id| self.profile(*id).is_some()).or(self.profiles_sorted_first());
        if let Some(id) = last {
            self.reveal_profile(id);
        }
        match startup {
            // Nothing to connect to: the new-profile form opens over the welcome panel.
            Startup::Normal if self.profiles.is_empty() && !self.config_broken => self.open_form(None, false),
            Startup::Normal => {}
            Startup::Profile(name) => match self.profiles.iter().find(|p| p.name == name).map(|p| p.id) {
                Some(id) => {
                    self.reveal_profile(id);
                    let c = self.conns.entry(id);
                    c.expand_on_connect = true;
                    c.console = Some(false);
                    let notices = std::mem::take(&mut self.notices);
                    self.connect(id);
                    self.notices = notices;
                }
                None => self.notices.push(Notice::new(Msg::ProfileUnknown { name }, Level::Error)),
            },
        }
    }

    /// The first profile in explorer order.
    fn profiles_sorted_first(&self) -> Option<ProfileId> {
        self.explorer_rows().into_iter().find_map(|r| match r.kind {
            explorer::RowKind::Profile(id) => Some(id),
            _ => None,
        })
    }

    /// Apply the migration's outcome: passwords that moved leave the profiles, the config
    /// version is updated, and the notices to show are returned. Config saves requested while
    /// it ran happen now.
    pub(crate) fn migrated(&mut self, r: migrate::Report) -> Vec<Notice> {
        self.migrating = false;
        let mut out = Vec::new();
        for p in self.profiles.iter_mut().filter(|p| r.moved.contains(&p.id)) {
            p.password.clear();
        }
        self.config_version = r.version;
        if let Some(e) = r.save_failed {
            let error = self.fault_text("migrate.save_failed", &e);
            out.push(Notice::new(Msg::MigrateSaveFailed { error }, Level::Warning));
        }
        if let Some(e) = r.final_save_failed {
            let error = self.fault_text("config.save_failed", &e);
            out.push(Notice::new(Msg::ConfigSaveFailed { error }, Level::Error));
        }
        if let Some(bak) = &r.backup
            && r.version >= config::CONFIG_VERSION
        {
            out.push(Notice::new(Msg::MigrateDone { path: bak.display().to_string() }, Level::Info));
        }
        match &r.keychain {
            Keychain::Unavailable(e) => {
                self.secrets.unavailable = Some(e.clone());
                // Once per machine: the passwords simply stay in the file.
                let plaintext = self.profiles.iter().any(|p| !p.password.is_empty());
                if plaintext && self.paths.first_time("keychain_unavailable") {
                    let error = self.fault_text("secret.migrate_failed", e);
                    out.push(Notice::new(Msg::SecretMigrateFailed { error }, Level::Warning));
                }
            }
            Keychain::Partial => {
                let error = r
                    .failures
                    .iter()
                    .map(|(n, e)| format!("{n}: {}", self.fault_text("migrate.partial", e)))
                    .collect::<Vec<_>>()
                    .join("; ");
                out.push(Notice::new(Msg::MigratePartial { error }, Level::Warning));
            }
            Keychain::Done => {}
        }
        if !r.conflicts.is_empty() {
            let mut names = r.conflicts.clone();
            names.dedup();
            out.push(Notice::new(Msg::MigrateConflict { names: names.join(", ") }, Level::Warning));
        }
        if !r.moved.is_empty() {
            out.push(Notice::new(Msg::SecretMigrated { count: r.moved.len() as u64 }, Level::Success));
        }
        if std::mem::take(&mut self.persist_deferred)
            && let Some(m) = self.persist()
        {
            out.push(m);
        }
        out
    }

    /// The background migration reported back: close its notice, show the result in the
    /// workspace and the status bar, and go on with a deferred `datarig <profile>`.
    pub(super) fn on_migrated(&mut self, r: migrate::Report) {
        self.overlays.close(OverlayKind::Busy);
        let notices = self.migrated(r);
        if let Some(m) = notices.iter().max_by_key(|n| n.level as u8).cloned() {
            self.flash(m);
        }
        for (i, m) in notices.into_iter().enumerate() {
            self.notices.insert(i, m);
        }
        self.resume_startup();
    }

    /// The active tab.
    pub fn tab(&self) -> &Tab {
        self.tabs.active()
    }

    /// The active tab.
    pub fn tab_mut(&mut self) -> &mut Tab {
        self.tabs.active_mut()
    }

    /// The profile of the active tab.
    pub fn conn(&self) -> Option<&ConnectionConfig> {
        self.profile(self.tab().profile?)
    }

    /// The profile with id `id`.
    pub fn profile(&self, id: ProfileId) -> Option<&ConnectionConfig> {
        self.profiles.iter().find(|p| p.id == id)
    }

    /// The database tab `t` works in when it is not its profile's own: its
    /// catalog, keys and schemas come from a metadata session of their own.
    pub fn other_database<'a>(&self, t: &'a Tab) -> Option<&'a str> {
        let db = t.context.database.as_deref()?;
        let own = t.profile.and_then(|p| self.profile(p)).map(|c| c.endpoint().1);
        (own.as_deref() != Some(db)).then_some(db)
    }

    /// The keys that apply to tab `t`'s results: its database's (unknown while not read).
    pub fn tab_keys(&self, t: &Tab) -> &Keys {
        self.conns.keys_in(t.profile, self.other_database(t))
    }

    /// The search path completion resolves names in for tab `t`: its schema then `public`, else the server's default (`public`).
    pub fn tab_path(&self, t: &Tab) -> Vec<String> {
        match &t.context.schema {
            Some(s) => datarig_core::sql::complete::schema_path(s),
            None => datarig_core::sql::complete::default_path(),
        }
    }

    /// Tab `t`'s chosen schema is not on its session's path as the server says (it does not
    /// exist there, or the user may not use it): the editor's first line marks it
    /// for as long as that is so. Unknown (no session yet) is not missing.
    pub fn schema_missing(&self, t: &Tab) -> bool {
        let Some(schema) = &t.context.schema else { return false };
        t.exec.context.as_ref().is_some_and(|(_, schemas)| !schemas.contains(schema))
    }

    /// The connection of the active tab's profile.
    pub fn current_conn(&self) -> Option<&ProfileConn> {
        self.conns.get(self.tab().profile?)
    }

    /// A statement runs, or waits for its connection, in some tab.
    pub fn any_running(&self) -> bool {
        self.tabs.iter().any(|t| self.tab_busy(t.id))
    }

    /// The server reports a transaction open in some tab (the user's or one that pages).
    pub fn any_tx_open(&self) -> bool {
        self.tabs.iter().any(|t| t.exec.tx_open)
    }

    /// Some tab has a transaction whose rollback may lose something (`TabSession::tx_at_risk`).
    pub fn any_tx_at_risk(&self) -> bool {
        self.tabs.iter().any(|t| t.exec.tx_at_risk())
    }

    pub fn modal_open(&self) -> bool {
        self.overlays.modal()
    }

    /// Hangul was read as QWERTY keys a moment ago (the status bar shows the
    /// Hangul/Latin indicator `status.hangul`).
    pub fn hangul_hint_active(&self) -> bool {
        self.hangul_hint.is_some_and(|t| t.elapsed() < TRANSIENT)
    }

    /// The key context: the top dialog, else the focused pane.
    pub fn key_context(&self) -> Ctx {
        self.context_of(self.overlays.top())
    }

    /// The context under the command line (whose keys its list shows).
    pub fn context_below_commands(&self) -> Ctx {
        self.context_of(self.overlays.iter().rev().find(|o| o.kind() != OverlayKind::Commands))
    }

    fn context_of(&self, top: Option<&Overlay>) -> Ctx {
        match top {
            Some(Overlay::Commands(_)) => Ctx::Commands,
            Some(Overlay::Password(_)) => Ctx::Password,
            Some(Overlay::CellViewer(_)) => Ctx::CellViewer,
            Some(Overlay::WhichKey(_)) => Ctx::WhichKey,
            // Only quitting (and the root keys) while it waits.
            Some(Overlay::Busy(_)) => Ctx::Busy,
            Some(Overlay::Confirm(_)) => Ctx::Confirm,
            Some(Overlay::RunConfirm(_)) => Ctx::RunConfirm,
            Some(Overlay::IconsAsk(_)) => Ctx::IconsAsk,
            Some(Overlay::Help(h)) if h.filtering => Ctx::HelpFilter,
            Some(Overlay::Help(_)) => Ctx::Help,
            Some(Overlay::ProfileForm(_)) => Ctx::ProfileForm,
            Some(Overlay::Settings(_)) => Ctx::Settings,
            Some(Overlay::QuickConnect(_)) => Ctx::QuickConnect,
            Some(Overlay::NameInput(_)) => Ctx::NameInput,
            Some(Overlay::ScriptTree(t)) if t.focus == script_tree::TreeFocus::Name => Ctx::ScriptTreeName,
            Some(Overlay::ScriptTree(_)) => Ctx::ScriptTree,
            Some(Overlay::ContextMenu(_)) => Ctx::ContextMenu,
            Some(Overlay::Chooser(c)) if c.filtering => Ctx::ChooserFilter,
            Some(Overlay::Chooser(_)) => Ctx::Chooser,
            // Too small to draw the panes: only the workspace keys (run, new tab, …).
            None if self.layout.too_small => Ctx::Workspace,
            None => match self.focus {
                Focus::Tree if self.explorer.filtering => Ctx::ExplorerFilter,
                Focus::Tree => Ctx::Explorer,
                // Without profiles the welcome panel takes the place of the editor and results.
                _ if self.profiles.is_empty() => Ctx::Welcome,
                // No tab: the empty state has no keys of its own.
                _ if self.tabs.is_empty() => Ctx::Explorer,
                Focus::Results | Focus::Inspector if self.plan_shown() => Ctx::Plan,
                Focus::Results => Ctx::Grid,
                Focus::Inspector if self.inspector_shown() => Ctx::Inspector,
                Focus::Inspector => Ctx::Grid,
                Focus::Editor if self.tab().editor.searching() => Ctx::VimSearch,
                // A DDL tab's own keys, unless a vim command is being typed (a count, `f`, `m`).
                Focus::Editor
                    if self.tab().is_ddl()
                        && self.tab().editor.mode == Mode::Normal
                        && !self.tab().editor.command_started() =>
                {
                    Ctx::Ddl
                }
                Focus::Editor => match self.tab().editor.mode {
                    Mode::Normal => Ctx::VimNormal,
                    Mode::Insert => Ctx::VimInsert,
                    Mode::Visual => Ctx::VimVisual,
                },
            },
        }
    }

    /// Where each tab of the document tab bar was drawn last, and what it leads to (tests
    /// click them).
    pub fn tab_hits(&self) -> Vec<(u16, u16, crate::widgets::tabbar::TabHit)> {
        self.tab_hits.clone()
    }

    /// Where each result tab of the strip was drawn last, and what it shows (tests click them).
    pub fn strip_hits(&self) -> Vec<(u16, u16, Option<usize>, tabs::ResultView)> {
        self.strip_hits.clone()
    }

    /// The result inspector is drawn as a panel next to the grid (shown, `detail_view =
    /// panel`, a result with rows).
    pub fn inspector_shown(&self) -> bool {
        !self.tabs.is_empty()
            && self.detail.visible
            && self.prefs.detail_view == config::DetailView::Panel
            && self.tab().exec.view == tabs::ResultView::Rows
            && matches!(self.tab().results, Results::Rows(_))
    }

    pub fn needs_tick(&self) -> bool {
        self.any_running()
            || self.transient.is_some()
            || self.tab().completion_due.is_some()
            || self.conns.attempt().is_some()
            || self.hangul_hint.is_some()
            || self.leader_at.is_some()
            || self.quitting.is_some()
            || self.tabs.iter().any(|t| {
                matches!(t.exec.paging, Paging::Open { in_block: false, .. }) && self.paging_timeout(t).is_some()
            })
            || self.conn_test.as_ref().is_some_and(|t| t.state == TestState::Running)
            || self.save_pending()
            || self.conns.auxes().any(|a| self.aux_idle(a))
    }

    /// Aux session `a` is open and no tab works in its database: it closes
    /// [`conn::AUX_IDLE`] after its last use.
    pub fn aux_idle(&self, a: &conn::AuxMeta) -> bool {
        a.session.is_some()
            && !self.tabs.iter().any(|t| t.profile == Some(a.profile) && self.other_database(t) == Some(&a.database))
    }

    /// When the event loop should next run [`App::on_tick`] (no timer while
    /// nothing waits for time). While something counts in tenths of a second or animates (a
    /// running statement's time, a connection attempt, a debounce, a key sequence, a flash, the
    /// quit grace): [`FINE_TICK`] from `now`. Otherwise the earliest deadline: a tab's or the
    /// workspace's autosave, an open portal's idle close, and the next second the active tab's
    /// countdown shows. `None`: nothing waits.
    pub fn next_tick(&self, now: Instant) -> Option<Instant> {
        if !self.needs_tick() {
            return None;
        }
        let fine = self.any_running()
            || self.transient.is_some()
            || self.tab().completion_due.is_some()
            || self.conns.attempt().is_some()
            || self.hangul_hint.is_some()
            || self.leader_at.is_some()
            || self.quitting.is_some()
            || self.conn_test.as_ref().is_some_and(|t| t.state == TestState::Running);
        if fine {
            return Some(now + FINE_TICK);
        }
        let mut next: Option<Instant> = self.workspace_due;
        let mut at = |t: Instant| next = Some(next.map_or(t, |n| n.min(t)));
        for t in self.tabs.iter() {
            if let Some(due) = t.doc.save_due {
                at(due);
            }
            if let (Paging::Open { since, in_block: false }, Some(timeout)) = (t.exec.paging, self.paging_timeout(t)) {
                let due = since + timeout;
                at(due);
                if t.id == self.tab().id {
                    // The countdown shows whole seconds rounded up: it changes when the time left
                    // crosses a whole second.
                    let left = due.saturating_duration_since(now);
                    let frac = Duration::from_nanos(u64::from(left.subsec_nanos()));
                    let step = if frac.is_zero() { Duration::from_secs(1) } else { frac };
                    at(now + step);
                }
            }
        }
        for a in self.conns.auxes().filter(|a| self.aux_idle(a)) {
            at(a.used + conn::AUX_IDLE);
        }
        // Never sooner than a fine tick, never in the past.
        next.map(|n| n.max(now + Duration::from_millis(1)))
    }

    /// Timer work (debounced auto-completion, connect timeout, which-key popup, idle paging
    /// portals). Called on every tick with the current time.
    pub fn on_tick(&mut self, now: Instant) {
        self.which_key_tick(now);
        self.autosave_tick(now);
        // Aux sessions no tab works in, unused for a while, close; what they read stays.
        let idle: Vec<u64> = self
            .conns
            .auxes()
            .filter(|a| self.aux_idle(a) && now.saturating_duration_since(a.used) >= conn::AUX_IDLE)
            .map(|a| a.id)
            .collect();
        for id in idle {
            if let Some(s) = self.conns.aux_by_id(id).and_then(|a| a.session.take()) {
                s.close();
            }
        }
        let idle: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|t| t.exec.running.is_none() && t.exec.paging.due(now, self.paging_timeout(t)))
            .map(|t| t.id)
            .collect();
        for id in idle {
            self.close_idle_portal(id);
        }
        let unanswered: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|t| {
                t.exec
                    .running
                    .and_then(|r| r.cancelling)
                    .is_some_and(|at| now.saturating_duration_since(at) >= CANCEL_GRACE)
            })
            .map(|t| t.id)
            .collect();
        for id in unanswered {
            self.cancel_unanswered(id);
        }
        if self.quitting.is_some_and(|t| now.saturating_duration_since(t) >= QUIT_GRACE) {
            self.finish_quit();
        }
        if self.hangul_hint.is_some_and(|t| now.saturating_duration_since(t) >= TRANSIENT) {
            self.hangul_hint = None;
        }
        // An attempt opening its tunnel is not timed here (each step of the tunnel has a limit
        // of its own, which stops while the user answers a question).
        let late: Vec<ProfileId> = self
            .conns
            .connecting()
            .filter(|(_, c)| c.tunnel.is_none() && now.saturating_duration_since(c.started) >= CONNECT_TIMEOUT)
            .map(|(id, _)| id)
            .collect();
        for id in late {
            let m = Notice::new(Msg::ConnTimeout { elapsed: CONNECT_TIMEOUT }, Level::Error);
            self.abort_connect(id, m.clone());
            self.status = Some(m);
        }
        if let Some(due) = self.tab().completion_due
            && now >= due
        {
            self.tab_mut().completion_due = None;
            if self.focus == Focus::Editor && self.tab().editor.mode == Mode::Insert && !self.modal_open() {
                self.update_completion(false);
            }
        }
    }

    /// Message for the status bar: transient > running > last status.
    pub fn status_line(&mut self) -> Option<(Localized, Level, bool)> {
        if let Some((_, at)) = &self.transient
            && at.elapsed() >= TRANSIENT
        {
            self.transient = None;
        }
        if let Some((m, _)) = &self.transient {
            return Some((m.render(&self.i18n), m.level, false));
        }
        if let Some(r) = &self.tab().exec.running {
            let log = &self.tab().exec.run;
            let step = log.running().filter(|_| log.several() && !r.fetch);
            let text = if r.cancelling.is_some() {
                self.i18n.label(Label::QueryCancelling)
            } else if r.count {
                self.i18n.label(Label::ResultsTitleCounting)
            } else if r.fetch {
                self.i18n.label(Label::QueryFetching)
            } else if let Some(i) = step {
                let (step, total) = ((i + 1).to_string(), log.len().to_string());
                self.i18n.msg(&Msg::QueryRunningStep { step, total, elapsed: r.started.elapsed() })
            } else {
                self.i18n.msg(&Msg::QueryRunning { elapsed: r.started.elapsed() })
            };
            return Some((text, if r.cancelling.is_some() { Level::Warning } else { Level::Info }, true));
        }
        self.status.as_ref().map(|m| (m.render(&self.i18n), m.level, false))
    }

    /// Text of the test-connection state (the profile form's footer).
    pub fn test_status(&self) -> Option<(Localized, Level)> {
        // The form's line says that its save waits for the keychain.
        if self.overlays.form().is_some_and(|f| f.saving) {
            return Some((self.i18n.label(Label::FormSavingKeychain), Level::Info));
        }
        let t = self.conn_test.as_ref()?;
        let m = test_msg(&self.i18n, t);
        Some((m.render(&self.i18n), m.level))
    }

    /// The profile form's test lines through a tunnel: one line per stage,
    /// the SSH hop (its host key, the login and its time) then the database (its result and
    /// time); the stage that failed says why, and one that did not run says so. `None` without a
    /// tunnel (the one line of [`App::test_status`]) or while a save waits.
    pub fn test_lines(&self) -> Option<Vec<(Localized, Level)>> {
        if self.overlays.form().is_some_and(|f| f.saving) {
            return None;
        }
        let t = self.conn_test.as_ref()?;
        let tt = t.tunnel.as_ref()?;
        let host = tt.host.clone();
        let line = |m: Msg, level: Level| (self.i18n.msg(&m), level);
        let whole = || {
            let m = test_msg(&self.i18n, t);
            (m.render(&self.i18n), m.level)
        };
        let ssh = match (&t.state, tt.opened, &tt.failure) {
            (_, Some(elapsed), _) => match &tt.how {
                Some(how) => {
                    let (host_key, login) = self.opened_texts(how, &tt.settings);
                    line(Msg::TestLineSshOpened { host, host_key, login, elapsed }, Level::Success)
                }
                None => line(Msg::TestLineSshOk { host, elapsed }, Level::Success),
            },
            (TestState::Running, None, _) => {
                let stage = self
                    .i18n
                    .label(tunnel::stage_label(tt.stage.unwrap_or(datarig_ssh::tunnel::Stage::Connecting)))
                    .to_string();
                line(Msg::TestLineSshRunning { host, stage, elapsed: t.started.elapsed() }, Level::Info)
            }
            (_, None, Some(failure)) => (Localized::verbatim(failure.clone()), Level::Error),
            // Timed out, cancelled, or a source failed before the tunnel opened.
            (_, None, None) => whole(),
        };
        if t.probe {
            let probe = match (&t.state, tt.opened) {
                (TestState::Running, None) => line(Label::TestLineProbeWaiting.into(), Level::Info),
                (_, None) => line(Label::TestLineProbeNotRun.into(), Level::Warning),
                (TestState::Running, Some(_)) => line(Label::TestLineProbeRunning.into(), Level::Info),
                (TestState::Reached(v), Some(_)) if v.is_empty() => {
                    line(Label::TestLineProbeNone.into(), Level::Success)
                }
                (TestState::Reached(v), Some(_)) => match reached_failures(v) {
                    None => line(Msg::TestLineProbeOk { targets: reached_targets(v) }, Level::Success),
                    Some(error) => line(Msg::TestLineProbeFailed { error }, Level::Error),
                },
                (_, Some(_)) => whole(),
            };
            return Some(vec![ssh, probe]);
        }
        let db = match (&t.state, tt.opened) {
            (TestState::Running, None) => line(Label::TestLineDbWaiting.into(), Level::Info),
            (_, None) => line(Label::TestLineDbNotRun.into(), Level::Warning),
            (TestState::Running, Some(_)) => line(Label::TestLineDbRunning.into(), Level::Info),
            (TestState::Ok(info), Some(_)) => {
                line(Msg::TestLineDbOk { version: info.server_version.clone(), latency: info.latency }, Level::Success)
            }
            (TestState::Failed(error), Some(_)) => line(Msg::TestLineDbFailed { error: error.clone() }, Level::Error),
            (_, Some(_)) => whole(),
        };
        Some(vec![ssh, db])
    }

    /// The host key and login parts of a test's SSH line.
    fn opened_texts(
        &self,
        how: &datarig_ssh::tunnel::Opened,
        settings: &datarig_core::profile::ssh::SshSettings,
    ) -> (String, String) {
        use datarig_ssh::tunnel::Method;
        // The key's type as `ssh-keygen -l` names it (the line has little room).
        let algorithm = match how.host_key.as_str() {
            "ssh-ed25519" => "ED25519".to_string(),
            a if a.starts_with("ecdsa-sha2-") => "ECDSA".to_string(),
            "ssh-rsa" | "rsa-sha2-256" | "rsa-sha2-512" => "RSA".to_string(),
            a => a.to_string(),
        };
        let host_key = match how.trusted_now {
            true => Msg::TestHostKeyTrusted { algorithm },
            false => Msg::TestHostKeyKnown { algorithm },
        };
        let key = how.key.clone().unwrap_or_default();
        let login = match how.method {
            Method::Key => {
                // The file's name: the SSH section shows its path.
                let file = settings.key_file.as_deref().unwrap_or_default();
                let name = Path::new(file).file_name().map_or(file.into(), |n| n.to_string_lossy());
                self.i18n.msg(&Msg::TestLoginKey { file: name.into_owned(), key }).to_string()
            }
            Method::Agent => self.i18n.msg(&Msg::TestLoginAgent { key }).to_string(),
            Method::Password => self.i18n.label(Label::TestLoginPassword).to_string(),
            Method::KeyboardInteractive => self.i18n.label(Label::TestLoginAnswers).to_string(),
        };
        (self.i18n.msg(&host_key).to_string(), login)
    }

    fn flash(&mut self, m: Notice) {
        self.transient = Some((m, Instant::now()));
    }

    /// Show `m` in the status bar, unless the active tab's own warning is there and `m` is less
    /// severe: then the warning stays (it says what happened to that tab's work, e.g. a queued
    /// statement that was dropped) and `m` only flashes for a moment.
    fn show_status(&mut self, m: Notice) {
        let held = |n: &Notice| n.level as u8 >= Level::Warning as u8 && n.level as u8 > m.level as u8;
        let tab_warning = self.tab().status.as_ref().filter(|t| held(t));
        let keep = match (tab_warning, &self.status) {
            (Some(t), Some(s)) => t.msg == s.msg,
            _ => false,
        };
        if keep {
            self.flash(m);
        } else {
            self.status = Some(m);
        }
    }
}

/// The body of `error.config` for `e`, in `i18n`'s language (a fault inside as a short
/// reason).
pub fn config_error_msg(i18n: &I18n, e: &ConfigError) -> Msg {
    let reason = |f: &datarig_core::fault::Fault| i18n.msg(&persist::fault_reason(f)).to_string();
    match e {
        ConfigError::Read { path, fault } => {
            Msg::ConfigErrorRead { path: path.display().to_string(), reason: reason(fault) }
        }
        ConfigError::Syntax(f) => persist::fault_reason(f),
        ConfigError::Version(v) => {
            Msg::ConfigErrorVersion { version: v.to_string(), max: config::CONFIG_VERSION.to_string() }
        }
        ConfigError::Value { key, value, profile, allowed } => {
            let (key, value) = (key.clone(), value.clone());
            match (profile.clone(), allowed.map(str::to_string)) {
                (None, None) => Msg::ConfigErrorValue { key, value },
                (None, Some(allowed)) => Msg::ConfigErrorValueAllowed { key, value, allowed },
                (Some(profile), None) => Msg::ConfigErrorValueProfile { key, value, profile },
                (Some(profile), Some(allowed)) => Msg::ConfigErrorValueProfileAllowed { key, value, profile, allowed },
            }
        }
        ConfigError::DuplicateId(id) => Msg::ConfigErrorDuplicateId { id: id.clone() },
        ConfigError::Missing { key, profile, source } => {
            Msg::ConfigErrorMissing { key: key.to_string(), profile: profile.clone(), source: source.to_string() }
        }
        ConfigError::SshMissing { key, profile } => {
            Msg::ConfigErrorSshMissing { key: key.to_string(), profile: profile.clone() }
        }
        ConfigError::TunnelName(name) => Msg::ConfigErrorTunnelName { name: name.clone() },
        ConfigError::DuplicateTunnelId(id) => Msg::ConfigErrorDuplicateTunnelId { id: id.clone() },
        ConfigError::TunnelMissing { key, tunnel } => {
            Msg::ConfigErrorTunnelMissing { key: key.to_string(), tunnel: tunnel.clone() }
        }
    }
}
