//! Environments: named sets of safety and behavior settings (`dev`, `staging`, `prod`, …) that
//! profiles name (`policy = "…"`), each with an optional color and tab badge. A profile without
//! one uses the default rules.
//!
//! In the config file an environment is a `[policy.<name>]` table (the key predates the name
//! "environment" and stays, so existing files keep working) with these items:
//!
//! * `paging` — what a result with more rows than a page keeps on the server, outside the
//!   user's own transaction block: `"no_hold"` (default) keeps nothing: the first page comes
//!   back and its server-side portal and the transaction it needs end in the same request, so
//!   no lock, snapshot or transaction stays while the user reads; the next page runs the
//!   statement again (only a statement on the plain-`SELECT` allowlist, announced), and any
//!   other statement shows its first page only. `"hold"` keeps the portal open, and with it the
//!   transaction and the locks its statement took (an `ACCESS SHARE` lock on every table read,
//!   which an `ALTER TABLE` of another session waits for, and every statement queued behind
//!   that one with it), until the result is read to its end or closed after
//!   `paging_idle_timeout`; the next page is fetched from it. Inside the user's `BEGIN` block a
//!   result's portal always lives in the block, whatever this says.
//! * `paging_idle_timeout` — with `paging = "hold"`, how long a result that has more rows may
//!   keep its server-side portal open without a fetch. A duration: seconds as a number, or a
//!   string with a unit (`"90s"`, `"5m"`, `"1h"`); `0` or `"off"` never closes it. Default
//!   30s. Suggested: shorter (e.g. `"10s"`) where an open transaction holds locks and a
//!   snapshot and blocks vacuum, and `"off"` for a local database.
//! * `spill_limit` — the most a result's rows may take on disk: past the
//!   config's `result_window_rows`, rows go to a temporary file, and fetching stops when it
//!   reaches this size. Bytes as a number, or a string with a unit (`"512MB"`, `"2GB"`; KB, MB
//!   and GB are powers of 1024); `0` or `"off"` for no limit. Without it, the config's
//!   top-level `spill_limit` applies (default 1 GB).
//!
//! * `read_only` — `true` makes the profile read-only: the app runs only
//!   reads, transaction control and safe session settings (`sql::risk::Risk::read_only`) and
//!   refuses the rest before sending anything, and the driver makes every transaction of the
//!   profile's query sessions `READ ONLY` on the server (with the startup option
//!   `default_transaction_read_only = on` as a second layer), so the server also rejects writes
//!   the text does not show (a function called from a `SELECT`). Default `false`.
//! * `confirm` — which statements ask before they run: `"destructive"` (default:
//!   `DROP`, `TRUNCATE`, `UPDATE`/`DELETE` of every row, `ALTER … DROP COLUMN`, `ALTER … TYPE …
//!   USING`, a `MERGE` that updates or deletes, `DO`/`CALL`, an unknown prepared statement,
//!   text the parser rejects: `sql::risk::Danger`) or `"writes"` (also every statement that is
//!   not a read, transaction control or a session setting; unknown and procedural ones
//!   included). Dangerous statements always ask.
//! * `commit` — when what runs is committed: `"auto"` (default: each statement outside the
//!   user's own `BEGIN` block commits by itself) or `"manual"`: the first statement that may
//!   write opens a transaction that stays open until the user commits or rolls it back; plain
//!   reads before it run without one. Needs a driver that can (`Capabilities::manual_commit`);
//!   elsewhere it runs as `"auto"`.
//! * `tx_idle_warning` — how long an open transaction may sit idle before the app warns that
//!   it holds locks: a duration as `paging_idle_timeout` takes (at least
//!   [`TX_IDLE_FLOOR`]), or `"off"`. Default 5m.
//! * `tx_idle_rollback` — how long a transaction opened for manual commit may sit idle before
//!   the app rolls it back (its session stays; one that cannot roll back in time is closed, and
//!   the server rolls back). A duration (at least [`TX_IDLE_FLOOR`]) or `"off"`. Default 30m.
//!   PostgreSQL's server also gets the limit, a little later, for that transaction only
//!   (`idle_in_transaction_session_timeout`), so it ends there when the app is gone; MySQL has no
//!   such server limit. Its old name `tx_idle_timeout` is still read.
//! * `idle_disconnect` — how long a profile's connection may sit unused before the app closes
//!   its sessions (the next action connects again): a duration as `paging_idle_timeout` takes,
//!   from [`IDLE_DISCONNECT_FLOOR`] to [`SESSION_LIMIT_MAX`], or `"off"`. Without it, the config's
//!   top-level `idle_disconnect` applies (default off).
//! * `lock_wait` — how long a statement of a query session waits for a lock another session
//!   holds before the server cancels it: a duration from [`LOCK_WAIT_FLOOR`] to
//!   [`SESSION_LIMIT_MAX`], or `"off"` (the server's setting). Without it, the config's top-level
//!   `lock_wait` applies (default off).
//! * `session` — the session preset its profiles' query sessions start with, for each database:
//!   `{ postgres = "<name>", mysql = "<name>" }` (either may be left out; see
//!   [`crate::profile::session`]). A profile may name its own (`session = "<name>"`) or none
//!   (`session = "none"`). No default (none).
//! * `color` — the environment's color: one of the profile colors' 12 names or `#rrggbb`
//!   ([`crate::profile::color`]). No default (no color).
//! * `badge` — a short text the tabs and the status bar show for its profiles (`"PROD"`): no
//!   blanks around it, no control character, at most [`BADGE_MAX`] columns. No default.
//!
//! An item a table leaves out has its built-in default. `[policy.default]` is not an
//! environment: it changes the default rules (the app edits them too), which profiles without
//! an environment use (its `color` and `badge`, if any, are not shown anywhere). A profile naming an environment that
//! is not defined is an error of that profile ([`Policies::get`]): it is never run under the
//! default rules instead.
//!
//! [`presets`] are the four environments the app offers while there is none.

