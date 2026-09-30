//! Config file: `--config <path>`, else `$XDG_CONFIG_HOME/datarig/config.toml`,
//! else `~/.config/datarig/config.toml`. A missing default file means zero profiles; it is
//! created (as `version = 2`) on the first save.
//!
//! The file is also *written* (settings changed in the app, connection profiles). [`save`]
//! edits the existing document with `toml_edit`, so comments and key order survive, and never
//! writes passwords (they live in the OS keychain or the secrets file, or come from a command,
//! an environment variable or a prompt; see `secret.rs`). A legacy plaintext `password` is kept
//! only until it has been moved to the profile's store (at launch, see [`crate::migrate`]). `last_used` remembers the profile the explorer's cursor starts on.
//!
//! Format version 2: every profile has a stable `id` (UUID), and
//! may have `folder`, `color`, `icon` and `policy`; `folders` keeps empty folders; `icons`
//! turns the Nerd Font icons on or off (`auto`, or no key: not decided yet, the app asks once);
//! `last_used` is a profile id. A file without `version`
//! is version 1 (the original format): it is read as well, and the launch-time migration
//! upgrades it once (one way: older binaries cannot read version 2).
//!
//! `theme` names the color theme: a built-in one or a file of `themes/` next to the config file
//! (see [`crate::theme`]; the UI resolves the name, so an unknown one never makes the file
//! unusable).
//! `[editor] mode` picks the editor key style (`vim` or `standard`), and
//! `[editor] cursor_shape` whether the cursor shows the editor's mode.
//! [`Prefs`] holds `[commands] position` (the `:` command line as a popup near
//! the top, or the bottom line), `detail_view` (the result inspector as a panel or a status bar
//! preview), `clipboard` (how copies reach the clipboard) and `copy_header`
//! (whether a copied block starts with the column names), `osc52_max_bytes` (the longest copy
//! sent through OSC 52).
//! `result_window_rows` (rows of a result kept in memory, default 10,000;
//! the rest go to a temporary file) and `spill_limit` (the most that file may take, default
//! 1 GB; also a policy item).
//! `[secrets] default_source` is the password source a new profile starts with (`auto`, the
//! default: the keychain when it works, else `prompt`). A profile's own source is
//! `password_source` (+ `password_command` / `password_env`), see [`crate::secret::source`].
//! `[keymap.<context>]` tables remap keys; core reads them as plain strings and never
//! rewrites them — the TUI validates the entries at startup.
//! `[policy.<name>]` tables are the safety policies profiles name (see [`crate::policy`]);
//! they are read only (saving keeps them as they are).

use crate::fault::{Fault, FaultKind};
use crate::policy::{self, Policies, Policy};
use crate::profile::color::ProfileColor;
use crate::profile::folder::{FolderPath, Folders};
use crate::profile::ssh::{SshProblem, SshSettings};
use crate::profile::{ConnectionConfig, ProfileId};
use crate::secret::source::{DefaultSource, SourceKind, valid_env_name};
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, Value};

/// `language` when the file does not set it: English, so bug
/// reports and screenshots share one language. `auto` (the locale) and `ko` are chosen.
pub const DEFAULT_LANGUAGE: &str = "en";

/// The config format this build writes.
pub const CONFIG_VERSION: u32 = 2;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    icons: Option<String>,
    #[serde(default)]
    theme: Option<String>,
    #[serde(default)]
    page_size: Option<usize>,
    #[serde(default)]
    result_window_rows: Option<i64>,
    #[serde(default)]
    spill_limit: Option<toml::Value>,
    #[serde(default)]
    folders: Vec<String>,
    #[serde(default)]
    connections: Vec<ConnectionConfig>,
    #[serde(default)]
    last_used: Option<String>,
    #[serde(default)]
    editor: Option<EditorSection>,
    #[serde(default)]
    secrets: Option<SecretsSection>,
    #[serde(default)]
    commands: Option<CommandsSection>,
    #[serde(default)]
    detail_view: Option<String>,
    #[serde(default)]
    clipboard: Option<String>,
    #[serde(default)]
    copy_header: Option<String>,
    #[serde(default)]
    osc52_max_bytes: Option<i64>,
    #[serde(default)]
    keymap: KeymapConfig,
    #[serde(default)]
    policy: BTreeMap<String, PolicySection>,
}

/// `[policy.<name>]`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicySection {
    #[serde(default)]
    paging_idle_timeout: Option<toml::Value>,
    #[serde(default)]
    spill_limit: Option<toml::Value>,
    #[serde(default)]
    read_only: Option<toml::Value>,
    #[serde(default)]
    confirm: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditorSection {
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    cursor_shape: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretsSection {
    #[serde(default)]
    default_source: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandsSection {
    #[serde(default)]
    position: Option<String>,
}

/// A setting with a fixed list of values, each with its name in the file.
pub trait Choice: Copy + Eq + Default + 'static {
    /// Every value, the default first.
    const ALL: &'static [Self];
    fn as_str(self) -> &'static str;
    fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|v| v.as_str().eq_ignore_ascii_case(s))
    }
}

