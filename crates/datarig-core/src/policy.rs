//! Safety policies: named sets of behavior settings; a profile names one
//! (`policy = "…"`), and a profile without one uses `default`.
//!
//! In the config file a policy is a `[policy.<name>]` table with these items:
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
//!
//! An item a table leaves out has its built-in default; `[policy.default]` changes the values of
//! the built-in `default` policy. A profile naming a policy that is not defined gets `default`.

pub use crate::driver::PagingMode;
use std::collections::BTreeMap;
use std::time::Duration;

/// Default of `paging_idle_timeout`.
pub const PAGING_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// The name of the policy a profile without `policy` uses.
pub const DEFAULT: &str = "default";

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
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            paging: PagingMode::NoHold,
            paging_idle_timeout: Some(PAGING_IDLE_TIMEOUT),
            spill_limit: None,
            read_only: false,
            confirm: Confirm::Destructive,
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

/// The policies of the config file by name; `default` is always there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Policies(BTreeMap<String, Policy>);

impl Policies {
    pub fn insert(&mut self, name: &str, p: Policy) {
        self.0.insert(name.to_string(), p);
    }

    /// The policy a profile with `policy = name` uses.
    pub fn get(&self, name: Option<&str>) -> Policy {
        let name = name.unwrap_or(DEFAULT);
        self.0.get(name).or_else(|| self.0.get(DEFAULT)).cloned().unwrap_or_default()
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

#[cfg(test)]
mod tests;