pub use crate::driver::PagingMode;
use crate::profile::color::ProfileColor;
pub use crate::profile::session::SessionMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Default of `paging_idle_timeout`.
pub const PAGING_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Default of `tx_idle_warning`.
pub const TX_IDLE_WARNING: Duration = Duration::from_secs(5 * 60);

/// Default of `tx_idle_rollback`.
pub const TX_IDLE_ROLLBACK: Duration = Duration::from_secs(30 * 60);

/// The shortest `tx_idle_warning` and `tx_idle_rollback` (a shorter one would warn about, or
/// end, a transaction between two statements the user types).
pub const TX_IDLE_FLOOR: Duration = Duration::from_secs(10);

/// The shortest `idle_disconnect` (a shorter one would close a connection between two
/// statements the user types).
pub const IDLE_DISCONNECT_FLOOR: Duration = Duration::from_secs(60);

/// The shortest `lock_wait` (the servers count it in whole seconds or milliseconds; zero would
/// mean no limit).
pub const LOCK_WAIT_FLOOR: Duration = Duration::from_secs(1);

/// The longest `idle_disconnect` and `lock_wait` (what every server's setting takes).
pub const SESSION_LIMIT_MAX: Duration = Duration::from_secs(24 * 3600);

/// The name of the table of the default rules (`[policy.default]`), which a profile without an
/// environment uses; never an environment's name.
pub const DEFAULT: &str = "default";

/// The most columns a badge may take.
pub const BADGE_MAX: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    /// Whether a result with more rows keeps its portal (and transaction) open.
    pub paging: PagingMode,
    /// `None`: an idle result's portal stays open (`PagingMode::Hold`).
    pub paging_idle_timeout: Option<Duration>,
    /// `None`: the config's `spill_limit`.
    pub spill_limit: Option<SpillLimit>,
    /// Only reads run, and the server makes every transaction read-only.
    pub read_only: bool,
    pub confirm: Confirm,
    pub commit: CommitRule,
    /// `None`: no warning.
    pub tx_idle_warning: Option<Duration>,
    /// `None`: never rolled back for that (on PostgreSQL, the server's own setting).
    pub tx_idle_rollback: Option<Duration>,
    /// `None`: the config's `idle_disconnect`; `Some(None)`: never closed for being idle.
    pub idle_disconnect: Option<Option<Duration>>,
    /// `None`: the config's `lock_wait`; `Some(None)`: the server's own setting.
    pub lock_wait: Option<Option<Duration>>,
    /// The session preset of each database its profiles start with (a profile may name its
    /// own, or none).
    pub session: SessionMap,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            paging: PagingMode::NoHold,
            paging_idle_timeout: Some(PAGING_IDLE_TIMEOUT),
            spill_limit: None,
            read_only: false,
            confirm: Confirm::Destructive,
            commit: CommitRule::Auto,
            tx_idle_warning: Some(TX_IDLE_WARNING),
            tx_idle_rollback: Some(TX_IDLE_ROLLBACK),
            idle_disconnect: None,
            lock_wait: None,
            session: SessionMap::default(),
        }
    }
}