/// A [`Choice`] setting `key` as the file has it (`None`: not set, the default).
fn choice<T: Choice>(key: &str, v: Option<String>, allowed: &'static str) -> Result<T, ConfigError> {
    match v {
        None => Ok(T::default()),
        Some(s) => T::parse(&s).ok_or_else(|| bad(key, &s).allowed(Some(allowed))),
    }
}

/// `[commands] position`: where the `:` command line is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CommandsPosition {
    /// A box near the top of the screen, the candidates below the input (noice.nvim style).
    #[default]
    Popup,
    /// The last line of the screen, the candidates rising above it (Neovim style).
    Bottom,
}

impl Choice for CommandsPosition {
    const ALL: &'static [Self] = &[CommandsPosition::Popup, CommandsPosition::Bottom];
    fn as_str(self) -> &'static str {
        match self {
            CommandsPosition::Popup => "popup",
            CommandsPosition::Bottom => "bottom",
        }
    }
}

/// `detail_view`: how the result inspector shows the selected cell.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DetailView {
    /// A panel on the right of the results (cell and row tabs).
    #[default]
    Panel,
    /// One line in the status bar.
    Statusbar,
}

impl Choice for DetailView {
    const ALL: &'static [Self] = &[DetailView::Panel, DetailView::Statusbar];
    fn as_str(self) -> &'static str {
        match self {
            DetailView::Panel => "panel",
            DetailView::Statusbar => "statusbar",
        }
    }
}

/// `clipboard`: how a copy reaches the clipboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClipboardSetting {
    /// OSC 52 in an SSH session, else the system clipboard, OSC 52 when that fails.
    #[default]
    Auto,
    System,
    Osc52,
}

impl Choice for ClipboardSetting {
    const ALL: &'static [Self] = &[ClipboardSetting::Auto, ClipboardSetting::System, ClipboardSetting::Osc52];
    fn as_str(self) -> &'static str {
        match self {
            ClipboardSetting::Auto => "auto",
            ClipboardSetting::System => "system",
            ClipboardSetting::Osc52 => "osc52",
        }
    }
}

/// `copy_header`: whether a copied block of cells starts with the column names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CopyHeader {
    /// With more than one row.
    #[default]
    Auto,
    On,
    Off,
}

impl Choice for CopyHeader {
    const ALL: &'static [Self] = &[CopyHeader::Auto, CopyHeader::On, CopyHeader::Off];
    fn as_str(self) -> &'static str {
        match self {
            CopyHeader::Auto => "auto",
            CopyHeader::On => "on",
            CopyHeader::Off => "off",
        }
    }
}

impl CopyHeader {
    /// Whether a copy of `rows` rows gets the header row.
    pub fn applies(self, rows: usize) -> bool {
        match self {
            CopyHeader::Auto => rows > 1,
            CopyHeader::On => true,
            CopyHeader::Off => false,
        }
    }
}

/// `[editor] cursor_shape`: whether the terminal's cursor takes the shape of the editor's mode
/// (a block in Normal and Visual, a bar in Insert and every text input).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CursorShape {
    #[default]
    On,
    /// The terminal's own cursor shape is left alone.
    Off,
}

impl Choice for CursorShape {
    const ALL: &'static [Self] = &[CursorShape::On, CursorShape::Off];
    fn as_str(self) -> &'static str {
        match self {
            CursorShape::On => "on",
            CursorShape::Off => "off",
        }
    }
}

/// The default of `osc52_max_bytes`: about 100 KB of base64 (75 KB of text). Terminals and
/// tmux drop longer OSC 52 sequences silently (tmux 3.x, many terminals cap them near here).
pub const OSC52_MAX_BYTES: usize = 100_000;

/// Display, result and clipboard settings. Each is written to the file only
/// when it differs from its default; set back to its default, its key is removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefs {
    pub commands_position: CommandsPosition,
    pub detail_view: DetailView,
    pub clipboard: ClipboardSetting,
    pub copy_header: CopyHeader,
    /// `osc52_max_bytes`: the longest OSC 52 payload (base64 bytes) a copy sends; a longer copy
    /// is not sent through the terminal (it would be dropped without a word).
    pub osc52_max_bytes: usize,
    /// `[editor] cursor_shape`.
    pub cursor_shape: CursorShape,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            commands_position: CommandsPosition::default(),
            detail_view: DetailView::default(),
            clipboard: ClipboardSetting::default(),
            copy_header: CopyHeader::default(),
            osc52_max_bytes: OSC52_MAX_BYTES,
            cursor_shape: CursorShape::default(),
        }
    }
}

