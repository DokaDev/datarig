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
pub mod chart;
pub mod chooser;
pub mod cmdline;
pub mod column_menu;
pub mod command;
mod conn;
mod conn_test;
mod connection;
mod context;
pub mod copy;
mod ddl;
mod detail;
mod dialog_mouse;
mod dispatch;
pub mod edit;
pub mod effects;
pub mod environments;
pub mod estimate;
mod execution;
pub mod explorer;
pub mod export;
pub mod export_form;
pub mod export_path_pick;
pub mod filter_dialog;
pub mod filter_history;
pub mod filter_value;
pub mod folder_pick;
pub mod folder_read;
mod format;
pub mod grid_search;
pub mod grid_summary;
pub mod guide;
pub mod hover;
pub mod idle;
pub mod key_picker;
mod keychain;
mod keychain_offer;
pub mod menu;
pub mod monitor;
pub mod overlay;
pub mod pages;
pub mod paging;
pub mod pane;
mod password;
mod persist;
pub mod picker;
pub mod plan;
mod pointer;
pub mod presets;
pub mod profiles;
pub mod quick;
mod run_hints;
pub mod runlog;
pub mod safety;
mod script_ops;
pub mod script_tree;
pub mod sessions;
pub mod settings;
mod startup;
mod status;
pub mod tab_drag;
pub mod tab_list;
mod tab_ops;
pub mod table_filter;
pub mod table_query;
mod table_view;
pub mod tabs;
pub mod themes;
mod tick;
pub mod tips;
pub mod tunnel;
pub mod tx;
mod update;
pub mod updates;
pub mod whats_new;
pub mod yank_flash;

pub use cmdline::{CommandItem, CommandLine};
pub use conn::{
    AUX_IDLE, AuxMeta, CHECKS_RETRY, ChecksCache, Connecting, ConnectionManager, Keys, NodeState, ProfileConn, Queued,
    TableChecks,
};
pub use conn_test::{ConnTest, TestState, TunnelTest};
use conn_test::{encryption_text, reached_failures, reached_targets, test_msg};
pub use detail::{Detail, DetailTab};
pub use paging::Paging;
pub use password::{EnvLookup, PasswordPrompt, PromptField, PromptPurpose, Secret};
pub use persist::fault_reason;
pub use tabs::{SessionState, Tab, TabId, TabKind, TabManager, ValuesWait};

use crate::app::profiles::{FormOutcome, ProfileForm, copy_name};
use crate::drivers::{DriverLookup, driver_for};
use crate::input::hangul;
use crate::input::keyboard::{self, KeyUse};
use crate::input::kitty;
use crate::keymap::{Ctx, IssueKind, KeyChord, KeyError, KeyState, Keymap, Resolved};
use crate::widgets::editor::{EdEvent, Editor, Mode};
use crate::widgets::grid::{GridState, ResultSet, viewer_text_of};
use crate::widgets::text_input::TextInput;
use crate::widgets::tree::TreeAction;
use action::{Action, ExplorerAction, GridAction, LangSetting, REGISTRY};
use datarig_core::config::{self, Config, ConfigError, IconsSetting, KeychainAccess, Profiles, Settings};
use datarig_core::driver::{ConnectOptions, DbCommand, DbEvent, Outcome, PingError, PingInfo, Session, SessionRole};
use datarig_core::migrate::{self, Keychain};
use datarig_core::paths::Paths;
use datarig_core::policy::Policies;
use datarig_core::profile::folder::{FolderPath, Folders};
use datarig_core::profile::{ConnectionConfig, ProfileId};
use datarig_core::secret::{DefaultSource, MemoryStore, SecretStore, Secrets, SourceError, SourceKind, Stores};
use datarig_core::sql::complete::{Candidate, Kind, complete_with};
use datarig_core::sql::dialect::{Dialect, Language};
use datarig_core::sql::risk::Classifier;
use datarig_core::tls::{Encryption, Verified};
use datarig_i18n::{I18n, Label, Lang, Localized, Msg, detect_lang};
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
/// Identifier characters typed before the popup opens by itself (`.` opens it at once).
pub const COMPLETION_MIN_PREFIX: usize = 2;
pub const TEST_TIMEOUT: Duration = Duration::from_secs(5);
/// A connection attempt gives up after this long (unreachable host, a server that accepts TCP
/// but never answers).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// After "quit anyway" the app waits this long for the running query to be cancelled before it
/// closes the connection regardless (the server then cancels and rolls back on its own).
pub const QUIT_GRACE: Duration = Duration::from_secs(3);
/// A cancelled statement whose session has said nothing at all for this long (a wedged driver)
/// is marked cancelled anyway, and the tab's connection is closed (the server cancels and rolls
/// back on its own; the next run connects again). The driver decides when to give a statement
/// up (MySQL: a kill not sent, or refused or taken while rows keep coming, at most some 20 s;
/// PostgreSQL: a cancel that cannot be sent, 10 s), and while it waits for a statement the
/// server stops (a long rollback, saying nothing) or will not stop (a refused cancel of a
/// statement that sends nothing) it says so every few seconds ([`DbEvent::Stopping`],
/// [`DbEvent::CancelRefused`]), each of which counts as a word: this bound is well past all of
/// them, a last resort only.
pub const CANCEL_GRACE: Duration = Duration::from_secs(60);
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
    Rows(Box<ResultSet>),
    Message(Notice),
    Error(String),
    Cancelled,
}

