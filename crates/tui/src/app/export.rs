//! Exporting a result to a file: `:export [format] [all|fetched|selection] [path]`, and the
//! export dialog (`results.export`, `:export` alone: [`super::export_form`]). A job writes
//! on a thread of its own ([`spawn_writer`]): the UI thread never writes the file and never
//! sees a row of an export of every row, which the tab's session streams into a bounded channel
//! (`DbCommand::Export`) the writer drains. Each job ends with exactly one terminal event
//! ([`ExportNews::Ended`]), whatever stops it.
//!
//! **Scopes**: the rows fetched so far, or the selected range, are read from the result's rows
//! (its spill file once it spilled: [`RowsSnapshot`]), with nothing sent to the server. Every row
//! of a result with more on the server runs its statement again on the tab's session (the tab
//! is busy meanwhile, and the export shows as what it runs; a cancel stops it), only when the
//! statement reads:
//!
//! * a table tab whose query is the table's own `SELECT *`, outside the user's transaction, on a
//!   table with a primary key (or a unique key of `NOT NULL` columns) is read in chunks by its
//!   key (`export::keyset`): short reads, none holding a lock for long; nothing is asked;
//! * any other statement runs once and streams. A statement on the plain-`SELECT` allowlist
//!   (`sql::risk::repeat`) runs without asking, except in a read-only or prod-like environment
//!   (`read_only`, or `confirm = "writes"`), where the question says that the statement holds its
//!   read lock and its snapshot until the last row is written (and the table's estimate when
//!   known); any other statement always asks, and says it runs again and why it is not on the
//!   allowlist. `Enter` cancels, `y` exports.
//!
//! Inside the user's transaction the export reads there (the driver's savepoint on
//! PostgreSQL), and says so. A held result's portal is closed first: the statement runs again.
//!
//! **The file**: written as `<path>.partial` (created new, never over anything), renamed to the
//! path once the export succeeded. A path that exists asks first (`Enter` keeps it, `y` replaces
//! it). A cancel or a failure leaves the `.partial` file with the rows written so far, its format
//! ended (a JSON array closed, the XML's root), and the message says where; one without any row
//! is removed. A file that failed to be written (a full disk) may end in the middle of a row.
//!
//! A server may end a slow export: MySQL's `net_write_timeout`, PostgreSQL's
//! `idle_in_transaction_session_timeout` (or a pooler's idle transaction timeout) between two
//! pages. The export then fails, keeping its partial file. A local export cannot be cancelled
//! while the tab runs another statement (the cancel is that statement's).

use super::copy::{CopyWhat, Intent};
use super::tabs::TableRef;
use super::*;
use datarig_core::driver::keys::{Generated, InsertPlan, insert_source, update_source};
use datarig_core::driver::structure::TableStructure;
use datarig_core::driver::{Cell, ColumnMeta, DbError, ExportBatch, ExportGuard, ExportRead, ReadIn};
use datarig_core::export::keyset::{self, Keyset};
use datarig_core::export::stream::{BadDelimited, Delimited, Quote, RowSink, TextWriter};
use datarig_core::export::{self as core_export, Kind, Refused, SqlCells};
use datarig_core::fault::Fault;
use datarig_core::results::RowsSnapshot;
use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::sync::mpsc;

/// Rows the driver sends in one batch, and rows read from a result's store at a time.
pub const BATCH_ROWS: usize = 2_000;

/// Batches the channel between the session and the writer holds: what an export keeps in memory
/// beyond the batch being written.
pub const CHANNEL_BATCHES: usize = 2;

/// The writer's buffer.
const BUFFER: usize = 256 * 1024;

/// How long quitting waits for the writers to end their files.
const QUIT_WAIT: Duration = Duration::from_secs(2);

/// How often the writer says how far it got, at most.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// A format of an export to a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Csv,
    Tsv,
    Json,
    JsonPretty,
    JsonLines,
    Markdown,
    Html,
    Xml,
    SqlInsert,
    SqlUpdate,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 10] = [
        ExportFormat::Csv,
        ExportFormat::Tsv,
        ExportFormat::Json,
        ExportFormat::JsonPretty,
        ExportFormat::JsonLines,
        ExportFormat::Markdown,
        ExportFormat::Html,
        ExportFormat::Xml,
        ExportFormat::SqlInsert,
        ExportFormat::SqlUpdate,
    ];

    /// Its name as `:export` takes it.
    pub fn name(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Tsv => "tsv",
            ExportFormat::Json => "json",
            ExportFormat::JsonPretty => "json_pretty",
            ExportFormat::JsonLines => "jsonl",
            ExportFormat::Markdown => "markdown",
            ExportFormat::Html => "html",
            ExportFormat::Xml => "xml",
            ExportFormat::SqlInsert => "insert",
            ExportFormat::SqlUpdate => "update",
        }
    }

    /// The format `:export` names `name` (case and `-` as `_` aside; `ndjson` is JSON Lines,
    /// `md` Markdown, `sql` INSERT statements).
    pub fn parse(name: &str) -> Option<ExportFormat> {
        let name = name.to_ascii_lowercase().replace('-', "_");
        match name.as_str() {
            "ndjson" | "json_lines" => Some(ExportFormat::JsonLines),
            "md" => Some(ExportFormat::Markdown),
            "sql" => Some(ExportFormat::SqlInsert),
            _ => ExportFormat::ALL.into_iter().find(|f| f.name() == name),
        }
    }

    /// The extension of its files.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Tsv => "tsv",
            ExportFormat::Json | ExportFormat::JsonPretty => "json",
            ExportFormat::JsonLines => "jsonl",
            ExportFormat::Markdown => "md",
            ExportFormat::Html => "html",
            ExportFormat::Xml => "xml",
            ExportFormat::SqlInsert | ExportFormat::SqlUpdate => "sql",
        }
    }

    /// The format a file's extension names (`.sql` is INSERT statements).
    pub fn of_path(path: &Path) -> Option<ExportFormat> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "csv" => ExportFormat::Csv,
            "tsv" | "tab" => ExportFormat::Tsv,
            "json" => ExportFormat::Json,
            "jsonl" | "ndjson" => ExportFormat::JsonLines,
            "md" | "markdown" => ExportFormat::Markdown,
            "html" | "htm" => ExportFormat::Html,
            "xml" => ExportFormat::Xml,
            "sql" => ExportFormat::SqlInsert,
            _ => return None,
        })
    }

    /// Its name in the export dialog's list.
    pub fn dialog_label(self) -> Label {
        match self {
            ExportFormat::Tsv => Label::CopyFormatTsv,
            ExportFormat::JsonPretty => Label::ExportFormatJsonPretty,
            ExportFormat::Markdown => Label::ExportFormatMarkdown,
            ExportFormat::Html => Label::ExportFormatHtml,
            f => f.label(),
        }
    }

    /// Its name in messages.
    pub fn label(self) -> Label {
        match self {
            ExportFormat::Csv => Label::CopyFormatCsv,
            ExportFormat::Tsv => Label::CopyFormatTsvHeader,
            ExportFormat::Json => Label::CopyFormatJson,
            ExportFormat::JsonPretty => Label::CopyFormatJsonPretty,
            ExportFormat::JsonLines => Label::ExportFormatJsonLines,
            ExportFormat::Markdown => Label::CopyFormatMarkdown,
            ExportFormat::Html => Label::CopyFormatHtml,
            ExportFormat::Xml => Label::CopyFormatXml,
            ExportFormat::SqlInsert => Label::CopyFormatSql,
            ExportFormat::SqlUpdate => Label::CopyFormatUpdate,
        }
    }
}

/// Formats the export dialog lists before the app writes them, by the name `:export` is to take
/// for each: greyed, with a word that they come later, while [`ExportFormat::parse`] knows no
/// format of that name.
pub const LATER_FORMATS: [(&str, Label); 2] =
    [("xlsx", Label::ExportFormatXlsx), ("parquet", Label::ExportFormatParquet)];

/// What an export takes from the result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportScope {
    /// Every row: those fetched, and the server's rest (the statement runs again).
    All,
    /// The rows fetched so far.
    Fetched,
    /// The selected range, else the cell under the cursor.
    Selection,
}

impl ExportScope {
    pub const ALL: [ExportScope; 3] = [ExportScope::All, ExportScope::Fetched, ExportScope::Selection];

    pub fn name(self) -> &'static str {
        match self {
            ExportScope::All => "all",
            ExportScope::Fetched => "fetched",
            ExportScope::Selection => "selection",
        }
    }

    pub fn parse(word: &str) -> Option<ExportScope> {
        ExportScope::ALL.into_iter().find(|s| s.name().eq_ignore_ascii_case(word))
    }
}

/// An export the user asked for (`:export`, the `results.export` action): what any way of asking
/// for one hands to [`App::start_export`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportRequest {
    pub scope: ExportScope,
    pub format: ExportFormat,
    /// CSV's options (and the header line of TSV).
    pub csv: Delimited,
    /// Where the file goes (absolute, `~` expanded).
    pub path: PathBuf,
    /// `path` is the default one: when its folder (`~/Downloads`) is missing it goes in the home
    /// folder instead.
    pub default_path: bool,
    /// The user said to replace a file at `path`.
    pub overwrite: bool,
}

/// What the writer reports ([`AppEvent::Export`]).
#[derive(Debug)]
pub struct ExportEvent {
    pub job: u64,
    pub tab: TabId,
    pub news: ExportNews,
}

#[derive(Debug)]
pub enum ExportNews {
    /// The `.partial` file is open: the rows may come (the file goes to `path` once done).
    Ready { path: PathBuf },
    /// Rows and bytes written so far.
    Progress { rows: u64, bytes: u64 },
    /// The job's one terminal event.
    Ended(Outcome),
}