/// `[keymap.<context>]` tables: context name -> key notation -> action id (or `"none"`).
pub type KeymapConfig = BTreeMap<String, BTreeMap<String, String>>;

/// Key style of the query editor (`[editor] mode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditorMode {
    /// Modal vim keys (the default).
    #[default]
    Vim,
    /// Modeless editing with the usual desktop keys.
    Standard,
}

impl EditorMode {
    pub fn as_str(self) -> &'static str {
        match self {
            EditorMode::Vim => "vim",
            EditorMode::Standard => "standard",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "vim" => Some(EditorMode::Vim),
            "standard" => Some(EditorMode::Standard),
            _ => None,
        }
    }
}

/// `icons`: Nerd Font icons next to profiles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IconsSetting {
    /// Not decided yet (no `icons` key, or `auto`, the older default): text marks,
    /// and the app asks once, with a preview of the glyphs, and saves the answer as `on` or
    /// `off`. The terminal's name says nothing about its font (over SSH it is not even sent).
    #[default]
    Auto,
    On,
    Off,
}

impl IconsSetting {
    pub fn as_str(self) -> &'static str {
        match self {
            IconsSetting::Auto => "auto",
            IconsSetting::On => "on",
            IconsSetting::Off => "off",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Some(IconsSetting::Auto),
            "on" => Some(IconsSetting::On),
            "off" => Some(IconsSetting::Off),
            _ => None,
        }
    }

    /// Whether icons are shown: only once the user said so (`auto` draws text marks).
    pub fn on(self) -> bool {
        self == IconsSetting::On
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    /// Format version of the file (1 when it has no `version`; [`CONFIG_VERSION`] for a new
    /// file).
    pub version: u32,
    pub language: String,
    pub icons: IconsSetting,
    /// `theme`: the name as written ([`crate::theme::DEFAULT`] without the key).
    pub theme: String,
    pub page_size: usize,
    /// Rows of a result kept in memory (`result_window_rows`); the rest spill to disk.
    pub result_window_rows: usize,
    /// The most a result's spill file may take (`spill_limit`; a policy may set its own).
    pub spill_limit: policy::SpillLimit,
    pub connections: Vec<ConnectionConfig>,
    /// Every folder (`folders` plus the folders of the profiles).
    pub folders: Folders,
    /// Profile connected to most recently (the explorer puts its cursor there at launch). A v1
    /// file's profile name is resolved to that profile's id.
    pub last_used: Option<ProfileId>,
    pub editor_mode: EditorMode,
    /// `[secrets] default_source`.
    pub default_source: DefaultSource,
    /// `[commands] position`, `detail_view`, `clipboard`, `copy_header`.
    pub prefs: Prefs,
    /// User key remapping, validated by the TUI.
    pub keymap: KeymapConfig,
    /// `[policy.<name>]` tables.
    pub policies: Policies,
    /// Where changes are saved. `None`: nothing is written (no home directory, or the file
    /// has errors and must not be overwritten).
    pub path: Option<PathBuf>,
    /// The file exists (a missing default file has nothing to migrate).
    pub exists: bool,
    /// Some profiles had no `id` in the file and got a new one in memory.
    pub ids_assigned: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            language: DEFAULT_LANGUAGE.into(),
            icons: IconsSetting::default(),
            theme: crate::theme::DEFAULT.into(),
            page_size: 500,
            result_window_rows: crate::results::WINDOW,
            spill_limit: policy::SpillLimit::default(),
            connections: Vec::new(),
            folders: Folders::default(),
            last_used: None,
            editor_mode: EditorMode::default(),
            default_source: DefaultSource::default(),
            prefs: Prefs::default(),
            keymap: KeymapConfig::new(),
            policies: Policies::default(),
            path: None,
            exists: false,
            ids_assigned: false,
        }
    }
}

impl Config {
    /// The launch-time migration has work to do: an old file, profiles without
    /// ids, or plaintext passwords. Never for a missing, unreadable or broken file.
    pub fn needs_migration(&self) -> bool {
        self.path.is_some()
            && self.exists
            && (self.version < CONFIG_VERSION
                || self.ids_assigned
                || self.connections.iter().any(has_movable_plaintext))
    }
}

/// A legacy plaintext password the launch-time migration moves (profiles whose source stores
/// passwords; a `command`, `env` or `prompt` profile never uses one).
pub fn has_movable_plaintext(c: &ConnectionConfig) -> bool {
    !c.password.is_empty() && c.password_source.unwrap_or(SourceKind::Keychain).stores_secret()
}

