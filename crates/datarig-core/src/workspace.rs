//! The workspace state kept between runs,
//! in the state directory:
//!
//! * `workspace.toml`: the open tabs (kind, console id or script path, profile id, cursor and
//!   scroll), the active tab, and which explorer folders are open.
//! * `consoles/<id>.sql`: the text of each console tab.
//! * `consoles/.trash/<millis>-<id>.sql`: the text of console tabs the user closed
//!   ([`trash_console`]), newest last by the time in the name; `Space t u` and `:recover` bring
//!   them back, across restarts too ([`list_trash`], [`untrash`]). Nothing else ever goes
//!   there: a console file no restored tab refers to (after a crash, a bug or a damaged
//!   `workspace.toml`) is never moved or deleted, it comes back as a "recovered" tab. At
//!   launch [`trim_trash`] deletes a trashed file only when it is both beyond the newest
//!   [`TRASH_KEEP`] and older than [`TRASH_DAYS`] days: the only place a console's text is
//!   ever deleted.
//! * `lock`: held by the running instance. A second datarig finds it held and runs
//!   workspace-read-only (it restores nothing and writes none of these files).
//!
//! ```toml
//! version = 3
//! active = 1
//!
//! [explorer]
//! expanded_folders = ["work", "work/prod"]
//! scripts_expanded = true
//! script_folders = ["reports"]
//!
//! [[tabs]]
//! id = "7d7c…"            # console file name
//! kind = "console"        # console | script | table
//! console = 1             # the console's number (`console 1`)
//! profile = "3f0b8f5e-…"  # none: a tab without a connection
//! cursor = [12, 4]
//! top = 0
//! results = { share = 60, hidden = false, maximized = false }
//! context = { database = "sales", schema = "shop" }   # none: the profile's defaults
//!
//! [[tabs]]
//! id = "a1b2…"
//! kind = "script"
//! script = "reports/daily.sql"
//!
//! [[tabs]]
//! id = "c3d4…"
//! kind = "table"
//! schema = "shop"
//! table = "users"
//! profile = "3f0b8f5e-…"
//! ```
//!
//! Version 2 added `console`, the `table` kind and `results`; version 3
//! a query tab's `context` (its database and schema). A file of an earlier
//! version reads with their defaults, and is copied to `workspace.toml.v<n>.bak` (once, never
//! over an existing copy) before it is first written as this version. Unknown is not absent: a tab of
//! a kind this version does not know is kept as it is and written back, after the others (the
//! file's `active` counts every tab, so the right one opens), and every key this version does
//! not know (at the top, a table such as `[layout]`, in `[explorer]`, in a tab, in its
//! `results`) is written back as it was ([`save`] merges them from the file it replaces). A
//! file of a later version ([`Loaded::newer`]) is read for what this version knows, and never
//! written ([`save`] refuses to replace it).

use crate::fault::Fault;
use crate::fsutil::{atomic_write, create_new, free_backup, rename_new};
use crate::profile::ProfileId;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const FILE: &str = "workspace.toml";
pub const BACKUP: &str = "workspace.toml.bak";
/// The copy of a version 1 file made before it is first written as a later version (the copy
/// of a version `n` file is `workspace.toml.v<n>.bak`, [`backup_of`]).
pub const V1_BACKUP: &str = "workspace.toml.v1.bak";

/// The name of the copy of a version `n` file.
pub fn backup_of(version: i64) -> String {
    format!("workspace.toml.v{version}.bak")
}
pub const LOCK: &str = "lock";
pub const CONSOLES: &str = "consoles";
/// Inside [`CONSOLES`]: where the consoles of closed tabs go.
pub const TRASH: &str = ".trash";
/// The trash always keeps its newest this many files ...
pub const TRASH_KEEP: usize = 50;
/// ... and every file trashed less than this many days ago.
pub const TRASH_DAYS: u64 = 30;
/// The latest time a trash name may carry (the end of the year 9999, in milliseconds): a
/// larger one is not a name the trash made, so the file is left alone like any other.
pub const TRASH_MAX_MILLIS: u128 = 253_402_300_799_999;

/// The version of `workspace.toml` this build writes.
pub const VERSION: i64 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKind {
    Console,
    Script,
    /// A table or view opened from the explorer (no console file).
    Table,
}