/// How an export ended.
#[derive(Debug)]
pub enum Outcome {
    Done {
        rows: u64,
        bytes: u64,
        path: PathBuf,
        /// Values written without the quotes they needed (CSV's quotes turned off).
        damaged: u64,
    },
    Cancelled {
        rows: u64,
        /// The partial file with the rows so far; `None` when none was written (it was removed).
        partial: Option<PathBuf>,
    },
    Failed {
        why: Failure,
        rows: u64,
        partial: Option<PathBuf>,
    },
    /// Nothing was written: the path exists and replacing it was not confirmed. The rows of a
    /// local export come back for the next try.
    Exists {
        path: PathBuf,
        rows: Option<RowsSnapshot>,
    },
}

/// Why an export failed.
#[derive(Debug)]
pub enum Failure {
    /// Writing the file failed (the path it was at).
    Io(Fault, PathBuf),
    /// Reading the result's rows failed (its spill file).
    Read(Fault),
    /// The partial file of an earlier export is in the way.
    PartialInTheWay(PathBuf),
    /// The path names a folder or a link, not a file to write.
    NotAFile(PathBuf),
    /// A file appeared at the path while the export ran (its rows stay in the partial file).
    TargetAppeared(PathBuf),
    /// The result's columns changed (the table was altered between two chunks).
    SchemaChanged,
    /// A value of a column cannot be written as SQL whole.
    NotWhole(Refused),
    /// The tab's session ended (closed, lost, rebound).
    SessionEnded,
    /// The tab's MySQL session was lost while it streamed (a server drops a connection whose
    /// reader stays behind longer than its `net_write_timeout`).
    MySqlLost,
    /// The tab's result changed before the export could start.
    Stale,
    /// The statement failed (the server's error).
    Db(DbError),
    /// A key value of the last row read cannot be written for the next chunk.
    Key,
    /// The writer stopped without saying why (a bug).
    Internal,
}

/// The app's end of a writer's control channel (never blocks the UI thread).
type CtlSender = tokio::sync::mpsc::UnboundedSender<Ctl>;

/// What the app tells a job's writer.
enum Ctl {
    /// The rows of the next statement (or chunk), in order.
    Rows(tokio::sync::mpsc::Receiver<ExportBatch>),
    /// Every row was sent: finish the file.
    Finish,
    Cancel,
    Fail(Failure),
}

/// What a job writes: its columns and format, and how a row of the source becomes a row of the
/// file. Owned, for the writer's thread.
#[derive(Clone, Debug)]
struct Spec {
    format: ExportFormat,
    csv: Delimited,
    /// The written columns: name and kind.
    columns: Vec<(String, Kind)>,
    /// For each column, its place in a row of the source.
    pick: Vec<usize>,
    /// How SQL formats write each picked value (MySQL's rules).
    cells: Option<SqlCells>,
    sql: Option<SqlTarget>,
    /// The source's column names (every column): a stream whose columns differ is refused.
    source: Vec<String>,
}

#[derive(Clone, Debug)]
enum SqlTarget {
    Insert {
        dialect: Dialect,
        schema: String,
        name: String,
        /// The picked columns written (their place among them) and their name in the table.
        written: Vec<(usize, String)>,
        overriding: bool,
    },
    Update {
        dialect: Dialect,
        schema: String,
        name: String,
        set: Vec<(usize, String)>,
        keys: Vec<(usize, String)>,
    },
}

/// How a job reads its rows.
#[derive(Clone, Debug)]
enum Plan {
    /// From the result's rows (fetched or selected).
    Local,
    /// The statement once, streamed.
    Stream { sql: String, guard: ExportGuard },
    /// The table by its key, a chunk at a time.
    Keyset(Keyset),
}

/// Where a job is.
#[derive(Debug, PartialEq, Eq)]
enum Stage {
    /// Waiting for the table's structure (its key): `schema.table` of profile `profile`, asked
    /// of its metadata session of generation `generation`.
    Structure { profile: ProfileId, schema: String, table: String, generation: u64 },
    /// Waiting for the user's answer.
    Asking,
    /// The writer opens the file.
    Preflight,
    /// Rows are written.
    Running,
}

/// One export (at most one per tab).
struct Job {
    id: u64,
    intent: Intent,
    request: ExportRequest,
    spec: Spec,
    plan: Plan,
    stage: Stage,
    /// The question opened by itself (after the structure came): it takes no key typed as it came
    /// up.
    own: bool,
    started: Instant,
    rows: u64,
    bytes: u64,
    /// How many rows there are: exact, or the server's estimate.
    total: Option<(u64, bool)>,
    path: PathBuf,
    ctl: Option<CtlSender>,
    /// Its writer's thread.
    writer: Option<std::thread::JoinHandle<()>>,
    /// Its start (and what it sends to the server) is in the run's Messages.
    noted: bool,
    /// The rows of a local export, until its writer takes them.
    snapshot: Option<RowsSnapshot>,
    /// A statement of the job runs on the tab's session.
    db: bool,
    /// The writer was told how the statements ended (finish, cancel or fail): nothing runs for
    /// the job any more.
    told: bool,
    cancelled: bool,
    /// It reads inside the user's transaction.
    in_block: bool,
    /// The last keyset chunk sent: the next one must differ, or the export would not move on.
    last_chunk: Option<String>,
    /// The table's estimate of its rows, for the question.
    estimate: Option<u64>,
}

/// The exports of the app: one per tab.
#[derive(Default)]
pub struct Exports {
    jobs: HashMap<TabId, Job>,
    seq: u64,
    /// The tab whose export asks (to start, or to replace the file).
    asking: Option<TabId>,
    /// Jobs the app ended itself (a cancelled one whose session closed): their writer's late
    /// terminal event says nothing more.
    quiet: std::collections::HashSet<u64>,
    /// Without the event loop's channel (headless): where the writers' events go.
    local: Option<(mpsc::Sender<AppEvent>, mpsc::Receiver<AppEvent>)>,
}

/// Where a writer's events go.
#[derive(Clone)]
enum Out {
    App(tokio::sync::mpsc::UnboundedSender<AppEvent>),
    Local(mpsc::Sender<AppEvent>),
}

impl Out {
    fn send(&self, ev: AppEvent) {
        match self {
            Out::App(tx) => {
                let _ = tx.send(ev);
            }
            Out::Local(tx) => {
                let _ = tx.send(ev);
            }
        }
    }
}

/// `path` with its `.partial` suffix.
pub fn partial_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".partial");
    PathBuf::from(s)
}

/// A file name from `name`: path separators, control characters and the characters Windows
/// refuses as `_`, trimmed, at most 80 characters (`export` when nothing is left).
pub fn file_stem(name: &str) -> String {
    let cleaned: String =
        name.chars()
            .map(|c| {
                if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                    '_'
                } else {
                    c
                }
            })
            .take(80)
            .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim();
    if cleaned.is_empty() { "export".to_string() } else { cleaned.to_string() }
}

/// `typed` as a path: `~` and `~/…` in `home`, an absolute path as it is, a relative one in
/// `folder` (the default folder of exports).
pub fn expand_path(typed: &str, home: Option<&Path>, folder: &Path) -> Option<PathBuf> {
    let typed = typed.trim();
    if typed.is_empty() {
        return None;
    }
    if typed == "~" {
        return home.map(Path::to_path_buf);
    }
    if let Some(rest) = typed.strip_prefix("~/").or_else(|| typed.strip_prefix("~\\")) {
        return home.map(|h| h.join(rest));
    }
    let p = PathBuf::from(typed);
    Some(if p.is_absolute() { p } else { folder.join(p) })
}

/// `:export`'s words: a format, a scope, CSV's options and a path, each optional, in that
/// order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportArgs {
    pub format: Option<ExportFormat>,
    pub scope: Option<ExportScope>,
    /// CSV's options (`delim=;`, `quote=never`, `header=off`, `null=NULL`): the defaults when
    /// none is given.
    pub csv: Delimited,
    /// The CSV options given, by key (`delim`, `quote`, `header`, `null`), whatever their value.
    pub given: Vec<&'static str>,
    pub path: Option<String>,
}

/// Why `:export`'s words were refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgsError {
    UnknownFormat(String),
    /// An option word whose value is not one (`quote=maybe`).
    BadOption(String),
}

/// CSV option `word` (`key=value`) applied to `csv`: its key, `Ok(None)` when it is not an
/// option word (a path), an error for a known key with a value it does not take.
fn csv_option(word: &str, csv: &mut Delimited) -> Result<Option<&'static str>, ArgsError> {
    let Some((key, value)) = word.split_once('=') else { return Ok(None) };
    let bad = || ArgsError::BadOption(word.to_string());
    let given = match key.to_ascii_lowercase().as_str() {
        "delim" | "delimiter" => {
            csv.delimiter = match value {
                "\\t" | "tab" => '\t',
                "comma" => ',',
                "semicolon" => ';',
                "pipe" => '|',
                v if v.chars().count() == 1 => v.chars().next().ok_or_else(bad)?,
                _ => return Err(bad()),
            };
            "delim"
        }
        "quote" => {
            csv.quote = match value.to_ascii_lowercase().replace('-', "_").as_str() {
                "necessary" => Quote::Necessary,
                "always" => Quote::Always,
                "non_numeric" => Quote::NonNumeric,
                "never" | "none" => Quote::Never,
                _ => return Err(bad()),
            };
            "quote"
        }
        "header" => {
            csv.header = match value.to_ascii_lowercase().as_str() {
                "on" | "yes" | "true" => true,
                "off" | "no" | "false" => false,
                _ => return Err(bad()),
            };
            "header"
        }
        "null" => {
            csv.null = value.to_string();
            "null"
        }
        _ => return Ok(None),
    };
    Ok(Some(given))
}