pub fn default_path(env: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(x) = env("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Some(Path::new(&x).join("datarig").join("config.toml"));
    }
    // `~/.config` on every OS; Windows usually has USERPROFILE instead of HOME.
    env("HOME")
        .filter(|v| !v.is_empty())
        .or_else(|| env("USERPROFILE").filter(|v| !v.is_empty()))
        .map(|h| Path::new(&h).join(".config").join("datarig").join("config.toml"))
}

/// Why the config file cannot be used. The app then runs with the defaults and no profiles,
/// and saves nothing. The UI words each case; nothing here is shown as it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// The file could not be read (`path` is the file).
    Read { path: PathBuf, fault: Fault },
    /// It is not valid TOML, or a key is unknown or a value has the wrong type.
    Syntax(Fault),
    /// A format version this build does not read.
    Version(u32),
    /// `key = value` is not valid (`profile`: in that profile; `allowed`: the literal values
    /// it takes, when there is a short list).
    Value { key: String, value: String, profile: Option<String>, allowed: Option<&'static str> },
    /// Two profiles have this id.
    DuplicateId(String),
    /// Profile `profile` has `password_source = "<source>"` but no `key`.
    Missing { key: &'static str, profile: String, source: &'static str },
    /// Profile `profile`'s SSH tunnel is on but `ssh.<key>` is empty.
    SshMissing { key: &'static str, profile: String },
}

/// A [`ConfigError::Value`] for a top-level key.
fn bad(key: &str, value: impl std::fmt::Display) -> ConfigError {
    ConfigError::Value { key: key.to_string(), value: value.to_string(), profile: None, allowed: None }
}

impl ConfigError {
    /// A [`ConfigError::Value`] that takes the values `list`.
    fn allowed(mut self, list: Option<&'static str>) -> Self {
        if let ConfigError::Value { allowed, .. } = &mut self {
            *allowed = list;
        }
        self
    }

    /// A [`ConfigError::Value`] in the table of profile `name`.
    fn in_profile(mut self, name: &str) -> Self {
        if let ConfigError::Value { profile, .. } = &mut self {
            *profile = Some(name.to_string());
        }
        self
    }
}

/// Load the config. Never fails: on error returns the defaults and **no** connections, no
/// path (nothing is written, nothing is migrated), plus the error to show as `error.config`.
pub fn load(cli_path: Option<PathBuf>) -> (Config, Option<ConfigError>) {
    let explicit = cli_path.is_some();
    let path = cli_path.or_else(|| default_path(|k| std::env::var(k).ok()));
    let Some(path) = path else {
        return (Config::default(), None);
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !explicit => {
            // Saving later creates the default file.
            return (Config { path: Some(path), ..Config::default() }, None);
        }
        Err(e) => return (Config::default(), Some(ConfigError::Read { fault: Fault::io_at(&e, &path), path })),
    };
    match parse(&text) {
        Ok(mut cfg) => {
            cfg.path = Some(path);
            cfg.exists = true;
            (cfg, None)
        }
        Err(e) => (Config::default(), Some(e)),
    }
}

pub fn parse(text: &str) -> Result<Config, ConfigError> {
    let f: FileConfig = toml::from_str(text).map_err(|e| ConfigError::Syntax(Fault::toml_de(text, &e)))?;
    let version = f.version.unwrap_or(1);
    if version == 0 || version > CONFIG_VERSION {
        return Err(ConfigError::Version(version));
    }
    let language = f.language.unwrap_or_else(|| DEFAULT_LANGUAGE.into());
    if !matches!(language.as_str(), "auto" | "en" | "ko") {
        return Err(bad("language", &language).allowed(Some("auto, en, ko")));
    }
    let icons = match f.icons {
        None => IconsSetting::default(),
        Some(v) => IconsSetting::parse(&v).ok_or_else(|| bad("icons", &v).allowed(Some("auto, on, off")))?,
    };
    let theme = f.theme.map_or_else(|| crate::theme::DEFAULT.to_string(), |t| t.trim().to_string());
    let page_size = f.page_size.unwrap_or(500);
    if page_size == 0 || page_size > i32::MAX as usize {
        return Err(bad("page_size", page_size));
    }
    let result_window_rows = match f.result_window_rows {
        None => crate::results::WINDOW,
        Some(n) if (crate::results::MIN_WINDOW as i64..=10_000_000).contains(&n) => n as usize,
        Some(n) => return Err(bad("result_window_rows", n).allowed(Some("1000 to 10000000"))),
    };
    const SIZES: &str = "1073741824, \"512MB\", \"2GB\", \"off\"";
    let spill_limit = match &f.spill_limit {
        None => policy::SpillLimit::default(),
        Some(v) => policy::parse_size(v).map_err(|e| bad("spill_limit", e).allowed(Some(SIZES)))?,
    };
    let (editor_mode, cursor_shape) = match f.editor {
        None => (None, None),
        Some(e) => (e.mode, e.cursor_shape),
    };
    let editor_mode = match editor_mode {
        None => EditorMode::default(),
        Some(m) => EditorMode::parse(&m).ok_or_else(|| bad("editor.mode", &m).allowed(Some("vim, standard")))?,
    };
    let default_source = match f.secrets.and_then(|s| s.default_source) {
        None => DefaultSource::default(),
        Some(v) => DefaultSource::parse(&v).ok_or_else(|| {
            bad("secrets.default_source", &v).allowed(Some("auto, keychain, file, command, env, prompt"))
        })?,
    };
    let prefs = Prefs {
        commands_position: choice("commands.position", f.commands.and_then(|c| c.position), "popup, bottom")?,
        detail_view: choice("detail_view", f.detail_view, "panel, statusbar")?,
        clipboard: choice("clipboard", f.clipboard, "auto, system, osc52")?,
        copy_header: choice("copy_header", f.copy_header, "auto, on, off")?,
        osc52_max_bytes: match f.osc52_max_bytes {
            None => OSC52_MAX_BYTES,
            Some(n) if n > 0 => usize::try_from(n).map_err(|_| bad("osc52_max_bytes", n))?,
            Some(n) => return Err(bad("osc52_max_bytes", n)),
        },
        cursor_shape: choice("editor.cursor_shape", cursor_shape, "on, off")?,
    };
    let mut policies = Policies::default();
    for (name, p) in &f.policy {
        let mut pol = Policy::default();
        if let Some(v) = &p.paging_idle_timeout {
            pol.paging_idle_timeout = policy::parse_timeout(v).map_err(|e| {
                bad(&format!("policy.{name}.paging_idle_timeout"), e)
                    .allowed(Some("30, \"30s\", \"5m\", \"1h\", \"off\""))
            })?;
        }
        if let Some(v) = &p.spill_limit {
            pol.spill_limit = Some(
                policy::parse_size(v)
                    .map_err(|e| bad(&format!("policy.{name}.spill_limit"), e).allowed(Some(SIZES)))?,
            );
        }
        if let Some(v) = &p.read_only {
            pol.read_only =
                v.as_bool().ok_or_else(|| bad(&format!("policy.{name}.read_only"), v).allowed(Some("true, false")))?;
        }
        if let Some(v) = &p.confirm {
            pol.confirm = policy::Confirm::parse(v)
                .ok_or_else(|| bad(&format!("policy.{name}.confirm"), v).allowed(Some("destructive, writes")))?;
        }
        policies.insert(name, pol);
    }
    let mut folders = Folders::default();
    for s in &f.folders {
        folders.insert(&FolderPath::parse(s).map_err(|_| bad("folders", s))?);
    }
    let mut connections = f.connections;
    let mut seen = HashSet::new();
    let mut ids_assigned = false;
    for c in &mut connections {
        if c.id.is_unset() {
            c.id = ProfileId::new();
            ids_assigned = true;
        } else if !seen.insert(c.id) {
            return Err(ConfigError::DuplicateId(c.id.to_string()));
        }
        if let Some(folder) = &c.folder {
            let p = FolderPath::parse(folder).map_err(|_| bad("folder", folder).in_profile(&c.name))?;
            folders.insert(&p);
        }
        if let Some(color) = &c.color
            && ProfileColor::parse(color).is_none()
        {
            return Err(bad("color", color).in_profile(&c.name).allowed(Some("red, green, blue, … #rrggbb")));
        }
        match c.password_source {
            Some(SourceKind::Command) if c.password_command.as_deref().is_none_or(|s| s.trim().is_empty()) => {
                return Err(ConfigError::Missing {
                    key: "password_command",
                    profile: c.name.clone(),
                    source: "command",
                });
            }
            Some(SourceKind::Env) => match c.password_env.as_deref() {
                Some(n) if valid_env_name(n) => {}
                Some(n) => return Err(bad("password_env", n).in_profile(&c.name)),
                None => {
                    return Err(ConfigError::Missing { key: "password_env", profile: c.name.clone(), source: "env" });
                }
            },
            _ => {}
        }
        if let Some(ssh) = &c.ssh {
            match ssh.problem() {
                Some(SshProblem::Missing(key)) => {
                    return Err(ConfigError::SshMissing { key, profile: c.name.clone() });
                }
                Some(SshProblem::Invalid(key)) => {
                    let value = match key {
                        "port" => ssh.port.to_string(),
                        _ => ssh.secret_env.clone().unwrap_or_default(),
                    };
                    return Err(bad(&format!("ssh.{key}"), value).in_profile(&c.name));
                }
                None => {}
            }
        }
        c.origin = Some(c.name.clone());
        c.normalize();
    }
    // A v1 file stores the profile's name; a v2 file its id.
    let last_used = f.last_used.and_then(|s| {
        ProfileId::parse(&s)
            .filter(|id| connections.iter().any(|c| c.id == *id))
            .or_else(|| connections.iter().find(|c| c.name == s).map(|c| c.id))
    });
    Ok(Config {
        version,
        language,
        icons,
        theme,
        page_size,
        result_window_rows,
        spill_limit,
        connections,
        folders,
        last_used,
        editor_mode,
        default_source,
        prefs,
        keymap: f.keymap,
        policies,
        path: None,
        exists: false,
        ids_assigned,
    })
}

/// Replace a value but keep its decoration (e.g. a trailing `# comment`) and key position.
fn set(t: &mut Table, key: &str, v: impl Into<Value>) {
    let mut v: Value = v.into();
    if let Some(old) = t.get_mut(key).and_then(Item::as_value_mut) {
        *v.decor_mut() = old.decor().clone();
        *old = v;
        return;
    }
    t.insert(key, Item::Value(v));
}

/// Set `key` to `v`, or remove it for `None`.
fn set_opt(t: &mut Table, key: &str, v: Option<&str>) {
    match v {
        Some(v) => set(t, key, v),
        None => {
            t.remove(key);
        }
    }
}

/// Write setting `key` of table `t` (`default`: `v` is its default value). Not the default: set
/// it (keeping its comments and place). The default: remove the key when the file has another
/// value (the setting went back to its default), leave it when the file already says `v`, never
/// add it. Returns the comments of a removed key that had no following key to go to.
fn setting(t: &mut Table, key: &str, v: Value, default: bool) -> Result<String, Fault> {
    match t.get(key) {
        Some(i) if !i.is_value() => {
            Err(Fault::new(FaultKind::Shape { key: key.into() }, format!("{key} must be a single value")))
        }
        Some(_) if default && !t.get(key).and_then(Item::as_value).is_some_and(|old| same(old, &v)) => {
            Ok(remove_keeping_comments(t, key))
        }
        _ if default => Ok(String::new()),
        _ => {
            set(t, key, v);
            Ok(String::new())
        }
    }
}

/// Whether two values are the same string or integer (their formatting aside).
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(x), Value::String(y)) => x.value() == y.value(),
        (Value::Integer(x), Value::Integer(y)) => x.value() == y.value(),
        _ => false,
    }
}