/// `commit`: when what runs is committed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CommitRule {
    /// Each statement outside the user's own block commits by itself.
    #[default]
    Auto,
    /// The first statement that may write opens a transaction the user ends.
    Manual,
}

impl CommitRule {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(CommitRule::Auto),
            "manual" => Some(CommitRule::Manual),
            _ => None,
        }
    }

    pub fn manual(self) -> bool {
        self == CommitRule::Manual
    }

    /// The value as written: `"auto"` or `"manual"`.
    pub fn as_str(self) -> &'static str {
        match self {
            CommitRule::Auto => "auto",
            CommitRule::Manual => "manual",
        }
    }
}

/// `confirm`: which statements ask before they run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Confirm {
    /// Destructive statements only.
    #[default]
    Destructive,
    /// Destructive statements and every statement that may change something.
    Writes,
}

impl Confirm {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "destructive" => Some(Confirm::Destructive),
            "writes" => Some(Confirm::Writes),
            _ => None,
        }
    }

    /// Every statement that may change something asks.
    pub fn writes(self) -> bool {
        self == Confirm::Writes
    }

    /// The value as written: `"destructive"` or `"writes"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Confirm::Destructive => "destructive",
            Confirm::Writes => "writes",
        }
    }
}

/// A `spill_limit`: the most bytes a result's spill file may take, or no limit (`None`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpillLimit(pub Option<u64>);

impl Default for SpillLimit {
    fn default() -> Self {
        Self(Some(crate::results::CAP))
    }
}

/// A `spill_limit` value: bytes as a number, `"<n>KB"`, `"<n>MB"`, `"<n>GB"` (powers of 1024),
/// or `"off"`; zero and `"off"` mean no limit.
pub fn parse_size(v: &toml::Value) -> Result<SpillLimit, String> {
    let bytes = match v {
        toml::Value::Integer(n) if *n >= 0 => *n as u64,
        toml::Value::String(s) => {
            let s = s.trim();
            if s.eq_ignore_ascii_case("off") {
                return Ok(SpillLimit(None));
            }
            let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
            let n: u64 = num.parse().map_err(|_| format!("{s:?}"))?;
            let mult: u64 = match unit.trim().to_ascii_uppercase().as_str() {
                "" | "B" => 1,
                "KB" => 1 << 10,
                "MB" => 1 << 20,
                "GB" => 1 << 30,
                _ => return Err(format!("{s:?}")),
            };
            n.checked_mul(mult).ok_or_else(|| format!("{s:?}"))?
        }
        other => return Err(other.to_string()),
    };
    Ok(SpillLimit((bytes > 0).then_some(bytes)))
}

/// An environment's identity while the app runs (never written: the file knows it by its
/// name). It stays the same across a rename.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EnvironmentId(u64);

impl EnvironmentId {
    /// A new id, unique in this process.
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        EnvironmentId(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for EnvironmentId {
    fn default() -> Self {
        Self::new()
    }
}

/// An environment: a `[policy.<name>]` table other than `default`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Environment {
    pub id: EnvironmentId,
    /// The table's key, and what profiles name (`policy = "<name>"`).
    pub name: String,
    pub policy: Policy,
    /// `red` … `gray` or `#rrggbb` (validated when read); `None`: no color.
    pub color: Option<String>,
    /// The tab badge ([`badge_ok`]); `None`: none.
    pub badge: Option<String>,
    /// The name its table has in the file (a rename moves that table, comments and all).
    pub origin: Option<String>,
}

impl Environment {
    /// A new environment `name` with `policy`, no color nor badge, and a new id.
    pub fn new(name: &str, policy: Policy) -> Self {
        Environment { id: EnvironmentId::new(), name: name.to_string(), policy, color: None, badge: None, origin: None }
    }

    /// Its color, when it has one.
    pub fn display_color(&self) -> Option<ProfileColor> {
        self.color.as_deref().and_then(ProfileColor::parse)
    }