/// Read `:export [format] [all|fetched|selection] [path]`. The path is the rest (it may hold
/// spaces); a first word that is a format's name is one (a file named `csv` is written `./csv`).
pub fn parse_args(arg: &str) -> Result<ExportArgs, ArgsError> {
    let mut out = ExportArgs::default();
    let mut rest = arg.trim();
    let word = |s: &str| -> (String, String) {
        let (w, r) = s.split_once(char::is_whitespace).unwrap_or((s, ""));
        (w.to_string(), r.trim_start().to_string())
    };
    if !rest.is_empty() {
        let (w, r) = word(rest);
        if let Some(f) = ExportFormat::parse(&w) {
            out.format = Some(f);
            rest = arg.trim()[arg.trim().len() - r.len()..].trim();
        } else if !w.contains(['/', '\\', '.', '~']) && ExportScope::parse(&w).is_none() && r.is_empty() {
            return Err(ArgsError::UnknownFormat(w));
        }
    }
    if !rest.is_empty() {
        let (w, r) = word(rest);
        if let Some(s) = ExportScope::parse(&w) {
            out.scope = Some(s);
            rest = rest[rest.len() - r.len()..].trim();
        }
    }
    while !rest.is_empty() {
        let (w, r) = word(rest);
        let Some(key) = csv_option(&w, &mut out.csv)? else { break };
        out.given.push(key);
        rest = rest[rest.len() - r.len()..].trim();
    }
    if !rest.is_empty() {
        out.path = Some(rest.to_string());
    }
    Ok(out)
}

impl App {
    /// What `:export [format] [all|fetched|selection] [path]` asks for: the format from the
    /// path's extension when it is not named (CSV without a path), every row by default, the
    /// path `~/Downloads/<tab>-<time>.<ext>` when none is given (a relative path goes in
    /// `~/Downloads` too).
    pub fn export_request(&self, arg: &str) -> Result<ExportRequest, Notice> {
        let args = match parse_args(arg) {
            Ok(a) => a,
            Err(ArgsError::UnknownFormat(format)) => {
                return Err(Notice::new(Msg::ExportUnknownFormat { format }, Level::Error));
            }
            Err(ArgsError::BadOption(option)) => {
                return Err(Notice::new(Msg::ExportBadOption { option }, Level::Error));
            }
        };
        let folder = self.export_folder();
        let path = match &args.path {
            Some(p) => Some(
                expand_path(p, self.home().as_deref(), &folder)
                    .ok_or_else(|| Notice::new(Label::ExportUsage, Level::Error))?,
            ),
            None => None,
        };
        let format = match (args.format, &path) {
            (Some(f), _) => f,
            (None, Some(p)) => ExportFormat::of_path(p).ok_or_else(|| {
                Notice::new(Msg::ExportUnknownExtension { path: p.display().to_string() }, Level::Error)
            })?,
            (None, None) => ExportFormat::Csv,
        };
        // CSV's options are CSV's; TSV takes its header line only.
        let ignored = match format {
            ExportFormat::Csv => false,
            ExportFormat::Tsv => args.given.iter().any(|k| *k != "header"),
            _ => !args.given.is_empty(),
        };
        if ignored {
            return Err(Notice::new(Label::ExportCsvOptionsOnly, Level::Error));
        }
        let default_path = path.is_none();
        let path = path.unwrap_or_else(|| self.default_export_path(format));
        Ok(ExportRequest {
            scope: args.scope.unwrap_or(ExportScope::All),
            format,
            csv: args.csv,
            path,
            default_path,
            overwrite: false,
        })
    }

    /// The `results.export` action: the export dialog on the active tab's result.
    pub(super) fn export_action(&mut self) {
        self.open_export_dialog();
    }

    /// Where exports go by default: `~/Downloads` (the writer falls back to the home folder
    /// when it is missing).
    pub(super) fn export_folder(&self) -> PathBuf {
        self.home().map(|h| h.join("Downloads")).unwrap_or_default()
    }

    /// `~/Downloads/<name>-<YYYYMMDD-HHMMSS>.<ext>`: the table's name for a table tab, else the
    /// tab's.
    pub(super) fn default_export_path(&self, format: ExportFormat) -> PathBuf {
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        self.export_folder().join(format!("{}-{stamp}.{}", self.export_stem(), format.extension()))
    }

    /// The name of the active tab's exports: its table's for a table tab, else the tab's.
    pub(super) fn export_stem(&self) -> String {
        let t = self.tab();
        let name = match &t.doc.table {
            Some(table) => table.name.clone(),
            None => crate::widgets::tabbar::document_name(self, t),
        };
        file_stem(&name)
    }

    /// Whether tab `id` has an export (asking, opening its file, or writing).
    pub fn exporting(&self, id: TabId) -> bool {
        self.exports.jobs.contains_key(&id)
    }

    /// Start `req` on the active tab's result (one export per tab).
    pub fn start_export(&mut self, req: ExportRequest) {
        let id = self.tab().id;
        if self.exports.jobs.contains_key(&id) {
            return self.flash(Notice::new(Label::ExportBusyTab, Level::Warning));
        }
        if let ExportFormat::Csv = req.format
            && let Err(e) = req.csv.check()
        {
            let label = match e {
                BadDelimited::Delimiter => Label::ExportBadDelimiter,
                BadDelimited::Null => Label::ExportBadNull,
            };
            return self.flash(Notice::new(label, Level::Warning));
        }
        let what = match req.scope {
            ExportScope::Selection => CopyWhat::Cell,
            ExportScope::Fetched | ExportScope::All => CopyWhat::Every,
        };
        let Some(b) = self.block(what) else {
            return self.flash(Notice::new(Label::ExportNothing, Level::Warning));
        };
        // Every row of a result fetched to its end is the rows fetched.
        let db = req.scope == ExportScope::All && b.partial;
        let (intent, read_in, snapshot) = {
            let t = self.tab();
            let Results::Rows(rs) = &t.results else { return };
            let read_in = if db { ReadIn::first(t.exec.values.as_ref()) } else { rs.read_in.clone() };
            let snapshot = if db { None } else { Some(rs.rows.snapshot(b.rows.clone())) };
            (Intent::of(t), read_in, snapshot)
        };
        let spec = match self.export_spec(&b.cols, &req, &read_in) {
            Ok(s) => s,
            Err(n) => return self.flash(n),
        };
        let snapshot = match snapshot {
            Some(Ok(s)) => Some(s),
            Some(Err(fault)) => {
                let reason = self.fault_text("export.read_failed", &fault);
                return self.flash(Notice::new(Msg::ExportFailedEmpty { reason }, Level::Error));
            }
            None => None,
        };
        if db && let Err(n) = self.export_db_checks(id) {
            return self.tab_status(id, n);
        }
        self.exports.seq += 1;
        let total = snapshot.as_ref().map(|s| (s.len() as u64, true));
        let (in_block, sql) = (self.tab().exec.in_block, self.tab().shown_sql().trim().to_string());
        let job = Job {
            id: self.exports.seq,
            intent,
            request: req.clone(),
            spec,
            plan: Plan::Local,
            stage: Stage::Preflight,
            own: false,
            started: self.now(),
            rows: 0,
            bytes: 0,
            total,
            path: req.path.clone(),
            ctl: None,
            writer: None,
            noted: false,
            snapshot,
            db,
            told: false,
            cancelled: false,
            in_block,
            last_chunk: None,
            estimate: None,
        };
        self.exports.jobs.insert(id, job);
        if !db {
            return self.export_launch(id);
        }
        // Every row: the statement runs again on the tab's session; a table tab's own query
        // outside the user's block is read by the table's key, once its structure says it.
        match self.export_keyset_table(id) {
            Some((pid, table)) => {
                let generation = self.conns.get(pid).map_or(0, |c| c.generation);
                if let Some(j) = self.exports.jobs.get_mut(&id) {
                    let (schema, name) = (table.schema.clone(), table.name.clone());
                    j.stage = Stage::Structure { profile: pid, schema, table: name, generation };
                }
                self.tab_status(id, Notice::new(Label::ExportReadingKey, Level::Info));
                self.send_meta(pid, DbCommand::LoadStructure { schema: table.schema, table: table.name });
            }
            None => self.export_stream(id, sql, None),
        }
    }

    /// The table of table tab `id` when every row of its result may be read by the table's key:
    /// its query is the table's own, outside the user's block, in the profile's own database,
    /// with a metadata session to read the key on (without one, nothing would answer).
    pub(super) fn export_keyset_table(&self, id: TabId) -> Option<(ProfileId, TableRef)> {
        let t = self.tabs.get(id)?;
        let table = t.doc.table.clone()?;
        let pid = t.profile?;
        let own = t.shown_sql().trim() == table.query(self.tab_dialect(id));
        let meta = self.structure_on(pid) && self.conns.get(pid).is_some_and(|c| c.meta.is_some());
        (own && !t.exec.in_block && t.context.database.is_none() && meta).then_some((pid, table))
    }

    /// How tab `id`'s result is read by the key of its table `schema.table`, whose structure is
    /// `s`: `None` when it has no key that fits, or the result is not the table's columns.
    pub(super) fn export_keyset(&self, id: TabId, schema: &str, table: &str, s: &TableStructure) -> Option<Keyset> {
        let t = self.tabs.get(id)?;
        let d = self.tab_dialect(id);
        let text_ok = match d {
            Dialect::Postgres => true,
            Dialect::MySql(_) => t.exec.values.as_ref().is_some_and(|v| v.charset.eq_ignore_ascii_case("utf8mb4")),
        };
        let Results::Rows(rs) = &t.results else { return None };
        let metas: Vec<ColumnMeta> = rs.columns.iter().map(|c| c.meta.clone()).collect();
        let table_columns: Vec<String> = s.columns.iter().map(|c| c.name.clone()).collect();
        keyset::table_key(s).and_then(|key| Keyset::new(d, schema, table, &key, &metas, &table_columns, text_ok)).ok()
    }