/// [`setting`] for `key` of the table `table` (`[editor] mode`, also an inline table). A table
/// the setting emptied goes too, unless comments hang on it.
fn nested_setting(doc: &mut DocumentMut, table: &str, key: &str, (v, default): (Value, bool)) -> Result<String, Fault> {
    let shape = || Fault::new(FaultKind::Shape { key: table.into() }, format!("{table} must be a table"));
    if default && doc.get(table).is_none() {
        return Ok(String::new());
    }
    let orphans = match doc.entry(table).or_insert_with(|| Item::Table(Table::new())) {
        Item::Table(t) => {
            let orphans = setting(t, key, v, default)?;
            let comments = |d: &toml_edit::Decor| {
                [d.prefix(), d.suffix()].into_iter().flatten().any(|r| r.as_str().is_some_and(|s| s.contains('#')))
            };
            if t.is_empty() && !comments(t.decor()) {
                doc.remove(table);
            }
            orphans
        }
        Item::Value(Value::InlineTable(t)) => {
            let old = t.get(key).and_then(|o| o.as_str()).map(str::to_string);
            if !default {
                t.insert(key, v);
            } else if old.is_some_and(|o| Some(o.as_str()) != v.as_str()) {
                t.remove(key);
            }
            String::new()
        }
        _ => return Err(shape()),
    };
    Ok(orphans)
}