#[derive(Clone, Copy, Debug)]
pub struct Running {
    pub id: u64,
    pub started: Instant,
    pub fetch: bool,
    /// A count of the rows the user asked for (or, with `estimate`, the planner's estimate of
    /// them: as a count, not a run, the rows stay as they are).
    pub count: bool,
    /// The planner's estimate of the rows the user asked for (`count` is set too).
    pub estimate: bool,
    /// When the user asked to cancel it, or when its session last said anything since (by the
    /// app's clock): [`CANCEL_GRACE`] counts from it.
    pub cancelling: Option<Instant>,
    /// Its session said the server took the cancel and is still ending the statement
    /// ([`DbEvent::Stopping`]). Of this run only: each run starts with a `Running` of its own.
    pub stopping: bool,
    /// Its session said the server refused the cancel and the statement runs on to its end
    /// ([`DbEvent::CancelRefused`]). Of this run only, as `stopping`.
    pub refused: bool,
    /// A statement of an export to a file (`DbCommand::Export`): its rows go to the export's
    /// writer, never to the tab's result.
    pub export: bool,
    /// The Apply of a result's changes (`DbCommand::ApplyEdits`): no statement of a run.
    pub apply: bool,
}

pub struct Popup {
    pub items: Vec<Candidate>,
    pub selected: usize,
    /// Byte offset in its owner's text where the accepted label starts.
    pub replace_start: usize,
    /// Bytes after the cursor the accepted label replaces too (a closing `"`).
    pub trail: usize,
    /// The text the popup completes.
    pub owner: PopupOwner,
    /// The user turned to it (asked for it with `Ctrl+Space`, or moved its selection): in a
    /// field of the filter dialog `Enter` then takes its entry, else it applies the dialog.
    pub touched: bool,
}

/// The text a completion popup completes: the tab's editor, or a field of the filter dialog's
/// SQL section (`WHERE`, `ORDER BY`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupOwner {
    Editor,
    Dialog(table_query::Field),
}

pub struct Viewer {
    pub column: String,
    pub text: String,
    pub scroll: usize,
    pub view_h: usize,
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
    /// An export's writer (its file, how far, its end).
    Export(export::ExportEvent),
    /// A folder read of a file-system picker answered.
    Folder(folder_read::FolderAnswer),
    /// A search job of a result grid answered.
    GridSearch(grid_search::Answer),
    /// A summary job of a result grid's selection answered.
    GridSummary(grid_summary::SummaryAnswer),
    /// A read of the system clipboard answered.
    Clipboard(edit::ClipAnswer),
    /// The update check answered.
    UpdateChecked(datarig_core::update::Answer),
    /// The filter history's writer wrote the file, or failed to.
    FilterHistory(filter_history::Saved),
    /// The hidden columns' writer wrote (or failed to).
    TableView(filter_history::Saved),
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
    /// The throwaway query session of a session preset's test, by the test's serial.
    SessionTest(u64),
    /// The monitor connection of a Monitor tab.
    Monitor(TabId),
}

/// Where the app reads the time for its timers and countdowns (tests use a fake one).
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// Where the app reads the local time of day (hour, minute) a run's hint shows (tests use a
/// fixed one).
pub type TimeOfDay = Arc<dyn Fn() -> (u32, u32) + Send + Sync>;