    /// Why every row of tab `id`'s result cannot be read again for an export now.
    pub(super) fn export_db_checks(&self, id: TabId) -> Result<(), Notice> {
        let t = self.tabs.get(id).ok_or_else(|| Notice::new(Label::ExportNothing, Level::Warning))?;
        if t.exec.running.is_some() || self.is_queued(id) {
            let key = self.key_for(Action::CancelQuery, Ctx::Nav);
            return Err(Notice::new(Msg::QueryBusy { key }, Level::Warning));
        }
        let pid = t.profile.ok_or_else(|| Notice::new(Label::ExportStale, Level::Warning))?;
        if !self.profile_capabilities(Some(pid)).is_some_and(|c| c.export) {
            return Err(Notice::new(Label::ExportUnsupportedDriver, Level::Warning));
        }
        if !(self.answer_here() || self.kept_answer_here()) {
            return Err(Notice::new(Label::ExportStale, Level::Warning));
        }
        if t.exec.in_block && t.exec.tx_aborted {
            return Err(Notice::new(Label::ExportTxAborted, Level::Warning));
        }
        let sql = t.shown_sql().trim().to_string();
        let statements = vec![sql.clone()];
        if let Some(refused) =
            self.unsupported(id, &statements).or_else(|| self.read_only_refusal(id, pid, &statements))
        {
            return Err(refused);
        }
        let risk = Classifier::classify_once(t.exec.prepared.language(), &sql);
        if risk.class != datarig_core::sql::risk::Class::Read || risk.writes {
            let sql = super::runlog::excerpt(&sql, 60);
            return Err(Notice::new(Msg::ExportNotARead { sql }, Level::Warning));
        }
        Ok(())
    }