/// Remove `key` from `t`, moving the comments around it (the lines before it, the comment after
/// its value) to the key that follows it. Without one, they are returned for the caller to
/// keep elsewhere.
fn remove_keeping_comments(t: &mut Table, key: &str) -> String {
    let raw = |r: Option<&toml_edit::RawString>| r.and_then(|r| r.as_str()).unwrap_or_default().to_string();
    let prefix = raw(t.key(key).and_then(|k| k.leaf_decor().prefix()));
    let suffix = raw(t.get(key).and_then(Item::as_value).and_then(|v| v.decor().suffix()));
    let next = t
        .iter()
        .map(|(k, _)| k.to_string())
        .skip_while(|k| k != key)
        .skip(1)
        .find(|k| t.get(k).is_some_and(Item::is_value));
    t.remove(key);
    let mut moved = if prefix.contains('#') { prefix } else { String::new() };
    if let Some(c) = suffix.find('#') {
        moved.push_str(&suffix[c..]);
        moved.push('\n');
    }
    if moved.is_empty() {
        return moved;
    }
    match next.and_then(|n| t.key_mut(&n)) {
        Some(mut k) => {
            let old = raw(k.leaf_decor().prefix());
            let sep = if moved.starts_with('\n') || old.is_empty() { "" } else { "\n" };
            k.leaf_decor_mut().set_prefix(format!("{moved}{sep}{}", old.trim_start_matches('\n')));
            String::new()
        }
        None => moved,
    }
}