/// The share of a results pane whose file does not say it (percent of the height).
pub const DEFAULT_SHARE: u16 = 60;

/// A query tab's results pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneState {
    /// Percent of the height the results take.
    pub share: u16,
    pub hidden: bool,
    pub maximized: bool,
}

/// One restored tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabState {
    /// The console's file name (`consoles/<id>.sql`); written for scripts too, but a restored
    /// saved query's tab gets a new one (it names no file).
    pub id: String,
    pub kind: TabKind,
    /// A script's path relative to the scripts directory.
    pub script: Option<String>,
    pub profile: Option<ProfileId>,
    /// Row and column of the cursor.
    pub cursor: (usize, usize),
    /// First line shown.
    pub top: usize,
    /// A console's number (0: none written; the app numbers it).
    pub console: u32,
    /// A table tab's schema and table.
    pub table: Option<(String, String)>,
    /// The results pane (`None`: not written, the defaults).
    pub results: Option<PaneState>,
    /// A query tab's database and schema (each `None`: the profile's default).
    pub database: Option<String>,
    pub schema: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExplorerState {
    /// Open profile folders.
    pub expanded_folders: Vec<String>,
    /// The "Saved queries" section is open.
    pub scripts_expanded: bool,
    /// Open folders of the saved queries.
    pub script_folders: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceState {
    /// Index of the active tab.
    pub active: usize,
    pub explorer: ExplorerState,
    pub tabs: Vec<TabState>,
    /// Tabs of a kind this version does not know, as they were written (TOML of each table):
    /// written back after the others.
    pub unknown_tabs: Vec<String>,
}

/// What [`load`] found.
#[derive(Debug, Default)]
pub struct Loaded {
    pub state: WorkspaceState,
    /// There was a file (otherwise this is a first run).
    pub found: bool,
    /// The file could not be read: why, and where it was moved (`workspace.toml.bak`, or the
    /// next free `workspace.toml.bak.<n>`; `None` when it could not be moved).
    pub broken: Option<(Fault, Option<PathBuf>)>,
    /// Console tabs left out because an earlier tab has the same console id (see [`parse`]).
    pub duplicates: u64,
    /// The file's version when it is later than this build's ([`VERSION`]): what this version
    /// knows of it was read, and it must not be written ([`save`] refuses).
    pub newer: Option<i64>,
}

/// Read `<state>/workspace.toml`. A missing file is an empty workspace; a broken or unreadable
/// one is moved aside (a free `workspace.toml.bak` name, an older backup is never replaced)
/// and reported, and the workspace starts empty.
pub fn load(state: &Path) -> Loaded {
    let path = state.join(FILE);
    let moved = || {
        let bak = free_backup(&state.join(BACKUP));
        rename_new(&path, &bak).is_ok().then_some(bak)
    };
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Loaded::default(),
        Err(e) => return Loaded { found: true, broken: Some((Fault::io_at(&e, &path), moved())), ..Loaded::default() },
    };
    match parse(&text) {
        Ok((s, duplicates)) => {
            let newer = version_of(&text).filter(|v| *v > VERSION);
            Loaded { state: s, found: true, broken: None, duplicates, newer }
        }
        Err(e) => Loaded { found: true, broken: Some((e, moved())), ..Loaded::default() },
    }
}

/// The `version` a workspace file says (1 when it says none); `None` when it is not TOML.
fn version_of(text: &str) -> Option<i64> {
    let doc: toml::Table = text.parse().ok()?;
    Some(doc.get("version").and_then(toml::Value::as_integer).unwrap_or(1))
}