    /// The table's structure came for an export waiting for its key: read by its key when it
    /// has one that fits, else with one statement. `Some` for a structure an export waited for.
    pub(super) fn export_structure_came(
        &mut self,
        profile: ProfileId,
        schema: &str,
        table: &str,
        result: Result<&TableStructure, &DbError>,
    ) {
        let waiting: Vec<TabId> = self
            .exports
            .jobs
            .iter()
            .filter(|(_, j)| {
                matches!(&j.stage, Stage::Structure { profile: p, schema: s, table: t, .. }
                    if *p == profile && s == schema && t == table)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in waiting {
            if !self.exports.jobs.get(&id).is_some_and(|j| j.intent.holds(self)) {
                self.drop_export(id, Notice::new(Label::ExportStale, Level::Warning));
                continue;
            }
            let Some(t) = self.tabs.get(id) else { continue };
            let sql = t.shown_sql().trim().to_string();
            let Ok(s) = result else {
                // No structure, no key: one statement.
                self.export_stream(id, sql, None);
                continue;
            };
            if !matches!(t.results, Results::Rows(_)) {
                continue;
            }
            let keyset = self.export_keyset(id, schema, table, s);
            let estimate = s.estimated_rows;
            if let Some(j) = self.exports.jobs.get_mut(&id) {
                j.estimate = estimate;
                j.own = true;
            }
            match keyset {
                Some(k) => {
                    if let Some(j) = self.exports.jobs.get_mut(&id) {
                        j.total = estimate.map(|e| (e, false));
                        j.plan = Plan::Keyset(k);
                    }
                    self.export_launch(id);
                }
                None => self.export_stream(id, sql, estimate),
            }
        }
    }

    /// Every row of tab `id`'s result with its statement `sql` once, streamed: asked first when
    /// it is not on the allowlist, or in a read-only or prod-like environment.
    /// Why every row of tab `id`'s result read with `sql` once is asked first: the statement
    /// is not on the allowlist (why, in words), and the profile's environment is read-only or
    /// prod-like (its name). Neither: it is not asked. The dialog's note says the same.
    pub(super) fn export_asks(&self, id: TabId, sql: &str) -> (Option<String>, Option<String>) {
        let Some(t) = self.tabs.get(id) else { return (None, None) };
        let rerun =
            t.exec.prepared.repeatable(sql).err().map(|why| self.i18n.msg(&super::pages::why(&why)).to_string());
        let environment = t.profile.filter(|pid| {
            let rules = self.rules_of(*pid);
            rules.read_only || rules.confirm.writes()
        });
        let environment = environment
            .map(|pid| self.environment_name(pid).unwrap_or_else(|| datarig_core::policy::DEFAULT.to_string()));
        (rerun, environment)
    }

    fn export_stream(&mut self, id: TabId, sql: String, estimate: Option<u64>) {
        if self.tabs.get(id).and_then(|t| t.profile).is_none() {
            return;
        }
        let (rerun, environment) = self.export_asks(id, &sql);
        let guard = if rerun.is_none() { ExportGuard::Repeatable } else { ExportGuard::Confirmed };
        let Some(j) = self.exports.jobs.get_mut(&id) else { return };
        j.plan = Plan::Stream { sql: sql.clone(), guard };
        j.estimate = j.estimate.or(estimate);
        j.total = j.estimate.map(|e| (e, false));
        if rerun.is_none() && environment.is_none() {
            return self.export_launch(id);
        }
        // Another question is open (this one comes once the table's structure came): not asked
        // over it.
        if self.overlays.confirm().is_some() {
            return self.drop_export(id, Notice::new(Label::ExportQuestionBusy, Level::Warning));
        }
        let Some(j) = self.exports.jobs.get_mut(&id) else { return };
        // Ask: what it does to the server while it runs, and why it runs again.
        j.stage = Stage::Asking;
        let (in_block, estimate) = (j.in_block, j.estimate);
        let mut details = Vec::new();
        if let Some(why) = rerun {
            details.push(Msg::ExportConfirmRerun { why, sql: super::runlog::excerpt(&sql, 80) });
        }
        details.push(Msg::Label(Label::ExportConfirmLock));
        if let Some(count) = estimate {
            details.push(Msg::ExportConfirmEstimate { count });
        }
        if in_block {
            details.push(Msg::Label(Label::ExportConfirmInTx));
        }
        if let Some(environment) = environment {
            details.push(Msg::ExportConfirmEnvironment { environment });
        }
        let format = self.exports.jobs.get(&id).map(|j| self.i18n.label(j.request.format.label()).to_string());
        let path = self.exports.jobs.get(&id).map(|j| j.request.path.display().to_string());
        self.exports.asking = Some(id);
        self.confirm(
            Label::ExportConfirmTitle,
            Label::ExportConfirmTitle,
            Label::ExportConfirmKeys,
            ConfirmAction::ExportStart,
        );
        if let Some(c) = self.overlays.confirm_mut().filter(|c| c.action == ConfirmAction::ExportStart) {
            c.text = Msg::ExportConfirmText { format: format.unwrap_or_default(), path: path.unwrap_or_default() };
            c.details = details;
        }
    }

    /// The question opened by itself (once the table's structure came) and takes no key typed
    /// as it came up.
    pub(super) fn export_unarmed(&self) -> bool {
        let own = self.exports.asking.and_then(|id| self.exports.jobs.get(&id)).is_some_and(|j| j.own);
        let shown = self.overlays.confirm().and_then(|c| c.buttons.press.shown_at);
        own && shown.is_none_or(|t| self.now().saturating_duration_since(t) < super::overlay::ARM_DELAY)
    }

    /// The question about starting (or about the file in the way) was answered.
    pub(super) fn export_answered(&mut self, yes: bool, replace: bool) {
        let Some(id) = self.exports.asking.take() else { return };
        if !yes {
            let label = if replace { Label::ExportKept } else { Label::ExportDeclined };
            return self.drop_export(id, Notice::new(label, Level::Info));
        }
        if !self.exports.jobs.get(&id).is_some_and(|j| j.intent.holds(self)) {
            return self.drop_export(id, Notice::new(Label::ExportStale, Level::Warning));
        }
        if let Some(j) = self.exports.jobs.get_mut(&id)
            && replace
        {
            j.request.overwrite = true;
        }
        self.export_launch(id);
    }

    /// Forget tab `id`'s export before its writer started, saying `why`.
    fn drop_export(&mut self, id: TabId, why: Notice) {
        if let Some(j) = self.exports.jobs.remove(&id)
            && let Some(ctl) = j.ctl
        {
            let _ = ctl.send(Ctl::Fail(Failure::Stale));
        }
        self.export_said(id, why);
    }

    /// Start tab `id`'s job's writer: it opens the file and says so ([`ExportNews::Ready`]).
    fn export_launch(&mut self, id: TabId) {
        let out = self.export_out();
        let home = self.home();
        let Some(j) = self.exports.jobs.get_mut(&id) else { return };
        j.stage = Stage::Preflight;
        let source = match j.snapshot.take() {
            Some(s) => Source::Local(s),
            None => Source::Db,
        };
        let fallback = if j.request.default_path { home } else { None };
        let (ctl, writer) =
            spawn_writer(j.id, id, j.spec.clone(), j.request.path.clone(), j.request.overwrite, fallback, source, out);
        j.ctl = Some(ctl);
        j.writer = writer;
        let format = self.i18n.label(j.request.format.label()).to_string();
        let path = j.request.path.display().to_string();
        let m = match &j.plan {
            Plan::Keyset(k) => Msg::ExportStartedKeyset { format, path, table: format!("{}.{}", k.schema, k.table) },
            Plan::Stream { .. } if j.in_block => Msg::ExportStartedInTx { format, path },
            _ => Msg::ExportStarted { format, path },
        };
        // The run's Messages get it once the export really starts ([`App::export_noted`]).
        self.tab_status(id, Notice::new(m, Level::Info));
    }

    fn export_out(&mut self) -> Out {
        match &self.tx {
            Some(tx) => Out::App(tx.clone()),
            None => Out::Local(self.exports.local.get_or_insert_with(mpsc::channel).0.clone()),
        }
    }

    /// Headless (without the event loop's channel): hand the export writers' events to the app,
    /// waiting up to `wait` for the first one. How many came.
    pub fn pump_exports(&mut self, wait: Duration) -> usize {
        let mut evs = Vec::new();
        if let Some((_, rx)) = &self.exports.local {
            if let Ok(ev) = rx.recv_timeout(wait) {
                evs.push(ev);
            }
            while let Ok(ev) = rx.try_recv() {
                evs.push(ev);
            }
        }
        let n = evs.len();
        for ev in evs {
            self.on_app_event(ev);
        }
        n
    }

    /// An event of an export's writer.
    pub(super) fn on_export_event(&mut self, ev: ExportEvent) {
        let ExportEvent { job, tab, news } = ev;
        let ours = self.exports.jobs.get(&tab).is_some_and(|j| j.id == job);
        match news {
            ExportNews::Ready { path } => {
                if !ours {
                    return;
                }
                if let Some(j) = self.exports.jobs.get_mut(&tab) {
                    j.path = path;
                    j.stage = Stage::Running;
                }
                if self.exports.jobs.get(&tab).is_some_and(|j| j.db) {
                    self.export_first_statement(tab);
                } else {
                    self.export_noted(tab);
                }
            }
            ExportNews::Progress { rows, bytes } => {
                if !ours {
                    return;
                }
                if let Some(j) = self.exports.jobs.get_mut(&tab) {
                    j.rows = rows;
                    j.bytes = bytes;
                }
                let progress = self.export_progress(tab);
                if let (Some(m), false) = (progress, self.exports.jobs.get(&tab).is_some_and(|j| j.db)) {
                    self.tab_status(tab, Notice::new(m, Level::Info));
                }
            }
            ExportNews::Ended(outcome) => self.export_ended(tab, job, ours, outcome),
        }
    }

    /// The writer opened the file: the first statement of tab `id`'s export goes to its session,
    /// while the tab still shows the result it was asked from.
    fn export_first_statement(&mut self, id: TabId) {
        let holds = self.exports.jobs.get(&id).is_some_and(|j| j.intent.holds(self));
        let busy = self.tabs.get(id).is_none_or(|t| t.exec.running.is_some() || t.exec.session.is_none());
        let Some(j) = self.exports.jobs.get_mut(&id) else { return };
        // Cancelled while the file opened: the writer stops (it may have the cancel already).
        if j.cancelled {
            j.told = true;
            if let Some(ctl) = &j.ctl {
                let _ = ctl.send(Ctl::Cancel);
            }
            return;
        }
        if !holds || busy {
            j.told = true;
            if let Some(ctl) = &j.ctl {
                let _ = ctl.send(Ctl::Fail(Failure::Stale));
            }
            return;
        }
        let Some(j) = self.exports.jobs.get(&id) else { return };
        let (sql, guard) = match &j.plan {
            Plan::Stream { sql, guard } => (sql.clone(), *guard),
            Plan::Keyset(k) => (k.first(), ExportGuard::Keyset),
            Plan::Local => return,
        };
        let now = self.now();
        let Some(t) = self.tabs.get_mut(id) else { return };
        // A held portal is closed first (its rows stay): the statement runs again.
        let qid = t.exec.query_id;
        if matches!(t.exec.paging, Paging::Open { .. }) {
            t.exec.paging = Paging::Replaced;
            t.exec.rerun_ok = t.exec.prepared.repeatable(t.answer_sql()).is_ok();
            self.send_tab(id, DbCommand::ClosePortal { id: qid });
        }
        let Some(t) = self.tabs.get_mut(id) else { return };
        t.exec.running = Some(Running {
            id: qid,
            started: now,
            fetch: false,
            count: false,
            cancelling: None,
            stopping: false,
            refused: false,
            export: true,
        });
        self.export_noted(id);
        self.export_send(id, sql, guard);
    }

    /// Tab `id`'s export starts for real (its file is open, its statement is being sent): said
    /// in the run's Messages, once, with what is sent to the server for the user.
    fn export_noted(&mut self, id: TabId) {
        let Some(j) = self.exports.jobs.get_mut(&id).filter(|j| !j.noted) else { return };
        j.noted = true;
        let format = self.i18n.label(j.request.format.label()).to_string();
        let path = j.path.display().to_string();
        let started = match &j.plan {
            Plan::Keyset(k) => Msg::ExportStartedKeyset { format, path, table: format!("{}.{}", k.schema, k.table) },
            Plan::Stream { .. } if j.in_block => Msg::ExportStartedInTx { format, path },
            _ => Msg::ExportStarted { format, path },
        };
        let sent = match &j.plan {
            Plan::Stream { sql, .. } => Some(Msg::ExportRunsAgain { sql: super::runlog::excerpt(sql, 120) }),
            Plan::Keyset(k) => Some(Msg::ExportRunsInChunks { sql: super::runlog::excerpt(&k.first(), 120) }),
            Plan::Local => None,
        };
        if let Some(t) = self.tabs.get_mut(id) {
            t.exec.run.notes.push(Notice::new(started, Level::Info));
            if let Some(sent) = sent {
                t.exec.run.notes.push(Notice::new(sent, Level::Info));
            }
        }
    }

    /// Send statement `sql` of tab `id`'s export with a channel of its own, handed to the writer.
    fn export_send(&mut self, id: TabId, sql: String, guard: ExportGuard) {
        let Some(j) = self.exports.jobs.get(&id) else { return };
        let Some(ctl) = j.ctl.as_ref() else { return };
        let (sink, rx) = tokio::sync::mpsc::channel(CHANNEL_BATCHES);
        let _ = ctl.send(Ctl::Rows(rx));
        let qid = self.tabs.get(id).map_or(0, |t| t.exec.query_id);
        self.send_tab(id, DbCommand::Export { id: qid, sql, guard, sink, batch: BATCH_ROWS });
    }

    /// The session answered a statement of tab `id`'s export: the next chunk, or the end.
    pub(super) fn on_exported(&mut self, id: TabId, qid: u64, result: Result<ExportRead, DbError>, cancelled: bool) {
        let Some(t) = self.tabs.get_mut(id) else { return };
        if t.exec.query_id != qid || !t.exec.running.is_some_and(|r| r.export) {
            return;
        }
        let Some(j) = self.exports.jobs.get_mut(&id) else {
            t.exec.running = None;
            return;
        };
        let next = match (&result, &j.plan) {
            (Ok(read), Plan::Keyset(k)) if !j.cancelled && read.complete && read.rows == k.chunk => {
                // A chunk that would read the same rows again ends it (no key literal may repeat).
                Some(
                    read.last
                        .as_deref()
                        .and_then(|last| k.after(last))
                        .filter(|sql| j.last_chunk.as_ref() != Some(sql)),
                )
            }
            _ => None,
        };
        if let Some(Some(sql)) = &next {
            j.last_chunk = Some(sql.clone());
        }
        match next {
            Some(Some(sql)) => return self.export_send(id, sql, ExportGuard::Keyset),
            Some(None) => {
                t.exec.running = None;
                j.told = true;
                if let Some(ctl) = &j.ctl {
                    let _ = ctl.send(Ctl::Fail(Failure::Key));
                }
                return;
            }
            None => {}
        }
        t.exec.running = None;
        j.told = true;
        let msg = match result {
            _ if j.cancelled || cancelled => Ctl::Cancel,
            Ok(read) if read.complete => Ctl::Finish,
            // The writer stopped reading (it failed): it says why itself.
            Ok(_) => return,
            Err(e) => Ctl::Fail(Failure::Db(e)),
        };
        if let Some(ctl) = &j.ctl {
            let _ = ctl.send(msg);
        }
    }

    /// Stop tab `id`'s export (the user's cancel): a statement it runs is cancelled with the
    /// tab's (the caller's); rows it writes from the result stop at the next chunk. Whether one
    /// ran.
    pub(super) fn stop_export(&mut self, id: TabId) -> bool {
        let Some(j) = self.exports.jobs.get_mut(&id) else { return false };
        j.cancelled = true;
        match j.stage {
            Stage::Structure { .. } | Stage::Asking => {
                if self.exports.asking == Some(id) {
                    self.exports.asking = None;
                    self.overlays.close(OverlayKind::Confirm);
                }
                self.drop_export(id, Notice::new(Label::ExportDeclined, Level::Warning));
            }
            Stage::Preflight | Stage::Running => {
                // Without a statement running, the writer stops on its own.
                let db_running = self.tabs.get(id).is_some_and(|t| t.exec.running.is_some_and(|r| r.export));
                if !db_running && let Some(ctl) = &j.ctl {
                    let _ = ctl.send(Ctl::Cancel);
                }
            }
        }
        true
    }

    /// The exports whose tab's statement ended without its answer (the session closed, was
    /// lost or rebound, the tab closed): their writer finishes with what it has, and says so.
    /// One waiting for its table's key whose metadata session is gone reads with one statement;
    /// one not started whose result changed ends, with a word.
    pub(super) fn sweep_exports(&mut self) {
        self.sweep_export_dialog();
        let mut ended = Vec::new();
        let mut no_key = Vec::new();
        for (id, j) in &self.exports.jobs {
            let tab = self.tabs.get(*id);
            match &j.stage {
                // Before anything ran: its tab, its result or (to run every row) its session went.
                Stage::Structure { .. } | Stage::Asking
                    if tab.is_none_or(|t| j.db && t.exec.session.is_none()) || !j.intent.holds(self) =>
                {
                    ended.push(*id)
                }
                // No metadata session, or another one than the key was asked of (its answer
                // will not come): one statement.
                Stage::Structure { profile, generation, .. }
                    if self.conns.get(*profile).is_none_or(|c| c.meta.is_none() || c.generation != *generation) =>
                {
                    no_key.push(*id)
                }
                Stage::Structure { .. } | Stage::Asking => {}
                // A local export goes on without its tab (its rows are its own).
                Stage::Preflight if j.db && tab.is_none() => ended.push(*id),
                Stage::Preflight => {}
                Stage::Running => {
                    if j.db
                        && !j.told
                        && j.ctl.is_some()
                        && tab.is_none_or(|t| !t.exec.running.is_some_and(|r| r.export))
                    {
                        ended.push(*id);
                    }
                }
            }
        }
        for id in no_key {
            // A question it opens comes by itself: a key typed as it comes up does not answer it.
            if let Some(j) = self.exports.jobs.get_mut(&id) {
                j.own = true;
            }
            let sql = self.tabs.get(id).map(|t| t.shown_sql().trim().to_string()).unwrap_or_default();
            self.export_stream(id, sql, None);
        }
        for id in ended {
            let Some(j) = self.exports.jobs.get(&id) else { continue };
            match j.stage {
                Stage::Structure { .. } | Stage::Asking => {
                    if self.exports.asking == Some(id) {
                        self.exports.asking = None;
                        self.overlays.close(OverlayKind::Confirm);
                    }
                    self.forget_export(id, Notice::new(Label::ExportStale, Level::Warning));
                }
                // Cancelled, and its session closed under it: cancelled, said now (a writer that
                // waits on its file would say it late, or never).
                _ if j.cancelled => {
                    let partial = partial_path(&j.path).display().to_string();
                    if let Some(j) = self.exports.jobs.remove(&id) {
                        if let Some(ctl) = &j.ctl {
                            let _ = ctl.send(Ctl::Cancel);
                        }
                        self.exports.quiet.insert(j.id);
                    }
                    self.export_said(id, Notice::new(Msg::ExportCancelledClosed { path: partial }, Level::Warning));
                }
                _ => {
                    let lost = self.tabs.get(id).is_some_and(|t| t.exec.state == SessionState::Lost);
                    let mysql = matches!(self.tab_dialect(id), Dialect::MySql(_));
                    let why = if lost && mysql { Failure::MySqlLost } else { Failure::SessionEnded };
                    let Some(j) = self.exports.jobs.get_mut(&id) else { continue };
                    if let Some(ctl) = j.ctl.take() {
                        let _ = ctl.send(Ctl::Fail(why));
                    }
                }
            }
        }
    }

    /// Forget tab `id`'s export that never started (its writer did not run), saying `why` once:
    /// in the tab's status, or the status bar when the tab is gone.
    fn forget_export(&mut self, id: TabId, why: Notice) {
        self.exports.jobs.remove(&id);
        self.export_said(id, why);
    }

    /// The one message of an export of tab `tab`: in its Messages and status, and said at once
    /// (the status bar when the tab is gone).
    fn export_said(&mut self, tab: TabId, notice: Notice) {
        if let Some(t) = self.tabs.get_mut(tab) {
            t.exec.run.notes.push(notice.clone());
        }
        if self.tabs.get(tab).is_some() {
            self.tab_status(tab, notice.clone());
        } else {
            self.show_status(notice.clone());
        }
        if self.tab().id == tab || self.tabs.get(tab).is_none() {
            self.flash(notice);
        }
    }

    /// Quitting: every export stops and ends its file (a partial file whose format is ended),
    /// waiting a moment for the writers.
    pub(super) fn end_exports_for_quit(&mut self) {
        let jobs: Vec<Job> = self.exports.jobs.drain().map(|(_, j)| j).collect();
        let mut writers = Vec::new();
        for j in jobs {
            if let Some(ctl) = &j.ctl {
                let _ = ctl.send(Ctl::Cancel);
            }
            writers.extend(j.writer);
        }
        let deadline = Instant::now() + QUIT_WAIT;
        while writers.iter().any(|w| !w.is_finished()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Tab `tab`'s export ended (`ours`: the job the tab has): the one message about it.
    fn export_ended(&mut self, tab: TabId, job_id: u64, ours: bool, outcome: Outcome) {
        let job = if ours { self.exports.jobs.remove(&tab) } else { None };
        // A late word of a job the app ended itself (its message said so already).
        if !ours && self.exports.quiet.remove(&job_id) {
            return;
        }
        // A statement still running for it needs no cancel: with nobody reading its channel the
        // session stops it at its next batch (a cancel sent now could reach the next statement).
        let notice = match outcome {
            // Cancelled before anything was written: nothing to ask.
            Outcome::Exists { .. } if job.as_ref().is_some_and(|j| j.cancelled) => {
                Notice::new(Label::ExportCancelledEmpty, Level::Warning)
            }
            // Another question is open: not asked over it.
            Outcome::Exists { .. } if ours && self.overlays.confirm().is_some() => {
                Notice::new(Label::ExportQuestionBusy, Level::Warning)
            }
            Outcome::Exists { path, rows } if ours => {
                // Asked (it opens by itself: armed): replace it, or keep it.
                if let Some(j) = job {
                    let job = Job { ctl: None, writer: None, stage: Stage::Asking, own: true, snapshot: rows, ..j };
                    self.exports.jobs.insert(tab, job);
                    self.exports.asking = Some(tab);
                    self.confirm(
                        Label::ExportReplaceTitle,
                        Label::ExportReplaceTitle,
                        Label::ExportReplaceKeys,
                        ConfirmAction::ExportReplace,
                    );
                    if let Some(c) = self.overlays.confirm_mut().filter(|c| c.action == ConfirmAction::ExportReplace) {
                        c.text = Msg::ExportReplaceText { path: path.display().to_string() };
                    }
                }
                return;
            }
            Outcome::Exists { .. } => return,
            Outcome::Done { rows, bytes, path, damaged } => {
                let path = path.display().to_string();
                let size = crate::text::human_bytes(bytes);
                let m = if damaged > 0 {
                    Msg::ExportDoneDamaged { count: rows, path, size, damaged: damaged.to_string() }
                } else {
                    Msg::ExportDone { count: rows, path, size }
                };
                Notice::new(m, Level::Success)
            }
            Outcome::Cancelled { rows, partial: Some(p) } => {
                Notice::new(Msg::ExportCancelled { count: rows, path: p.display().to_string() }, Level::Warning)
            }
            Outcome::Cancelled { partial: None, .. } => Notice::new(Label::ExportCancelledEmpty, Level::Warning),
            Outcome::Failed { why, rows, partial } => {
                let reason = self.export_failure_text(&why);
                let m = match partial {
                    Some(p) => Msg::ExportFailed { count: rows, reason, path: p.display().to_string() },
                    None => Msg::ExportFailedEmpty { reason },
                };
                Notice::new(m, Level::Error)
            }
        };
        self.export_said(tab, notice);
    }

    /// Why an export failed, in words.
    fn export_failure_text(&self, why: &Failure) -> String {
        match why {
            Failure::Io(fault, path) => {
                let error = self.fault_text("export.write_failed", fault);
                self.i18n.msg(&Msg::ExportWhyIo { path: path.display().to_string(), error }).to_string()
            }
            Failure::Read(fault) => {
                let error = self.fault_text("export.read_failed", fault);
                self.i18n.msg(&Msg::ExportWhyRead { error }).to_string()
            }
            Failure::NotAFile(p) => {
                self.i18n.msg(&Msg::ExportWhyNotAFile { path: p.display().to_string() }).to_string()
            }
            Failure::PartialInTheWay(p) => {
                self.i18n.msg(&Msg::ExportWhyPartialInTheWay { path: p.display().to_string() }).to_string()
            }
            Failure::TargetAppeared(p) => {
                self.i18n.msg(&Msg::ExportWhyTargetAppeared { path: p.display().to_string() }).to_string()
            }
            Failure::SchemaChanged => self.i18n.label(Label::ExportWhySchemaChanged).to_string(),
            Failure::NotWhole(r) => self.i18n.msg(&self.not_whole(r).msg).to_string(),
            Failure::SessionEnded => self.i18n.label(Label::ExportWhySessionEnded).to_string(),
            Failure::MySqlLost => self.i18n.label(Label::ExportWhyMysqlLost).to_string(),
            Failure::Stale => self.i18n.label(Label::ExportStale).to_string(),
            Failure::Db(DbError::NotRepeatable(r)) => self.i18n.msg(&super::pages::why(r)).to_string(),
            Failure::Db(DbError::NotSupported) => self.i18n.label(Label::ExportWhyNotSupported).to_string(),
            Failure::Db(e) => self.db_error_text(e),
            Failure::Key => self.i18n.label(Label::ExportWhyKey).to_string(),
            Failure::Internal => self.i18n.label(Label::ExportWhyInternal).to_string(),
        }
    }

    /// What tab `id`'s export has done so far, for its status: rows, size, time, and how far
    /// when the total is known (`~` for an estimate, never 100% before the end).
    pub(super) fn export_progress(&self, id: TabId) -> Option<Msg> {
        let j = self.exports.jobs.get(&id)?;
        let file = j.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let secs = self.now().saturating_duration_since(j.started).as_secs();
        let time = format!("{}:{:02}", secs / 60, secs % 60);
        let size = crate::text::human_bytes(j.bytes);
        Some(match j.total {
            Some((total, exact)) if total > 0 => {
                let p = (j.rows.saturating_mul(100) / total).min(99);
                let percent = if exact { format!("{p}%") } else { format!("~{p}%") };
                Msg::ExportProgressPercent { count: j.rows, file, size, time, percent }
            }
            _ => Msg::ExportProgress { count: j.rows, file, size, time },
        })
    }

    /// How the SQL formats, and every format, write the columns `cols` of the active tab's
    /// result; why not, as a notice.
    /// Why `req` cannot write the columns `cols` of the active tab's result read as `read_in`
    /// ([`App::export_spec`]'s refusal).
    pub(super) fn export_refused(&self, cols: &[usize], req: &ExportRequest, read_in: &ReadIn) -> Option<Notice> {
        self.export_spec(cols, req, read_in).err()
    }

    fn export_spec(&self, cols: &[usize], req: &ExportRequest, read_in: &ReadIn) -> Result<Spec, Notice> {
        let t = self.tab();
        let Results::Rows(rs) = &t.results else { return Err(Notice::new(Label::ExportNothing, Level::Warning)) };
        let d = self.tab_dialect(t.id);
        let metas: Vec<&ColumnMeta> = cols.iter().map(|&c| &rs.columns[c].meta).collect();
        let columns: Vec<(String, Kind)> = metas.iter().map(|m| (m.name.clone(), Kind::of_column(m))).collect();
        let keys = self.tab_keys(t).catalog();
        let sql_cells = |insert: bool| {
            let cs: Vec<core_export::SqlColumn> = metas
                .iter()
                .map(|m| {
                    let auto = keys.and_then(|k| m.origin.as_ref().and_then(|o| k.column(o)));
                    core_export::SqlColumn {
                        name: &m.name,
                        type_name: &m.type_name,
                        kind: m.kind,
                        auto_increment: insert && auto.is_some_and(|c| c.generated == Generated::AutoIncrement),
                    }
                })
                .collect();
            core_export::sql_cells(d, &cs, read_in, keys.and_then(|k| k.server_zone()))
        };
        let origins: Vec<_> = metas.iter().map(|m| m.origin.clone()).collect();
        let names: Vec<&str> = metas.iter().map(|m| m.name.as_str()).collect();
        let (cells, sql) = match req.format {
            ExportFormat::SqlInsert => {
                let plan = insert_source(d, keys, &origins, &names, t.shown_sql())
                    .map_err(|why| Notice::new(Msg::CopyInsertRefused { reason: self.why(&why) }, Level::Warning))?;
                let InsertPlan { table, columns: written, overriding, .. } = plan;
                let cells = sql_cells(true).map_err(|r| self.not_whole(&r))?;
                let target = SqlTarget::Insert {
                    dialect: d,
                    schema: table.schema.clone(),
                    name: table.name.clone(),
                    written: written.iter().map(|(i, n)| (*i, n.to_string())).collect(),
                    overriding,
                };
                (Some(cells), Some(target))
            }
            ExportFormat::SqlUpdate => {
                let plan = update_source(d, keys, &origins, &names, t.shown_sql()).map_err(|why| {
                    Notice::new(Msg::CopyUpdateRefused { reason: self.why_not_update(&why) }, Level::Warning)
                })?;
                let cells = sql_cells(false).map_err(|r| self.not_whole(&r))?;
                let own = |v: &[(usize, &str)]| v.iter().map(|(i, n)| (*i, n.to_string())).collect();
                let target = SqlTarget::Update {
                    dialect: d,
                    schema: plan.table.schema.clone(),
                    name: plan.table.name.clone(),
                    set: own(&plan.set),
                    keys: own(&plan.keys),
                };
                (Some(cells), Some(target))
            }
            _ => (None, None),
        };
        Ok(Spec {
            format: req.format,
            csv: req.csv.clone(),
            columns,
            pick: cols.to_vec(),
            cells,
            sql,
            source: rs.columns.iter().map(|c| c.meta.name.clone()).collect(),
        })
    }
}

/// Where a writer's rows come from.
enum Source {
    Local(RowsSnapshot),
    /// Statements of the tab's session, a channel each ([`Ctl::Rows`]).
    Db,
}

/// Start the writer of job `job` (tab `tab`): it opens `path` + `.partial` (in `fallback`, the
/// home folder, when the default folder is missing), writes the rows `source` gives as `spec`
/// says, and ends with exactly one [`ExportNews::Ended`].
#[allow(clippy::too_many_arguments)]
fn spawn_writer(
    job: u64,
    tab: TabId,
    spec: Spec,
    path: PathBuf,
    overwrite: bool,
    fallback: Option<PathBuf>,
    source: Source,
    out: Out,
) -> (CtlSender, Option<std::thread::JoinHandle<()>>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let thread_out = out.clone();
    let spawned = std::thread::Builder::new().name("datarig-export".into()).spawn(move || {
        let mut end = EndGuard { out: thread_out, job, tab, sent: false, partial: Default::default() };
        let outcome = write(&end, &spec, path, overwrite, fallback, source, &mut rx);
        end.send(ExportNews::Ended(outcome));
        end.sent = true;
    });
    match spawned {
        Ok(handle) => (tx, Some(handle)),
        Err(_) => {
            out.send(AppEvent::Export(ExportEvent {
                job,
                tab,
                news: ExportNews::Ended(Outcome::Failed { why: Failure::Internal, rows: 0, partial: None }),
            }));
            (tx, None)
        }
    }
}

/// Sends the writer's events, and its terminal one when the thread ends without it (a panic).
struct EndGuard {
    out: Out,
    job: u64,
    tab: TabId,
    sent: bool,
    /// The partial file, once it was made: a writer that stops without its terminal event says
    /// where it is.
    partial: std::cell::RefCell<Option<PathBuf>>,
}

impl EndGuard {
    fn send(&self, news: ExportNews) {
        self.out.send(AppEvent::Export(ExportEvent { job: self.job, tab: self.tab, news }));
    }
}

impl Drop for EndGuard {
    fn drop(&mut self) {
        if !self.sent {
            let partial = self.partial.borrow_mut().take().filter(|p| p.exists());
            self.send(ExportNews::Ended(Outcome::Failed { why: Failure::Internal, rows: 0, partial }));
        }
    }
}

/// The file being written, counting what went in.
struct Progress<'a> {
    end: &'a EndGuard,
    last: Instant,
}

impl Progress<'_> {
    fn tick(&mut self, rows: u64, bytes: u64, force: bool) {
        if force || self.last.elapsed() >= PROGRESS_EVERY {
            self.last = Instant::now();
            self.end.send(ExportNews::Progress { rows, bytes });
        }
    }
}

/// How a writer's rows stopped.
enum Stopped {
    Done,
    Cancelled,
    Failed(Failure),
}

/// The writer's work ([`spawn_writer`]).
#[allow(clippy::too_many_arguments)]
fn write(
    end: &EndGuard,
    spec: &Spec,
    path: PathBuf,
    overwrite: bool,
    fallback: Option<PathBuf>,
    source: Source,
    ctl: &mut tokio::sync::mpsc::UnboundedReceiver<Ctl>,
) -> Outcome {
    // The default folder may be missing: the home folder then.
    let path = match (&fallback, path.parent(), path.file_name()) {
        (Some(home), Some(dir), Some(name)) if !dir.is_dir() => home.join(name),
        _ => path,
    };
    match std::fs::symlink_metadata(&path) {
        // A folder or a link is never replaced (renaming over it would fail, or replace the
        // link rather than what it names).
        Ok(m) if !m.is_file() => return failed(Failure::NotAFile(path), 0, None),
        Ok(_) if !overwrite => {
            let rows = match source {
                Source::Local(s) => Some(s),
                Source::Db => None,
            };
            return Outcome::Exists { path, rows };
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return failed(Failure::Io(Fault::io_at(&e, &path), path.clone()), 0, None),
    }
    let partial = partial_path(&path);
    let file = match std::fs::OpenOptions::new().write(true).create_new(true).open(&partial) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return failed(Failure::PartialInTheWay(partial), 0, None);
        }
        Err(e) => return failed(Failure::Io(Fault::io_at(&e, &partial), partial.clone()), 0, None),
    };
    *end.partial.borrow_mut() = Some(partial.clone());
    end.send(ExportNews::Ready { path: path.clone() });
    let columns: Vec<core_export::Column> =
        spec.columns.iter().map(|(name, kind)| core_export::Column { name, kind: *kind }).collect();
    let written: Vec<core_export::Column> = match &spec.sql {
        Some(SqlTarget::Insert { written, .. }) => written.iter().map(|(i, _)| columns[*i]).collect(),
        _ => columns.clone(),
    };
    let format = match (&spec.format, &spec.sql) {
        (ExportFormat::Csv, _) => core_export::Format::Delimited(spec.csv.clone()),
        (ExportFormat::Tsv, _) => core_export::Format::Tsv { header: spec.csv.header },
        (ExportFormat::Json, _) => core_export::Format::Json,
        (ExportFormat::JsonPretty, _) => core_export::Format::JsonPretty,
        (ExportFormat::JsonLines, _) => core_export::Format::JsonLines,
        (ExportFormat::Markdown, _) => core_export::Format::Markdown,
        (ExportFormat::Html, _) => core_export::Format::Html,
        (ExportFormat::Xml, _) => core_export::Format::Xml,
        (_, Some(SqlTarget::Insert { dialect, schema, name, written, overriding })) => core_export::Format::Sql(
            *dialect,
            core_export::Target::Table {
                schema,
                name,
                columns: written.iter().map(|(_, n)| n.as_str()).collect(),
                overriding: *overriding,
            },
        ),
        (_, Some(SqlTarget::Update { dialect, schema, name, set, keys })) => core_export::Format::Update(
            *dialect,
            core_export::UpdateTarget {
                schema,
                name,
                set: set.iter().map(|(i, n)| (*i, n.as_str())).collect(),
                keys: keys.iter().map(|(i, n)| (*i, n.as_str())).collect(),
            },
        ),
        (ExportFormat::SqlInsert | ExportFormat::SqlUpdate, None) => {
            drop(file);
            let _ = std::fs::remove_file(&partial);
            return failed(Failure::Internal, 0, None);
        }
    };
    let mut w = TextWriter::new(format, &written, BufWriter::with_capacity(BUFFER, file)).trailing_newline(true);
    let mut progress = Progress { end, last: Instant::now() };
    let io = |e: std::io::Error| Failure::Io(Fault::io_at(&e, &partial), partial.clone());
    let stopped = match w.begin().map_err(io) {
        Err(f) => Stopped::Failed(f),
        Ok(()) => match source {
            Source::Local(snap) => local_rows(&mut w, spec, &snap, ctl, &mut progress, &io),
            Source::Db => db_rows(&mut w, spec, ctl, &mut progress, &io),
        },
    };
    let rows = w.rows_written();
    // The format is ended whatever stopped it: the rows so far make a file that reads.
    let ended = w.end().map_err(io);
    let (bytes, damaged) = (w.bytes(), w.damaged());
    let ended = ended.and_then(|()| {
        let file = w.into_inner().into_inner().map_err(|e| io(e.into_error()))?;
        file.sync_all().map_err(io)
    });
    progress.tick(rows, bytes, true);
    let stopped = match (stopped, ended) {
        (Stopped::Done, Err(f)) => Stopped::Failed(f),
        (s, _) => s,
    };
    // Nothing to keep without a row.
    let keep = |rows: u64| -> Option<PathBuf> {
        if rows > 0 {
            Some(partial.clone())
        } else {
            let _ = std::fs::remove_file(&partial);
            None
        }
    };
    match stopped {
        Stopped::Done => {
            let renamed = if overwrite {
                std::fs::rename(&partial, &path).inspect(|()| {
                    if let Some(dir) = path.parent() {
                        datarig_core::fsutil::sync_dir(dir);
                    }
                })
            } else {
                datarig_core::fsutil::rename_new(&partial, &path)
            };
            match renamed {
                Ok(()) => Outcome::Done { rows, bytes, path, damaged },
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    failed(Failure::TargetAppeared(path), rows, Some(partial.clone()))
                }
                Err(e) => failed(Failure::Io(Fault::io_at(&e, &path), path.clone()), rows, Some(partial.clone())),
            }
        }
        Stopped::Cancelled => Outcome::Cancelled { rows, partial: keep(rows) },
        Stopped::Failed(why) => failed(why, rows, keep(rows)),
    }
}