/// `t` with `id` as its first key (the rest keep their order and comments).
fn with_id_first(t: &Table, id: &str) -> Table {
    let mut out = Table::new();
    out.insert("id", Item::Value(id.into()));
    for (k, item) in t.iter() {
        match t.key(k) {
            Some(key) => out.insert_formatted(key, item.clone()),
            None => out.insert(k, item.clone()),
        };
    }
    *out.decor_mut() = t.decor().clone();
    out.set_position(t.position());
    out
}

/// App-wide settings written to the file.
#[derive(Clone, Copy, Debug)]
pub struct Settings<'a> {
    /// Format version to record. `version` is written once it is 2 or more (or already in the
    /// file); a file still being migrated keeps its old version.
    pub version: u32,
    pub language: &'a str,
    pub icons: IconsSetting,
    /// `theme`, as the user chose it (also a name that did not resolve: it is kept).
    pub theme: &'a str,
    pub editor_mode: EditorMode,
    pub default_source: DefaultSource,
    pub prefs: Prefs,
}

/// The profile part of the file: `[[connections]]`, `folders` and `last_used`.
#[derive(Clone, Copy)]
pub struct Profiles<'a> {
    pub connections: &'a [ConnectionConfig],
    pub folders: &'a Folders,
    /// `None` removes the key.
    pub last_used: Option<ProfileId>,
}