    /// The same settings: the rules, the color and the badge (not the name nor the id).
    pub fn same_settings(&self, other: &Environment) -> bool {
        self.policy == other.policy && self.color == other.color && self.badge == other.badge
    }
}

/// Whether `s` can be a badge: not empty, no blanks around it, no control character, at most
/// [`BADGE_MAX`] columns.
pub fn badge_ok(s: &str) -> bool {
    !s.is_empty()
        && s.trim() == s
        && !s.chars().any(char::is_control)
        && unicode_width::UnicodeWidthStr::width(s) <= BADGE_MAX
}

/// The environments the app offers while there is none, in order: `dev` and `test` with the
/// default rules (green, blue), `staging` asking before every write and committing by hand
/// (orange, `STG`) and `prod` read-only, asking before every write and committing by hand
/// (red, `PROD`).
pub fn presets() -> Vec<Environment> {
    let env = |name: &str, policy: Policy, color: &str, badge: Option<&str>| Environment {
        color: Some(color.to_string()),
        badge: badge.map(str::to_string),
        ..Environment::new(name, policy)
    };
    let writes = Policy { confirm: Confirm::Writes, commit: CommitRule::Manual, ..Policy::default() };
    vec![
        env("dev", Policy::default(), "green", None),
        env("test", Policy::default(), "blue", None),
        env("staging", writes.clone(), "orange", Some("STG")),
        env("prod", Policy { read_only: true, ..writes }, "red", Some("PROD")),
    ]
}

/// A profile names an environment that is not defined (its name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownEnvironment(pub String);

/// The default rules and the environments of the config file, in the file's order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policies {
    /// `[policy.default]` (else the built-in defaults).
    default: Policy,
    environments: Vec<Environment>,
}

impl Policies {
    /// `default` changes the default rules ([`DEFAULT`]); another name adds environment `name`
    /// with `p` or changes its rules.
    pub fn insert(&mut self, name: &str, p: Policy) {
        if name == DEFAULT {
            self.default = p;
        } else if let Some(e) = self.environments.iter_mut().find(|e| e.name == name) {
            e.policy = p;
        } else {
            let mut e = Environment::new(name, p);
            e.origin = Some(name.to_string());
            self.environments.push(e);
        }
    }

    /// The default rules.
    pub fn default_rules(&self) -> &Policy {
        &self.default
    }

    pub fn environments(&self) -> &[Environment] {
        &self.environments
    }

    pub fn environments_mut(&mut self) -> &mut Vec<Environment> {
        &mut self.environments
    }

    /// Environment `name` (exactly that name).
    pub fn environment(&self, name: &str) -> Option<&Environment> {
        self.environments.iter().find(|e| e.name == name)
    }

    /// The rules of a profile with `policy = name`: the default rules without a name (or with
    /// `default`), else that environment's. A name no environment has is an error, never the
    /// default rules.
    pub fn get(&self, name: Option<&str>) -> Result<Policy, UnknownEnvironment> {
        match name {
            None | Some(DEFAULT) => Ok(self.default.clone()),
            Some(n) => self.environment(n).map(|e| e.policy.clone()).ok_or_else(|| UnknownEnvironment(n.to_string())),
        }
    }
}

/// A `paging` value: `"no_hold"` or `"hold"` (case and surrounding spaces do not matter).
pub fn parse_paging(s: &str) -> Option<PagingMode> {
    match s.trim().to_ascii_lowercase().as_str() {
        "no_hold" => Some(PagingMode::NoHold),
        "hold" => Some(PagingMode::Hold),
        _ => None,
    }
}

/// A `paging_idle_timeout` value: a number of seconds, or `"<n>s"`, `"<n>m"`, `"<n>h"`, `"off"`.
/// Zero and `"off"` mean never (`None`).
pub fn parse_timeout(v: &toml::Value) -> Result<Option<Duration>, String> {
    let secs = match v {
        toml::Value::Integer(n) if *n >= 0 => *n as u64,
        toml::Value::String(s) => {
            let s = s.trim();
            if s.eq_ignore_ascii_case("off") {
                return Ok(None);
            }
            let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
            let n: u64 = num.parse().map_err(|_| format!("{s:?}"))?;
            let mult = match unit.trim() {
                "" | "s" => 1,
                "m" => 60,
                "h" => 3600,
                _ => return Err(format!("{s:?}")),
            };
            n.checked_mul(mult).ok_or_else(|| format!("{s:?}"))?
        }
        other => return Err(other.to_string()),
    };
    Ok((secs > 0).then(|| Duration::from_secs(secs)))
}