fn failed(why: Failure, rows: u64, partial: Option<PathBuf>) -> Outcome {
    Outcome::Failed { why, rows, partial }
}

/// The rows of `snap`, a chunk at a time; a cancel stops them before the next chunk.
fn local_rows<W: Write>(
    w: &mut TextWriter<'_, W>,
    spec: &Spec,
    snap: &RowsSnapshot,
    ctl: &mut tokio::sync::mpsc::UnboundedReceiver<Ctl>,
    progress: &mut Progress<'_>,
    io: &dyn Fn(std::io::Error) -> Failure,
) -> Stopped {
    let mut stopped = Stopped::Done;
    let read = snap.for_each_chunk(BATCH_ROWS, |rows| {
        match ctl.try_recv() {
            Ok(Ctl::Cancel) => {
                stopped = Stopped::Cancelled;
                return false;
            }
            Ok(Ctl::Fail(why)) => {
                stopped = Stopped::Failed(why);
                return false;
            }
            Ok(Ctl::Rows(_) | Ctl::Finish) | Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
            // The app is gone: what was written is kept.
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                stopped = Stopped::Failed(Failure::SessionEnded);
                return false;
            }
        }
        if let Err(f) = write_rows(w, spec, rows, io) {
            stopped = Stopped::Failed(f);
            return false;
        }
        progress.tick(w.rows_written(), w.bytes(), false);
        true
    });
    match read {
        Err(fault) => Stopped::Failed(Failure::Read(fault)),
        Ok(()) => stopped,
    }
}