/// Write the settings and (when `profiles` is `Some`) the connection profiles, folders and
/// `last_used` into the file at `path`, preserving everything else in it (comments, key order,
/// `[keymap.*]`). Creates the file and its directory if needed. Symlinked config files
/// (dotfiles) are written through to their target. The write is atomic
/// ([`crate::fsutil::atomic_write`]).
///
/// A profile's table is found by its `id`, else by its name when it was read
/// ([`ConnectionConfig::origin`]; a table without `id` right after the migration), so its
/// comments survive; the `id` is then added as its first key.
pub fn save(path: &Path, settings: Settings, profiles: Option<Profiles>) -> Result<(), Fault> {
    let Settings { version, language, icons, theme, editor_mode, default_source, prefs } = settings;
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = match std::fs::read_to_string(&target) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(Fault::io_at(&e, &target)),
    };
    let mut doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| Fault::toml_edit(&text, &e))?;
    if version >= CONFIG_VERSION || doc.contains_key("version") {
        set(doc.as_table_mut(), "version", i64::from(version));
    }
    // Each setting: written when it is not its default; back at its default, the key goes (its
    // comments stay); a file that already says the default is left as it is.
    let root = doc.as_table_mut();
    let mut orphans = String::new();
    orphans += &setting(root, "language", language.into(), language == DEFAULT_LANGUAGE)?;
    orphans += &setting(root, "icons", icons.as_str().into(), icons == IconsSetting::Auto)?;
    orphans += &setting(root, "theme", theme.into(), theme == crate::theme::DEFAULT)?;
    orphans +=
        &setting(root, "detail_view", prefs.detail_view.as_str().into(), prefs.detail_view == DetailView::default())?;
    orphans +=
        &setting(root, "clipboard", prefs.clipboard.as_str().into(), prefs.clipboard == ClipboardSetting::default())?;
    orphans +=
        &setting(root, "copy_header", prefs.copy_header.as_str().into(), prefs.copy_header == CopyHeader::default())?;
    let osc = i64::try_from(prefs.osc52_max_bytes).unwrap_or(i64::MAX);
    orphans += &setting(root, "osc52_max_bytes", osc.into(), prefs.osc52_max_bytes == OSC52_MAX_BYTES)?;
    let mode = (editor_mode.as_str().into(), editor_mode == EditorMode::Vim);
    orphans += &nested_setting(&mut doc, "editor", "mode", mode)?;
    let shape = (prefs.cursor_shape.as_str().into(), prefs.cursor_shape == CursorShape::default());
    orphans += &nested_setting(&mut doc, "editor", "cursor_shape", shape)?;
    let source = (default_source.as_str().into(), default_source == DefaultSource::Auto);
    orphans += &nested_setting(&mut doc, "secrets", "default_source", source)?;
    let position = (prefs.commands_position.as_str().into(), prefs.commands_position == CommandsPosition::default());
    orphans += &nested_setting(&mut doc, "commands", "position", position)?;
    if !orphans.is_empty() {
        let trailing = doc.trailing().as_str().unwrap_or_default().to_string();
        doc.set_trailing(format!("{trailing}{orphans}"));
    }
    if let Some(Profiles { connections: conns, folders, last_used }) = profiles {
        match last_used {
            Some(id) => set(doc.as_table_mut(), "last_used", id.to_string()),
            None => {
                doc.remove("last_used");
            }
        }
        if !folders.is_empty() || doc.contains_key("folders") {
            let arr: Array = folders.iter().map(FolderPath::as_str).collect();
            set(doc.as_table_mut(), "folders", arr);
        }
        let old: Vec<Table> = doc
            .get("connections")
            .and_then(Item::as_array_of_tables)
            .map(|a| a.iter().cloned().collect())
            .unwrap_or_default();
        // Tables keep their document positions; hand them out in the new order so a reordered
        // list is written in that order, still at the same place in the file.
        let mut positions: Vec<isize> = old.iter().filter_map(Table::position).collect();
        positions.sort_unstable();
        let mut arr = ArrayOfTables::new();
        for (i, c) in conns.iter().enumerate() {
            let id = c.id.to_string();
            let found = old.iter().find(|t| t.get("id").and_then(Item::as_str) == Some(id.as_str())).or_else(|| {
                c.origin.as_deref().and_then(|o| {
                    old.iter().find(|t| !t.contains_key("id") && t.get("name").and_then(Item::as_str) == Some(o))
                })
            });
            let mut t = match found {
                Some(t) if t.contains_key("id") => t.clone(),
                Some(t) => with_id_first(t, &id),
                None => with_id_first(&Table::new(), &id),
            };
            set(&mut t, "id", id.as_str());
            set(&mut t, "name", c.name.as_str());
            set(&mut t, "driver", c.driver.as_str());
            set_opt(&mut t, "folder", c.folder.as_deref());
            set_opt(&mut t, "color", c.color.as_deref());
            set_opt(&mut t, "icon", c.icon.as_deref());
            set_opt(&mut t, "policy", c.policy.as_deref());
            set_opt(&mut t, "password_source", c.password_source.map(SourceKind::as_str));
            set_opt(&mut t, "password_command", c.password_command.as_deref());
            set_opt(&mut t, "password_env", c.password_env.as_deref());
            if c.statement_cache {
                t.remove("statement_cache");
            } else {
                set(&mut t, "statement_cache", false);
            }
            match &c.ssh {
                Some(ssh) => write_ssh(&mut t, ssh),
                None => {
                    t.remove("ssh");
                }
            }
            if let Some(d) = &c.dsn {
                set(&mut t, "dsn", d.as_str());
            } else {
                t.remove("dsn");
                set(&mut t, "host", c.host.as_str());
                set(&mut t, "port", i64::from(c.port));
                set(&mut t, "user", c.user.as_str());
                set(&mut t, "database", c.database.as_str());
                set(&mut t, "sslmode", c.sslmode.as_str());
            }
            if c.password.is_empty() {
                t.remove("password");
            } else {
                set(&mut t, "password", c.password.as_str());
            }
            t.set_position(positions.get(i).copied());
            arr.push(t);
        }
        doc.insert("connections", Item::ArrayOfTables(arr));
    }
    crate::fsutil::atomic_write(&target, doc.to_string().as_bytes()).map_err(|e| Fault::io_at(&e, &target))
}

/// The profile's `[connections.ssh]` table: the keys it has are updated in place (comments
/// stay), the defaults are left out.
fn write_ssh(profile: &mut Table, ssh: &SshSettings) {
    let mut t = match profile.remove("ssh") {
        Some(Item::Table(t)) => t,
        _ => Table::new(),
    };
    set(&mut t, "enabled", ssh.enabled);
    set(&mut t, "host", ssh.host.as_str());
    if ssh.port == 22 {
        t.remove("port");
    } else {
        set(&mut t, "port", i64::from(ssh.port));
    }
    set(&mut t, "user", ssh.user.as_str());
    set(&mut t, "auth", ssh.auth.as_str());
    set_opt(&mut t, "key_file", ssh.key_file.as_deref());
    set_opt(&mut t, "secret_source", ssh.secret_source.map(SourceKind::as_str));
    set_opt(&mut t, "secret_command", ssh.secret_command.as_deref());
    set_opt(&mut t, "secret_env", ssh.secret_env.as_deref());
    match ssh.keepalive {
        Some(k) => set(&mut t, "keepalive", i64::try_from(k).unwrap_or(i64::MAX)),
        None => {
            t.remove("keepalive");
        }
    }
    match ssh.timeout {
        Some(k) => set(&mut t, "timeout", i64::try_from(k).unwrap_or(i64::MAX)),
        None => {
            t.remove("timeout");
        }
    }
    profile.insert("ssh", Item::Table(t));
}

#[cfg(test)]
mod tests;