/// The wall clock (`SystemTime::now`; tests pass their own).
pub type WallClock = Arc<dyn Fn() -> std::time::SystemTime + Send + Sync>;

/// The local time of day by the system's clock and time zone.
fn local_time_of_day() -> (u32, u32) {
    use chrono::Timelike;
    let now = chrono::Local::now();
    (now.hour(), now.minute())
}

#[derive(Default, Clone, Copy, Debug)]
pub struct Layout {
    /// The area the explorer and the tabs (their bar and panes) share, between the banner and
    /// the status bar: the explorer's width is measured against it.
    pub workspace: Rect,
    /// The explorer as drawn (the whole tab area when it is zoomed; empty when hidden).
    pub tree: Rect,
    /// The explorer's right border while it is drawn next to the panes: dragging it resizes it.
    pub explorer_edge: Rect,
    pub editor: Rect,
    pub results: Rect,
    /// The inspector panel next to the results (empty when it is not shown).
    pub detail: Rect,
    /// The entries of the inspector's Row tab, one line each from its first one on screen
    /// (empty when they are not drawn).
    pub detail_list: Rect,
    /// The editor's text (inside its border, below a banner), as drawn last.
    pub editor_text: Rect,
    /// Where the editor's cursor was drawn last (the action menu opens there).
    pub editor_cursor: (u16, u16),
    /// Where the cursor of the grid's search prompt was drawn last, while it is open.
    pub grid_prompt_cursor: Option<(u16, u16)>,
    /// The grid's rows and the lines under them, as drawn last (inside the results pane).
    pub grid_area: Rect,
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
    /// A table tab's filter and sort on the results' bottom border (a click opens the filter
    /// dialog).
    pub table_status: Rect,
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
    /// The explorer's right border.
    ExplorerEdge,
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
    /// How many colors the terminal draws (the cursor tints follow it).
    pub color_depth: crate::theme::ColorDepth,
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
    /// The session presets (`[sessions.<name>]`), by name.
    pub session_presets: Vec<datarig_core::profile::session::SessionPreset>,
    /// The presets' shared SSH connections.
    pub shared: tunnel::SharedTunnels,
    pub status: Option<Notice>,
    pub transient: Option<(Notice, Instant)>,
    /// Shorter forms of the notice `transient` holds when it is this message.
    transient_shorter: Option<(Msg, Vec<Msg>)>,
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
    /// Cells a copy just took from the result grid, flashing (see [`yank_flash`]).
    pub copy_flash: Option<yank_flash::CopyFlash>,
    /// The view of the Changes tab `s` chose for this run (`None`: `[ui] changes_view`).
    pub changes_view: Option<datarig_core::config::ChangesView>,
    /// The active tab's shown result and the revision of its changes as last seen, and since
    /// when: the counts of the Changes tab wait for the changes to settle.
    pub(crate) changes_seen: Option<edit::Seen>,
    /// The SQL view of the Changes tab as last laid out, and the model of the changes it is of.
    pub(crate) changes_body: Option<(std::sync::Arc<edit::ChangesModel>, edit::PreviewBody)>,
    /// The question the status bar asks in place of its segments (`y` does it).
    pub ask: Option<edit::StatusAsk>,
    /// An Apply waiting for the counts of its rows found by their values.
    pub apply_wait: Option<edit::ApplyWait>,
    /// A question (or a waiting Apply) ended without its answer: until when its answer keys are
    /// ignored, and what it said.
    pub(crate) ask_gone: Option<(Instant, Msg)>,
    /// Text pasted into the grid, waiting for its result's table to be known.
    pub(crate) paste_in: Option<edit::PasteIn>,
    /// The last paste into a grid: `space d V` reads it again while it is the newest change.
    pub(crate) last_paste: Option<edit::LastPaste>,
    /// The cells `y` or `Y` last copied in a grid, which `g p` pastes (kept for the session).
    pub(crate) grid_copy: Option<std::sync::Arc<edit::GridCopy>>,
    /// The reads of the system clipboard `space d v` makes.
    pub(crate) clip_reads: edit::ClipReads,
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
    /// A run that waits for the answer whether to drop the pending changes of its tab.
    pending_leave: Option<(TabId, Vec<String>)>,
    /// "Fetch every row, then copy" waiting for its confirmation: the format.
    pending_fetch_copy: Option<(copy::Intent, copy::CopyFormat)>,
    /// An `EXPLAIN ANALYZE` to run again in the plan format once confirmed, with the result it was
    /// asked from.
    pending_as_plan: Option<plan::AsPlan>,
    /// Whether the shown rows are a text plan, for the result it was worked out for.
    text_plan_cache: std::cell::Cell<Option<plan::TextPlan>>,
    /// A copy waiting for the rest of a tab's rows.
    fetch_copy: Option<(copy::Intent, copy::CopyRequest)>,
    /// The exports to files, one per tab at most.
    exports: export::Exports,
    /// The folder reads of the file-system pickers, on workers.
    folder_reads: folder_read::FolderReads,
    /// The search jobs of the result grids, on workers.
    grid_jobs: grid_search::GridJobs,
    grid_summary: grid_summary::GridSummary,
    /// Why the config file could not be used (the settings screen says so; nothing is saved).
    config_problem: Option<Notice>,
    /// The default rules and the environments (`[policy.<name>]`); a profile's environment
    /// applies to its tabs.
    pub policies: Policies,
    clock: Clock,
    time_of_day: TimeOfDay,
    wall: WallClock,
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
    /// What the offer to stop the macOS keychain prompts needs to know of the system (`None`:
    /// never offered; [`App::set_keychain_host`]).
    keychain_host: Option<datarig_core::secret::offer::Host>,
    /// The timed direct-mode keychain calls not looked at yet.
    keychain_seen: datarig_core::secret::offer::Seen,
    /// The offer waits for the connect to finish and the dialogs to close.
    keychain_offer_due: bool,
    /// Writing the keychain offer's state file failed once: not tried again this session.
    keychain_state_unwritable: bool,
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
    /// The explorer's "Connections" section (its folders and profiles) is open; kept in the
    /// workspace file.
    pub connections_expanded: bool,
    /// The presets whose profiles the explorer lists under them.
    pub tunnels_open: std::collections::BTreeSet<datarig_core::profile::tunnel::TunnelId>,
    /// The explorer's "Environments" section is open.
    pub environments_expanded: bool,
    /// The explorer lists the profiles without an environment under the default rules.
    pub default_rules_open: bool,
    /// The environments whose profiles the explorer lists under them.
    pub environments_open: std::collections::BTreeSet<datarig_core::policy::EnvironmentId>,
    /// The explorer's "Session presets" section is open.
    pub sessions_expanded: bool,
    /// The session presets whose profiles the explorer lists under them.
    pub sessions_open: std::collections::BTreeSet<datarig_core::profile::session::SessionPresetId>,
    /// What each session preset's statements are, checked once each time the presets change.
    pub(crate) session_checks: sessions::Checks,
    /// The session of a session preset's test that runs, with what it tests.
    pub(crate) session_test: Option<sessions::SessionTest>,
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
    /// The last request of a table's keys for its order (`DbCommand::LoadTableKeys`).
    order_keys_seq: u64,
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
    /// The folder of the saved queries saved into last this run (the tree dialog's default).
    pub(crate) last_save_folder: Option<String>,
    /// The database and schema `:use` named for a tab, while the switch waits for its confirm.
    pending_context: Option<(TabId, datarig_core::driver::SessionContext)>,
    /// A question's answer waiting for open transactions to be committed or rolled back first.
    pending_end: Option<tx::PendingEnd>,
    /// A `:use` whose name a list read earlier does not have, while the server is asked again.
    pending_use: Option<cmdline::PendingUse>,
    /// The small clickable target under the pointer (drawn highlighted).
    pub pointer_on: Option<hover::PointerOn>,
    /// The tips: usage counts, the page they turn with, each place's pick ([`tips`]).
    pub tips: tips::TipState,
    /// The filters and orders table tabs ran, per table ([`filter_history`]).
    filter_history: filter_history::FilterHistoryState,
    table_view: table_view::TableViewState,
    /// The values loads of tables showed in this session, for the filter dialog's lists.
    value_memory: filter_dialog::values::Memory,
    /// What's new does not show by itself at launch, nor is anything written for it (the tests
    /// of other screens set it; `:whatsnew` still works).
    pub whats_new_paused: bool,
    /// The update check of this run ([`updates`]).
    pub updates: updates::Updates,
    /// Pointer state: clicks, drags, and where tabs and result tabs were drawn last (hit tests).
    pub(crate) pointer: pointer::PointerState,
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
            color_depth: crate::theme::ColorDepth::TrueColor,
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
            session_presets: cfg.sessions.clone(),
            shared: tunnel::SharedTunnels::default(),
            status: status.clone(),
            transient: None,
            transient_shorter: None,
            quit: false,
            layout: Layout::default(),
            keymap,
            key_state: KeyState::default(),
            leader_at: None,
            idle_event: false,
            keymap_notices: keymap_issues.iter().map(issue_msg).collect(),
            hangul_hint: None,
            copy_flash: None,
            changes_view: None,
            changes_seen: None,
            changes_body: None,
            ask: None,
            apply_wait: None,
            ask_gone: None,
            paste_in: None,
            last_paste: None,
            grid_copy: None,
            clip_reads: Default::default(),
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
            pending_leave: None,
            pending_fetch_copy: None,
            pending_as_plan: None,
            text_plan_cache: Default::default(),
            fetch_copy: None,
            exports: export::Exports::default(),
            folder_reads: Default::default(),
            grid_jobs: Default::default(),
            grid_summary: Default::default(),
            config_problem: status.clone(),
            policies: cfg.policies.clone(),
            clock: Arc::new(Instant::now),
            time_of_day: Arc::new(local_time_of_day),
            wall: Arc::new(std::time::SystemTime::now),
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
            keychain_host: None,
            keychain_seen: Default::default(),
            keychain_offer_due: false,
            keychain_state_unwritable: false,
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
            connections_expanded: true,
            tunnels_open: std::collections::BTreeSet::new(),
            environments_expanded: true,
            default_rules_open: false,
            environments_open: std::collections::BTreeSet::new(),
            sessions_expanded: true,
            sessions_open: std::collections::BTreeSet::new(),
            session_checks: sessions::Checks::of(&cfg.sessions),
            session_test: None,
            pending_tunnel_secret: None,
            host_keys: VecDeque::new(),
            secret_waits: VecDeque::new(),
            tunnel_saves: std::collections::HashMap::new(),
            instance: std::process::id().to_string(),
            test_seq: 0,
            query_seq: 0,
            ddl_seq: 0,
            order_keys_seq: 0,
            pointer: Default::default(),
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
            pointer_on: None,
            tips: tips::TipState::default(),
            filter_history: Default::default(),
            table_view: Default::default(),
            value_memory: Default::default(),
            whats_new_paused: false,
            updates: updates::Updates::default(),
            pending_context: None,
            pending_end: None,
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
    /// limit ([`App::set_keychain_timeout`]); on macOS the OS keychain's direct calls are timed
    /// ([`App::set_keychain_host`]).
    pub fn set_secret_stores(&mut self, mut stores: Stores) {
        // The keychain is reached the way the loaded config says (macOS only).
        stores.access.set(self.prefs.keychain_access);
        let guard = keychain::guarded(stores.keychain.clone(), self.keychain_timeout);
        self.keychain_guard = Some(guard.clone());
        stores.keychain = self.timed_keychain(guard, &stores);
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
    /// headless too (tests read their command channels and feed their events). Open tabs take
    /// the language of their profile's driver in the new registry.
    pub fn set_drivers(&mut self, drivers: DriverLookup) {
        self.drivers = Some(drivers);
        let tabs: Vec<TabId> = self.tabs.iter().map(|t| t.id).collect();
        for t in tabs {
            self.sync_tab_language(t);
        }
    }

    /// The driver of `name` (a profile's `driver`).
    pub(crate) fn driver(&self, name: &str) -> Option<Arc<dyn datarig_core::driver::Driver>> {
        match &self.drivers {
            Some(d) => d(name),
            None => driver_for(name),
        }
    }

    /// The language tab `tab`'s text is in: its profile's driver's
    /// (`Capabilities::language`), as its session said it reads it (`DbEvent::Language`: a
    /// MySQL session's sql mode), else as the profile's last session said; the default for a
    /// tab without a profile, or whose profile or driver is unknown.
    pub fn tab_language(&self, tab: TabId) -> Language {
        let t = self.tabs.get(tab);
        let profile = t.and_then(|t| t.profile);
        let driver = self.profile_language(profile);
        // Only a language of the driver's dialect: the profile may have changed its driver.
        let same = |l: &Language| std::mem::discriminant(&l.dialect()) == std::mem::discriminant(&driver.dialect());
        let last = profile.and_then(|p| self.conns.get(p)).and_then(|c| c.language);
        t.and_then(|t| t.exec.language).filter(same).or(last.filter(same)).unwrap_or(driver)
    }

    /// The SQL dialect the app writes in for tab `tab`: its classifier's language (what its
    /// statements are checked in), [`App::tab_language`] for a tab that is not there.
    pub(crate) fn tab_dialect(&self, tab: TabId) -> Dialect {
        self.tabs.get(tab).map_or_else(|| self.tab_language(tab), |t| t.exec.prepared.language()).dialect()
    }

    /// Whether tab `tab`'s server gives a statement's plan ([`App::tab_explain`]), which the
    /// plan actions need.
    pub(crate) fn tab_explains(&self, tab: TabId) -> bool {
        self.tab_explain(tab).is_some()
    }

    /// The plan format tab `tab`'s server gives: as a session of its profile said
    /// (`DbEvent::Explain`: none on MariaDB), else its driver's (`Capabilities::explain`). A
    /// tab without a profile, or whose profile or driver is unknown, counts as PostgreSQL's:
    /// its text is in the default language, PostgreSQL ([`App::tab_language`]), so the plan
    /// actions stay offered as they always were (running one asks for a connection first).
    pub(crate) fn tab_explain(&self, tab: TabId) -> Option<datarig_core::driver::ExplainFormat> {
        let profile = self.tabs.get(tab).and_then(|t| t.profile);
        let Some(caps) = self.profile_capabilities(profile) else {
            return Some(datarig_core::driver::ExplainFormat::PostgresJson);
        };
        // Only a driver with a format has servers that say theirs.
        let said = profile.and_then(|p| self.conns.get(p)).and_then(|c| c.explain);
        caps.explain.and(said.unwrap_or(caps.explain))
    }

    /// The language of a tab bound to `profile` ([`App::tab_language`]), before the tab exists.
    pub(crate) fn profile_language(&self, profile: Option<ProfileId>) -> Language {
        self.profile_capabilities(profile).map_or_else(Language::default, |c| c.language)
    }

    /// Whether `profile`'s server has one level of namespaces (`Hierarchy::SchemaOnly`, MySQL: a
    /// database is the schema, and a session moves between them): the explorer lists them under
    /// the profile, and a tab works in one of them (its context's schema).
    pub fn schema_only(&self, profile: Option<ProfileId>) -> bool {
        self.profile_capabilities(profile).is_some_and(|c| c.hierarchy == datarig_core::driver::Hierarchy::SchemaOnly)
    }

    /// The capabilities of `profile`'s driver; `None` without a profile, or when the profile
    /// or its driver is unknown.
    fn profile_capabilities(&self, profile: Option<ProfileId>) -> Option<datarig_core::driver::Capabilities> {
        let profile = profile.and_then(|id| self.profile(id))?;
        self.driver(&profile.driver).map(|d| d.capabilities())
    }

    /// Tab `tab`'s editor and risk classifier follow its language ([`App::tab_language`]), once
    /// the tab appears or its binding (or its profile's driver) changes. Another language gets
    /// a new classifier, with nothing prepared.
    pub(crate) fn sync_tab_language(&mut self, tab: TabId) {
        let lang = self.tab_language(tab);
        let Some(t) = self.tabs.get_mut(tab) else { return };
        t.editor.set_language(lang);
        if t.exec.prepared.language() != lang {
            t.exec.prepared = Classifier::new(lang);
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

    /// Read the local time of day from `time_of_day` (tests pass a fixed one).
    pub fn set_time_of_day(&mut self, time_of_day: TimeOfDay) {
        self.time_of_day = time_of_day;
    }

    /// Read the wall clock from `wall` (tests pass one that moves with their fake clock).
    pub fn set_wall_clock(&mut self, wall: WallClock) {
        self.wall = wall;
    }

    /// The local time of day as a run's hint shows it (`14:03`).
    pub fn time_of_day(&self) -> String {
        let (h, m) = (self.time_of_day)();
        format!("{h:02}:{m:02}")
    }

    /// The current time of the app's clock.
    pub fn now(&self) -> Instant {
        (self.clock)()
    }

    /// The rules of tab `t`'s profile ([`App::rules_of`]; the default rules without one).
    fn tab_rules(&self, t: &Tab) -> datarig_core::policy::Policy {
        match t.profile {
            Some(id) => self.rules_of(id),
            None => self.policies.default_rules().clone(),
        }
    }

    /// `paging` of the environment of tab `t`'s profile: whether a result with more rows keeps
    /// its portal (and the transaction that holds it) open.
    pub fn paging_mode(&self, t: &Tab) -> datarig_core::driver::PagingMode {
        self.tab_rules(t).paging
    }

    /// `paging_idle_timeout` of the environment of tab `t`'s profile (`None`: never close).
    pub fn paging_timeout(&self, t: &Tab) -> Option<Duration> {
        self.tab_rules(t).paging_idle_timeout
    }

    /// How many rows of tab `t`'s results stay in memory and how large their spill file may
    /// grow (its profile's environment, else the config).
    pub fn result_limits(&self, t: &Tab) -> datarig_core::results::Limits {
        let cap = self.tab_rules(t).spill_limit.unwrap_or(self.spill_limit);
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
        self.database_other_than_own(t.profile, &t.context)
    }

    /// The database of `context` when it is not profile `profile`'s own.
    pub fn database_other_than_own<'a>(
        &self,
        profile: Option<ProfileId>,
        context: &'a datarig_core::driver::SessionContext,
    ) -> Option<&'a str> {
        let db = context.database.as_deref()?;
        let own = profile.and_then(|p| self.profile(p)).map(|c| c.endpoint().1);
        (own.as_deref() != Some(db)).then_some(db)
    }

    /// The keys that apply to tab `t`'s results: its database's (unknown while not read).
    pub fn tab_keys(&self, t: &Tab) -> &Keys {
        self.conns.keys_in(t.profile, self.other_database(t))
    }

    /// The search path completion resolves names in for tab `t`: its dialect's for the tab's
    /// schema (PostgreSQL: that schema then `public`), else the server's default (`public`).
    pub fn tab_path(&self, t: &Tab) -> Vec<String> {
        // One level of namespaces: the tab works in its schema, else the profile's database.
        let own = (t.context.schema.is_none() && self.schema_only(t.profile))
            .then(|| t.profile.map(|p| self.own_database(p)).filter(|d| !d.is_empty()))
            .flatten();
        self.tab_dialect(t.id).default_path(t.context.schema.as_deref().or(own.as_deref()))
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

    /// Where each tab of the document tab bar was drawn last, and what it leads to (tests
    /// click them).
    pub fn tab_hits(&self) -> Vec<(u16, u16, crate::widgets::tabbar::TabHit)> {
        self.pointer.tab_hits.clone()
    }

    /// The column tab `index` was drawn from last (a sliding or dragged tab where it was at
    /// that frame; tests follow it).
    pub fn tab_x(&self, index: usize) -> Option<i32> {
        self.pointer.tab_geom.drawn.get(index).copied().flatten()
    }

    /// Where each result tab of the strip was drawn last, and what it shows (tests click them).
    pub fn strip_hits(&self) -> Vec<(u16, u16, Option<usize>, tabs::ResultView)> {
        self.pointer.strip_hits.clone()
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
        ConfigError::Alias { table, key, alias } => {
            Msg::ConfigErrorAlias { table: table.clone(), key: key.to_string(), alias: alias.to_string() }
        }
        ConfigError::AliasTables { key, alias } => {
            Msg::ConfigErrorAliasTables { key: key.to_string(), alias: alias.to_string() }
        }
        ConfigError::AliasProfile { profile, key, alias } => {
            Msg::ConfigErrorAliasProfile { profile: profile.clone(), key: key.to_string(), alias: alias.to_string() }
        }
        ConfigError::SessionName(name) => Msg::ConfigErrorSessionName { name: name.clone() },
        ConfigError::SessionReserved(name) => Msg::ConfigErrorSessionReserved { name: name.clone() },
        ConfigError::SessionMissing { key, preset } => {
            Msg::ConfigErrorSessionMissing { key: key.to_string(), preset: preset.clone() }
        }
    }
}