/// What the writer waits for next while a statement's rows come: a batch, or word from the app.
enum Next {
    Batch(Option<ExportBatch>),
    Ctl(Option<Ctl>),
}

/// The rows of the statements the app hands over, until it says the export is over. While a
/// statement's rows come, word from the app is heard too: a session that never closes its end of
/// the channel (a wedged driver, closed by the app) cannot keep the writer waiting.
fn db_rows<W: Write>(
    w: &mut TextWriter<'_, W>,
    spec: &Spec,
    ctl: &mut tokio::sync::mpsc::UnboundedReceiver<Ctl>,
    progress: &mut Progress<'_>,
    io: &dyn Fn(std::io::Error) -> Failure,
) -> Stopped {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().build() else {
        return Stopped::Failed(Failure::Internal);
    };
    // The rows of the statement being read, then those of the next one the app handed over.
    let mut rows: Option<tokio::sync::mpsc::Receiver<ExportBatch>> = None;
    let mut queued: std::collections::VecDeque<tokio::sync::mpsc::Receiver<ExportBatch>> = Default::default();
    // The app's word taken between two batches.
    let mut said: Option<Next> = None;
    loop {
        if rows.is_none() {
            rows = queued.pop_front();
        }
        let next = match (said.take(), rows.as_mut()) {
            (Some(n), _) => n,
            // Rows first: what the session sent before it answered is written before the app's
            // word that followed its answer.
            (None, Some(rx)) => rt.block_on(async {
                tokio::select! {
                    biased;
                    b = rx.recv() => Next::Batch(b),
                    c = ctl.recv() => Next::Ctl(c),
                }
            }),
            (None, None) => Next::Ctl(rt.block_on(ctl.recv())),
        };
        match next {
            Next::Batch(Some(ExportBatch::Columns(cols))) => {
                let same = cols.len() == spec.source.len() && cols.iter().zip(&spec.source).all(|(c, n)| c.name == *n);
                if !same {
                    // Dropping the receiver stops the statement.
                    return Stopped::Failed(Failure::SchemaChanged);
                }
            }
            Next::Batch(Some(ExportBatch::Rows(batch))) => {
                if let Err(f) = write_rows(w, spec, &batch, io) {
                    return Stopped::Failed(f);
                }
                progress.tick(w.rows_written(), w.bytes(), false);
                // A session that always has rows ready would keep the app's word (a cancel, a
                // quit) waiting: it is looked at after every batch.
                said = match ctl.try_recv() {
                    Ok(c) => Some(Next::Ctl(Some(c))),
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => Some(Next::Ctl(None)),
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => None,
                };
            }
            // That statement's rows are all here.
            Next::Batch(None) => rows = None,
            Next::Ctl(Some(Ctl::Rows(rx))) => queued.push_back(rx),
            Next::Ctl(Some(Ctl::Finish)) => {
                // Sent once the session answered: whatever it still has in the channel first.
                while let Some(rx) = rows.as_mut().or(queued.front_mut()) {
                    match rt.block_on(rx.recv()) {
                        Some(ExportBatch::Rows(batch)) => {
                            if let Err(f) = write_rows(w, spec, &batch, io) {
                                return Stopped::Failed(f);
                            }
                        }
                        Some(ExportBatch::Columns(_)) => {}
                        None if rows.is_some() => rows = None,
                        None => {
                            queued.pop_front();
                        }
                    }
                }
                return Stopped::Done;
            }
            Next::Ctl(Some(Ctl::Cancel)) => return Stopped::Cancelled,
            Next::Ctl(Some(Ctl::Fail(why))) => return Stopped::Failed(why),
            // The app forgot the job (its tab, its session): what was written is kept.
            Next::Ctl(None) => return Stopped::Failed(Failure::SessionEnded),
        }
    }
}