/// A `tx_idle_warning` or `tx_idle_rollback` value: as [`parse_timeout`] reads it (zero and
/// `"off"` are `None`), and at least [`TX_IDLE_FLOOR`].
pub fn parse_tx_idle(v: &toml::Value) -> Result<Option<Duration>, String> {
    match parse_timeout(v)? {
        Some(d) if d < TX_IDLE_FLOOR => {
            Err(format!("{} (less than {}s)", format_timeout(Some(d)), TX_IDLE_FLOOR.as_secs()))
        }
        d => Ok(d),
    }
}

/// A duration as [`parse_timeout`] reads it (zero and `"off"` are `None`), from `floor` to
/// [`SESSION_LIMIT_MAX`].
fn parse_limit(v: &toml::Value, floor: Duration) -> Result<Option<Duration>, String> {
    match parse_timeout(v)? {
        Some(d) if d < floor => Err(format!("{} (less than {})", format_timeout(Some(d)), format_timeout(Some(floor)))),
        Some(d) if d > SESSION_LIMIT_MAX => {
            Err(format!("{} (more than {})", format_timeout(Some(d)), format_timeout(Some(SESSION_LIMIT_MAX))))
        }
        d => Ok(d),
    }
}

/// An `idle_disconnect` value: a duration from [`IDLE_DISCONNECT_FLOOR`] to
/// [`SESSION_LIMIT_MAX`], or off (`None`).
pub fn parse_idle_disconnect(v: &toml::Value) -> Result<Option<Duration>, String> {
    parse_limit(v, IDLE_DISCONNECT_FLOOR)
}

/// A `lock_wait` value: a duration from [`LOCK_WAIT_FLOOR`] to [`SESSION_LIMIT_MAX`], or off
/// (`None`).
pub fn parse_lock_wait(v: &toml::Value) -> Result<Option<Duration>, String> {
    parse_limit(v, LOCK_WAIT_FLOOR)
}

/// An `idle_disconnect` typed as text (`:set`, a form's field).
pub fn parse_idle_disconnect_text(s: &str) -> Result<Option<Duration>, String> {
    parse_idle_disconnect(&toml::Value::String(s.to_string()))
}

/// A `lock_wait` typed as text (`:set`, a form's field).
pub fn parse_lock_wait_text(s: &str) -> Result<Option<Duration>, String> {
    parse_lock_wait(&toml::Value::String(s.to_string()))
}

/// A `tx_idle_warning` or `tx_idle_rollback` typed as text (a form's field).
pub fn parse_tx_idle_text(s: &str) -> Result<Option<Duration>, String> {
    parse_tx_idle(&toml::Value::String(s.to_string()))
}

/// A duration typed as text (a form's field): as [`parse_timeout`] reads a string.
pub fn parse_timeout_text(s: &str) -> Result<Option<Duration>, String> {
    parse_timeout(&toml::Value::String(s.to_string()))
}

/// A size typed as text (a form's field): as [`parse_size`] reads a string.
pub fn parse_size_text(s: &str) -> Result<SpillLimit, String> {
    parse_size(&toml::Value::String(s.to_string()))
}

/// A `paging` value as written: `"no_hold"` or `"hold"`.
pub fn paging_str(p: PagingMode) -> &'static str {
    match p {
        PagingMode::NoHold => "no_hold",
        PagingMode::Hold => "hold",
    }
}

/// A `paging_idle_timeout` as written, in the largest unit that keeps it exact (`"30s"`, `"5m"`,
/// `"1h"`), or `"off"` for never; [`parse_timeout`] reads it back.
pub fn format_timeout(t: Option<Duration>) -> String {
    match t.map(|d| d.as_secs()) {
        None | Some(0) => "off".into(),
        Some(s) if s % 3600 == 0 => format!("{}h", s / 3600),
        Some(s) if s % 60 == 0 => format!("{}m", s / 60),
        Some(s) => format!("{s}s"),
    }
}

/// A `spill_limit` as written, in the largest unit that keeps it exact (`"512MB"`, `"2GB"`; bytes
/// as a plain number otherwise), or `"off"` for no limit; [`parse_size`] reads it back.
pub fn format_size(l: SpillLimit) -> String {
    match l.0 {
        None | Some(0) => "off".into(),
        Some(b) if b % (1 << 30) == 0 => format!("{}GB", b >> 30),
        Some(b) if b % (1 << 20) == 0 => format!("{}MB", b >> 20),
        Some(b) if b % (1 << 10) == 0 => format!("{}KB", b >> 10),
        Some(b) => b.to_string(),
    }
}

#[cfg(test)]
mod tests;