/// The state in `text`, and how many duplicate console tabs were left out. Tabs that are not
/// understood are skipped (their console files, if any, come back as recovered tabs).
///
/// One console file is open in one tab at most: two tabs on one file would each save over the
/// other, and closing one would trash the file under the other. The first console tab with an
/// id keeps it and later ones are left out.
fn parse(text: &str) -> Result<(WorkspaceState, u64), Fault> {
    let doc: toml::Table = text.parse().map_err(|e: toml::de::Error| Fault::toml_de(text, &e))?;
    let int = |v: Option<&toml::Value>| v.and_then(toml::Value::as_integer).map_or(0, |i| i.max(0) as usize);
    let strings = |v: Option<&toml::Value>| -> Vec<String> {
        v.and_then(toml::Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    // `active` counts every tab of the file, those of unknown kinds and those left out too.
    let active = int(doc.get("active"));
    let mut s = WorkspaceState::default();
    let tabs = doc.get("tabs").and_then(toml::Value::as_array);
    if let Some(e) = doc.get("explorer").and_then(toml::Value::as_table) {
        s.explorer = ExplorerState {
            expanded_folders: strings(e.get("expanded_folders")),
            scripts_expanded: e.get("scripts_expanded").and_then(toml::Value::as_bool).unwrap_or(false),
            script_folders: strings(e.get("script_folders")),
        };
    }
    let mut consoles = std::collections::HashSet::new();
    let mut duplicates = 0;
    for (pos, t) in tabs.into_iter().flatten().enumerate() {
        // The active tab is the one the file's index names, else the last one before it.
        if pos <= active {
            s.active = s.tabs.len().saturating_sub(1);
        }
        let Some(t) = t.as_table() else { continue };
        let kind = match t.get("kind").and_then(toml::Value::as_str) {
            Some("console") => TabKind::Console,
            Some("script") => TabKind::Script,
            Some("table") => TabKind::Table,
            // A later version's kind: kept, never dropped (a table without a kind is not a
            // tab of any version).
            Some(_) => {
                if let Ok(text) = toml::to_string(t) {
                    s.unknown_tabs.push(text);
                }
                continue;
            }
            None => continue,
        };
        let id = t.get("id").and_then(toml::Value::as_str).filter(|i| valid_id(i)).map(str::to_string);
        let script = t.get("script").and_then(toml::Value::as_str).map(str::to_string);
        if kind == TabKind::Script && script.is_none() {
            continue;
        }
        let text = |k: &str| t.get(k).and_then(toml::Value::as_str).map(str::to_string);
        let table = match (text("schema"), text("table")) {
            (Some(schema), Some(name)) => Some((schema, name)),
            _ => None,
        };
        if kind == TabKind::Table && table.is_none() {
            continue;
        }
        if let (TabKind::Console, Some(i)) = (kind, &id)
            && !consoles.insert(i.clone())
        {
            duplicates += 1;
            continue;
        }
        let context = t.get("context").and_then(toml::Value::as_table);
        let context_text = |k: &str| {
            context.and_then(|c| c.get(k)).and_then(toml::Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
        };
        let cursor = t.get("cursor").and_then(toml::Value::as_array);
        let at = |i: usize| int(cursor.and_then(|c| c.get(i)));
        let results = t.get("results").and_then(toml::Value::as_table).map(|r| PaneState {
            share: r.get("share").map_or(DEFAULT_SHARE, |v| u16::try_from(int(Some(v))).unwrap_or(u16::MAX)),
            hidden: r.get("hidden").and_then(toml::Value::as_bool).unwrap_or(false),
            maximized: r.get("maximized").and_then(toml::Value::as_bool).unwrap_or(false),
        });
        s.tabs.push(TabState {
            id: id.unwrap_or_else(new_id),
            kind,
            script,
            profile: t.get("profile").and_then(toml::Value::as_str).and_then(ProfileId::parse),
            cursor: (at(0), at(1)),
            top: int(t.get("top")),
            console: u32::try_from(int(t.get("console"))).unwrap_or(0),
            table,
            results,
            database: context_text("database"),
            schema: context_text("schema"),
        });
        if pos == active {
            s.active = s.tabs.len() - 1;
        }
    }
    Ok((s, duplicates))
}

/// Write `<state>/workspace.toml` (atomically). The file it replaces is read first: one of a
/// later version is never replaced (an error); one of version 1 is copied to [`V1_BACKUP`]
/// first, once; and every key of it this version does not know is written back ([`merge`]).
pub fn save(state: &Path, s: &WorkspaceState) -> io::Result<()> {
    let path = state.join(FILE);
    let old = match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let old_doc = old.as_deref().and_then(|t| t.parse::<toml_edit::DocumentMut>().ok());
    if let (Some(text), Some(version)) = (&old, old.as_deref().and_then(version_of)) {
        if version > VERSION {
            return Err(io::Error::other(format!("{FILE} is of a later version ({version})")));
        }
        if version < VERSION {
            match create_new(&state.join(backup_of(version)), text.as_bytes()) {
                Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),
                _ => {}
            }
        }
    }
    let mut doc = build(s);
    if let Some(old) = &old_doc {
        merge(old, &mut doc);
    }
    atomic_write(&path, doc.to_string().as_bytes())
}

/// The keys of the file this version writes, at the top, in `[explorer]`, in a tab and in its
/// `results`.
const TOP_KEYS: [&str; 4] = ["version", "active", "explorer", "tabs"];
const EXPLORER_KEYS: [&str; 3] = ["expanded_folders", "scripts_expanded", "script_folders"];
const TAB_KEYS: [&str; 11] =
    ["id", "kind", "script", "console", "schema", "table", "profile", "cursor", "top", "results", "context"];
const RESULTS_KEYS: [&str; 3] = ["share", "hidden", "maximized"];
const CONTEXT_KEYS: [&str; 2] = ["database", "schema"];

/// What a tab of the file is, to find it again in the next one: a console by its file, a saved
/// query by its path, a table by its profile and name (a restored saved query gets a new id).
fn tab_key(t: &dyn toml_edit::TableLike) -> Option<String> {
    let text = |k: &str| t.get(k).and_then(toml_edit::Item::as_str).unwrap_or_default().to_string();
    match t.get("kind").and_then(toml_edit::Item::as_str)? {
        "console" => Some(format!("console:{}", text("id"))),
        "script" => Some(format!("script:{}", text("script"))),
        "table" => Some(format!("table:{}:{}.{}", text("profile"), text("schema"), text("table"))),
        _ => None,
    }
}

/// Copy into `new` every key of `old` this version does not know: at the top (a table such as
/// `[layout]` too), in `[explorer]`, in each tab still there (found by [`tab_key`]) and in its
/// `results`. Unknown is not absent: a later version's keys survive this one's saves.
fn merge(old: &toml_edit::DocumentMut, new: &mut toml_edit::DocumentMut) {
    for (k, v) in old.iter() {
        if !TOP_KEYS.contains(&k) && !new.contains_key(k) {
            new[k] = v.clone();
        }
    }
    if let (Some(o), Some(n)) =
        (old.get("explorer").and_then(toml_edit::Item::as_table_like), new["explorer"].as_table_like_mut())
    {
        for (k, v) in o.iter() {
            if !EXPLORER_KEYS.contains(&k) && !n.contains_key(k) {
                n.insert(k, v.clone());
            }
        }
    }
    let Some(old_tabs) = old.get("tabs").and_then(toml_edit::Item::as_array_of_tables) else { return };
    let Some(new_tabs) = new["tabs"].as_array_of_tables_mut() else { return };
    for n in new_tabs.iter_mut() {
        let Some(key) = tab_key(n) else { continue };
        let Some(o) = old_tabs.iter().find(|o| tab_key(*o).as_deref() == Some(key.as_str())) else { continue };
        for (k, v) in o.iter() {
            if !TAB_KEYS.contains(&k) && !n.contains_key(k) {
                n.insert(k, v.clone());
            }
        }
        for (key, known) in [("results", &RESULTS_KEYS[..]), ("context", &CONTEXT_KEYS[..])] {
            let old_inner = o.get(key).and_then(toml_edit::Item::as_table_like);
            let new_inner = n.get_mut(key).and_then(toml_edit::Item::as_table_like_mut);
            match (old_inner, new_inner) {
                (Some(or), Some(nr)) => {
                    for (k, v) in or.iter() {
                        if !known.contains(&k) && !nr.contains_key(k) {
                            nr.insert(k, v.clone());
                        }
                    }
                }
                // This version wrote none for the tab (the defaults): only the keys it does not
                // know stay (the ones it knows are the defaults now).
                (Some(or), None) => {
                    let mut kept = toml_edit::InlineTable::new();
                    for (k, v) in or.iter() {
                        if let (false, Some(v)) = (known.contains(&k), v.as_value()) {
                            kept.insert(k, v.clone());
                        }
                    }
                    if !kept.is_empty() || key == "results" {
                        match (key, o.get(key)) {
                            ("results", Some(v)) => {
                                n.insert(key, v.clone());
                            }
                            _ => {
                                n.insert(key, toml_edit::value(kept));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// The document of `s`, as this version writes it.
fn build(s: &WorkspaceState) -> toml_edit::DocumentMut {
    let mut doc = toml_edit::DocumentMut::new();
    doc["version"] = toml_edit::value(VERSION);
    doc["active"] = toml_edit::value(s.active as i64);
    let list = |v: &[String]| toml_edit::value(v.iter().map(String::as_str).collect::<toml_edit::Array>());
    let mut e = toml_edit::Table::new();
    e["expanded_folders"] = list(&s.explorer.expanded_folders);
    e["scripts_expanded"] = toml_edit::value(s.explorer.scripts_expanded);
    e["script_folders"] = list(&s.explorer.script_folders);
    doc["explorer"] = toml_edit::Item::Table(e);
    let mut tabs = toml_edit::ArrayOfTables::new();
    for t in &s.tabs {
        let mut tt = toml_edit::Table::new();
        tt["id"] = toml_edit::value(t.id.as_str());
        tt["kind"] = toml_edit::value(match t.kind {
            TabKind::Console => "console",
            TabKind::Script => "script",
            TabKind::Table => "table",
        });
        if let Some(p) = &t.script {
            tt["script"] = toml_edit::value(p.as_str());
        }
        if t.kind == TabKind::Console && t.console > 0 {
            tt["console"] = toml_edit::value(i64::from(t.console));
        }
        if let Some((schema, name)) = &t.table {
            tt["schema"] = toml_edit::value(schema.as_str());
            tt["table"] = toml_edit::value(name.as_str());
        }
        if let Some(p) = t.profile {
            tt["profile"] = toml_edit::value(p.to_string());
        }
        tt["cursor"] = toml_edit::value(toml_edit::Array::from_iter([t.cursor.0 as i64, t.cursor.1 as i64]));
        tt["top"] = toml_edit::value(t.top as i64);
        if let Some(r) = t.results {
            let mut rt = toml_edit::InlineTable::new();
            rt.insert("share", i64::from(r.share).into());
            rt.insert("hidden", r.hidden.into());
            rt.insert("maximized", r.maximized.into());
            tt["results"] = toml_edit::value(rt);
        }
        if t.database.is_some() || t.schema.is_some() {
            let mut ct = toml_edit::InlineTable::new();
            if let Some(d) = &t.database {
                ct.insert("database", d.as_str().into());
            }
            if let Some(s) = &t.schema {
                ct.insert("schema", s.as_str().into());
            }
            tt["context"] = toml_edit::value(ct);
        }
        tabs.push(tt);
    }
    // Tabs of a later version, as they were read.
    for text in &s.unknown_tabs {
        if let Ok(d) = text.parse::<toml_edit::DocumentMut>() {
            tabs.push(d.as_table().clone());
        }
    }
    doc["tabs"] = toml_edit::Item::ArrayOfTables(tabs);
    doc
}

/// A new console id.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Only ids that are plain file names are used as such.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// `<state>/consoles/<id>.sql`.
pub fn console_path(state: &Path, id: &str) -> PathBuf {
    state.join(CONSOLES).join(format!("{id}.sql"))
}

/// The text of console `id` (`Ok(None)`: no file). A file that is there but cannot be read
/// is an error, never an empty console.
pub fn read_console(state: &Path, id: &str) -> io::Result<Option<String>> {
    match fs::read_to_string(console_path(state, id)) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Save the text of console `id` (atomically).
pub fn write_console(state: &Path, id: &str, text: &str) -> io::Result<()> {
    atomic_write(&console_path(state, id), text.as_bytes())
}

/// Remove console `id`'s file: only when its text was just saved elsewhere (the console
/// became a saved query). A missing file is fine.
pub fn remove_console(state: &Path, id: &str) -> io::Result<()> {
    match fs::remove_file(console_path(state, id)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The ids of the console files no id in `keep` refers to, sorted. Only files named like
/// the app names them (`<id>.sql`) count; anything else in the directory is left alone. No
/// directory is no files; a directory (or an entry) that cannot be read is an error, never
/// "no files".
pub fn orphan_consoles(state: &Path, keep: &[String]) -> io::Result<Vec<String>> {
    let rd = match fs::read_dir(state.join(CONSOLES)) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for e in rd {
        let e = e?;
        let Some(id) = e.file_name().to_str().and_then(|n| n.strip_suffix(".sql")).map(str::to_string) else {
            continue;
        };
        if valid_id(&id) && !keep.contains(&id) && !e.file_type()?.is_dir() {
            out.push(id);
        }
    }
    out.sort();
    Ok(out)
}

/// A file in the trash (named `<millis>-<id>.sql` by [`trash_console`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trashed {
    /// The file name.
    pub name: String,
    /// When it was trashed, in milliseconds since the Unix epoch.
    pub millis: u128,
    /// The id of the console it was.
    pub id: String,
}

impl Trashed {
    /// The file name's parts, if it is one [`trash_console`] makes (a time up to
    /// [`TRASH_MAX_MILLIS`]).
    fn parse(name: &str) -> Option<Trashed> {
        let (stamp, rest) = name.split_once('-')?;
        let id = rest.strip_suffix(".sql")?;
        if !valid_id(id) || stamp.len() < 13 || !stamp.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let millis = stamp.parse().ok().filter(|m| *m <= TRASH_MAX_MILLIS)?;
        Some(Trashed { name: name.to_string(), millis, id: id.to_string() })
    }
}

/// The user closed console `id` whose text is `text`: write the text to the trash under a new
/// name ([`Trashed`], never over an existing file), then remove the console's own file. When
/// the removal fails the file stays and comes back as a recovered tab at the next launch.
pub fn trash_console(state: &Path, id: &str, text: &str) -> io::Result<Trashed> {
    trash_console_at(state, id, text, SystemTime::now())
}

/// [`trash_console`] at time `now` (tests).
pub fn trash_console_at(state: &Path, id: &str, text: &str, now: SystemTime) -> io::Result<Trashed> {
    let dir = state.join(CONSOLES).join(TRASH);
    // Later than everything in the trash, so names keep increasing even for several closes
    // within one millisecond (a trash that cannot be listed just starts from `now`).
    let now = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    // A newest time at the very end of the range gives no later one: `now` then.
    let newest = list_trash(state).ok().and_then(|l| l.first().map(|t| t.millis));
    let next = newest.and_then(|n| n.checked_add(1)).filter(|n| *n <= TRASH_MAX_MILLIS);
    let mut millis = next.map_or(now, |n| now.max(n));
    let trashed = loop {
        let name = format!("{millis:013}-{id}.sql");
        match create_new(&dir.join(&name), text.as_bytes()) {
            Ok(()) => break Trashed { name, millis, id: id.to_string() },
            // Taken (another instance, a clock that went back): the next name.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                millis = millis.checked_add(1).ok_or_else(|| io::Error::other("no free name in the trash"))?;
            }
            Err(e) => return Err(e),
        }
    };
    remove_console(state, id)?;
    Ok(trashed)
}

/// What is in the trash, newest first (by the time in the name, then by name). Files the trash
/// did not name are left out. No trash is an empty one; one that cannot be read is an error.
pub fn list_trash(state: &Path) -> io::Result<Vec<Trashed>> {
    let rd = match fs::read_dir(state.join(CONSOLES).join(TRASH)) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for e in rd {
        let e = e?;
        if let Some(t) = e.file_name().to_str().and_then(Trashed::parse)
            && e.file_type()?.is_file()
        {
            out.push(t);
        }
    }
    out.sort_by(|a, b| (b.millis, &b.name).cmp(&(a.millis, &a.name)));
    Ok(out)
}

/// Why [`untrash`] failed; the trashed file stays in the trash either way.
#[derive(Debug)]
pub enum UntrashError {
    /// The trashed file cannot be read (a missing one is `NotFound`).
    Unreadable(io::Error),
    /// Its text could not be put back as a console, or the trashed file removed.
    Io(io::Error),
}

/// Take trashed file `name` back as a console: its text goes to a new console file (a new
/// id, written without replacing anything), then the trashed file is removed. Returns the new
/// console's id and text.
pub fn untrash(state: &Path, name: &str) -> Result<(String, String), UntrashError> {
    let from = state.join(CONSOLES).join(TRASH).join(name);
    let text = fs::read_to_string(&from).map_err(UntrashError::Unreadable)?;
    let id = new_id();
    create_new(&console_path(state, &id), text.as_bytes()).map_err(UntrashError::Io)?;
    fs::remove_file(&from).map_err(UntrashError::Io)?;
    Ok((id, text))
}

/// Trim the trash (at launch only, never together with putting something in it). Files are
/// ordered by the time in their name, then by name; a file is deleted only when it is both
/// outside the newest [`TRASH_KEEP`] and trashed more than [`TRASH_DAYS`] days ago. Only files
/// the trash named are touched. Returns how many were deleted.
pub fn trim_trash(state: &Path, now: SystemTime) -> io::Result<usize> {
    let files = list_trash(state)?;
    let now = now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let max_age = u128::from(TRASH_DAYS) * 24 * 60 * 60 * 1000;
    let dir = state.join(CONSOLES).join(TRASH);
    let mut deleted = 0;
    // `files` is newest first: past the ones the count keeps, oldest first.
    for t in files.iter().skip(TRASH_KEEP).rev() {
        if now.saturating_sub(t.millis) > max_age {
            fs::remove_file(dir.join(&t.name))?;
            deleted += 1;
        }
    }
    Ok(deleted)
}

// ── the instance lock ───────────────────────────────────────────────────────

/// The lock of the running instance: `<state>/lock`, held with an OS file lock for as long as
/// this value lives, with the owner's process id inside. The OS releases the lock when the
/// process ends, however it ends, so a crashed instance never blocks the next one.
pub struct InstanceLock {
    _file: File,
    pub path: PathBuf,
    /// Held without an OS lock (the file system has none): only the process id protects it.
    pub pid_only: bool,
}

/// What [`acquire`] got.
pub enum Acquired {
    /// This process owns the workspace.
    Owned(InstanceLock),
    /// Another datarig holds it (its process id, when readable).
    Held { pid: Option<u32> },
}

/// Take `<state>/lock`. The OS lock decides who owns the workspace. On a file system without
/// file locks the process id in the file decides: a lock whose process is gone (it crashed) is
/// taken over, a live one is respected.
pub fn acquire(state: &Path) -> io::Result<Acquired> {
    fs::create_dir_all(state)?;
    let path = state.join(LOCK);
    let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path)?;
    let pid_only = match file.try_lock() {
        Ok(()) => false,
        Err(fs::TryLockError::WouldBlock) => return Ok(Acquired::Held { pid: read_pid(&mut file) }),
        Err(fs::TryLockError::Error(e)) if e.kind() == io::ErrorKind::Unsupported => {
            match read_pid(&mut file) {
                Some(pid) if pid != std::process::id() && pid_alive(pid) => {
                    return Ok(Acquired::Held { pid: Some(pid) });
                }
                // No owner, or one that is gone: a stale lock, taken over.
                _ => true,
            }
        }
        Err(fs::TryLockError::Error(e)) => return Err(e),
    };
    file.set_len(0)?;
    use std::io::Seek;
    file.seek(io::SeekFrom::Start(0))?;
    writeln!(file, "{}", std::process::id())?;
    file.sync_all()?;
    Ok(Acquired::Owned(InstanceLock { _file: file, path, pid_only }))
}

/// The process id in a lock file.
pub fn read_pid(file: &mut File) -> Option<u32> {
    let mut s = String::new();
    use std::io::Seek;
    file.seek(io::SeekFrom::Start(0)).ok()?;
    file.read_to_string(&mut s).ok()?;
    s.trim().parse().ok()
}

/// Whether process `pid` is running (asked through the OS's own tools, without `unsafe`).
pub fn pid_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }
    #[cfg(unix)]
    {
        if Path::new("/proc/self").exists() {
            return Path::new(&format!("/proc/{pid}")).exists();
        }
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
    #[cfg(windows)]
    {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().any(|w| w == pid.to_string()))
    }
    #[cfg(not(any(unix, windows)))]
    {
        true
    }
}

#[cfg(test)]
mod tests;