/// Write `rows` of the source as `spec` says: the picked columns, SQL's values as its rules
/// write them (a value that is not written whole fails the export, with its column).
fn write_rows<W: Write>(
    w: &mut TextWriter<'_, W>,
    spec: &Spec,
    rows: &[Vec<Cell>],
    io: &dyn Fn(std::io::Error) -> Failure,
) -> Result<(), Failure> {
    // A row at a time: what is formatted and held beside the batch is one row's text, however
    // wide the rows.
    for r in rows {
        let picked = spec
            .pick
            .iter()
            .enumerate()
            .map(|(i, &c)| {
                let v = r.get(c).and_then(|v| v.as_deref());
                match (&spec.cells, v) {
                    (Some(cells), Some(v)) => cells.cell(i, v).map(Some),
                    (_, v) => Ok(v.map(Cow::Borrowed)),
                }
            })
            .collect::<Result<Vec<Option<Cow<str>>>, Refused>>()
            .map_err(Failure::NotWhole)?;
        let out: core_export::Row = match &spec.sql {
            Some(SqlTarget::Insert { written, .. }) => written.iter().map(|(i, _)| picked[*i].as_deref()).collect(),
            _ => picked.iter().map(|v| v.as_deref()).collect(),
        };
        w.rows(&[out]).map_err(io)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
