//! Update check and What's new: the facts kept in `<state>/update.toml` and the decisions made
//! from them, with no network and no screen.
//!
//! Once a day at most, datarig asks GitHub for its latest release ([`release`]); the answer is
//! kept in the file, and the next start tells about a newer version from it. The file also
//! keeps the newest version that ran here, so What's new ([`notes`]) shows once after an update:
//!
//! ```toml
//! last_attempt = 2026-11-06T09:12:00Z
//! result = "ok"
//! etag = "W/\"5d1c\""
//! latest = "0.39.1"
//! latest_date = 2026-11-10
//! summary = "The result grid sums a selected range on the status line."
//! seen = "0.39.0"
//! skipped = "0.39.1"
//! reminded = { version = "0.39.1", at = 2026-11-06T09:13:00Z }
//! ```
//!
//! Nothing of the user's is in it. A file that is not there knows nothing yet. A file that is
//! there but cannot be read, or has a value this version cannot read, is unknown: no check, no
//! notice, no automatic What's new, and it is never written over. Keys this version does not
//! know are kept as they are when the file is written.

pub mod install;
pub mod notes;
pub mod proxy;
pub mod release;
mod version;

use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeDelta, Utc};

pub use release::Release;
pub use version::Version;

/// The file's name, in the state directory.
pub const FILE: &str = "update.toml";

/// The host asked.
pub const HOST: &str = "api.github.com";

/// The address asked.
pub const URL: &str = "https://api.github.com/repos/DokaDev/datarig/releases/latest";

/// The `User-Agent` sent: the same for every version, so it tells nothing about the user.
pub const USER_AGENT: &str = "datarig";

/// At most one attempt in this long, whether it got an answer or not.
pub const INTERVAL: TimeDelta = TimeDelta::hours(24);

/// A notice closed without skipping its version comes back after this long.
pub const REMIND_AFTER: TimeDelta = TimeDelta::days(7);

/// A time further ahead than this is a clock that went back: it does not hold anything off.
pub const CLOCK_SLACK: TimeDelta = TimeDelta::hours(1);

/// How the last attempt ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// An answer with a release.
    Ok,
    /// The release had not changed since the last answer (`304`).
    NotModified,
    /// No connection (no network, a name that did not resolve, refused).
    Unreachable,
    /// No answer in time.
    TimedOut,
    /// The server's certificate was not trusted.
    Tls,
    /// A proxy that cannot be used is set, or the proxy refused.
    Proxy,
    /// GitHub limits requests from this address for now.
    RateLimited,
    /// An answer that is not a release.
    BadAnswer,
    /// A value a newer version wrote; kept as it is.
    Other,
}

impl Outcome {
    const NAMES: [(Outcome, &'static str); 8] = [
        (Outcome::Ok, "ok"),
        (Outcome::NotModified, "not_modified"),
        (Outcome::Unreachable, "unreachable"),
        (Outcome::TimedOut, "timed_out"),
        (Outcome::Tls, "tls"),
        (Outcome::Proxy, "proxy"),
        (Outcome::RateLimited, "rate_limited"),
        (Outcome::BadAnswer, "bad_answer"),
    ];

    /// The name in the file; `None` for [`Outcome::Other`].
    pub fn name(self) -> Option<&'static str> {
        Self::NAMES.iter().find(|(o, _)| *o == self).map(|(_, n)| *n)
    }

    fn from_name(name: &str) -> Self {
        Self::NAMES.iter().find(|(_, n)| *n == name).map_or(Outcome::Other, |(o, _)| *o)
    }

    /// The attempt learnt the latest release.
    pub fn succeeded(self) -> bool {
        matches!(self, Outcome::Ok | Outcome::NotModified)
    }
}

/// What the file says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// When the last attempt started (written before it is sent).
    pub last_attempt: Option<DateTime<Utc>>,
    /// How it ended; `None` while it has not (or never ran).
    pub result: Option<Outcome>,
    /// GitHub's tag for the last answer, sent back so an unchanged release answers `304`.
    pub etag: Option<String>,
    /// The latest release known.
    pub latest: Option<Version>,
    /// Its date, `YYYY-MM-DD`.
    pub latest_date: Option<String>,
    /// The one line that opens its notes.
    pub summary: Option<String>,
    /// The newest version that ran here (What's new).
    pub seen: Option<Version>,
    /// The version the user chose not to be told about.
    pub skipped: Option<Version>,
    /// The version whose notice was last closed, and when.
    pub reminded: Option<(Version, DateTime<Utc>)>,
}

/// What an attempt learnt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// A release, with GitHub's tag for it when there was one.
    Release(Release, Option<String>),
    /// Unchanged since the tag sent.
    NotModified,
    /// No release learnt.
    Failed(Outcome),
}

impl State {
    /// An attempt starts now: it counts as the day's even if it never ends.
    pub fn claim(&mut self, now: DateTime<Utc>) {
        self.last_attempt = Some(now);
        self.result = None;
    }

    /// What the attempt learnt.
    pub fn record(&mut self, answer: Answer) {
        match answer {
            Answer::Release(r, etag) => {
                self.result = Some(Outcome::Ok);
                self.etag = etag;
                self.latest = Some(r.version);
                self.latest_date = r.date;
                self.summary = r.summary;
            }
            Answer::NotModified => self.result = Some(Outcome::NotModified),
            Answer::Failed(o) => self.result = Some(o),
        }
    }

    /// The notice about `version` was closed now: it comes back after [`REMIND_AFTER`].
    pub fn remind(&mut self, version: Version, now: DateTime<Utc>) {
        self.reminded = Some((version, now));
    }

    /// Not to be told about `version` again (a newer one is told about).
    pub fn skip(&mut self, version: Version) {
        self.skipped = Some(version);
    }
}

/// The file, as far as it could be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cache {
    /// No state directory: nothing is kept, so nothing is checked or shown by itself.
    NoStateDir,
    /// The file is there but could not be read: unknown.
    Unreadable(PathBuf),
    /// The file (an empty state when there is none yet).
    Known(PathBuf, Box<State>),
}

impl Cache {
    /// The file of `paths`.
    pub fn load(paths: &crate::paths::Paths) -> Self {
        match paths.update_file() {
            None => Cache::NoStateDir,
            Some(path) => match load(&path) {
                Ok(state) => Cache::Known(path, Box::new(state)),
                Err(Unreadable) => Cache::Unreadable(path),
            },
        }
    }

    pub fn state(&self) -> Option<&State> {
        match self {
            Cache::Known(_, s) => Some(s),
            _ => None,
        }
    }
}

/// The file exists but could not be read, or has a value this version cannot read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unreadable;

/// Read the file: an empty state when there is none.
pub fn load(path: &Path) -> Result<State, Unreadable> {
    read(path)?.map_or(Ok(State::default()), |doc| read_state(&doc))
}

/// The state a document holds.
fn read_state(doc: &toml_edit::DocumentMut) -> Result<State, Unreadable> {
    let time = |key: &str| -> Result<Option<DateTime<Utc>>, Unreadable> {
        doc.get(key).map(|i| i.as_datetime().and_then(utc).ok_or(Unreadable)).transpose()
    };
    let text = |key: &str| -> Result<Option<String>, Unreadable> {
        doc.get(key).map(|i| i.as_str().map(str::to_string).ok_or(Unreadable)).transpose()
    };
    let version = |key: &str| -> Result<Option<Version>, Unreadable> {
        text(key)?.map(|v| Version::parse(&v).ok_or(Unreadable)).transpose()
    };
    let latest_date = match doc.get("latest_date") {
        None => None,
        Some(i) => match i.as_datetime() {
            Some(d) if d.time.is_none() && d.offset.is_none() => d.date.map(|d| d.to_string()),
            _ => return Err(Unreadable),
        },
    };
    let reminded = match doc.get("reminded") {
        None => None,
        Some(i) => {
            let t = i.as_table_like().ok_or(Unreadable)?;
            let v = t.get("version").and_then(|v| v.as_str()).and_then(Version::parse).ok_or(Unreadable)?;
            let at = t.get("at").and_then(|a| a.as_datetime()).and_then(utc).ok_or(Unreadable)?;
            Some((v, at))
        }
    };
    Ok(State {
        last_attempt: time("last_attempt")?,
        result: text("result")?.map(|r| Outcome::from_name(&r)),
        etag: text("etag")?,
        latest: version("latest")?,
        latest_date,
        summary: text("summary")?,
        seen: version("seen")?,
        skipped: version("skipped")?,
        reminded,
    })
}

/// Write `state` into the file atomically. Keys this version does not know stay as they are, and
/// a result this version could not name ([`Outcome::Other`]) is left as the file has it. A file
/// that cannot be read is not written over (an `InvalidData` error).
pub fn save(path: &Path, state: &State) -> std::io::Result<()> {
    let unreadable = |_| std::io::Error::new(std::io::ErrorKind::InvalidData, "update file unreadable");
    let mut doc = read(path).map_err(unreadable)?.unwrap_or_default();
    // A value this version cannot read is unknown too.
    read_state(&doc).map_err(unreadable)?;
    let mut put = |key: &str, value: Option<toml_edit::Value>| match value {
        Some(v) => doc[key] = toml_edit::Item::Value(v),
        None => {
            doc.remove(key);
        }
    };
    put("last_attempt", state.last_attempt.map(|t| datetime(t).into()));
    match state.result {
        Some(Outcome::Other) => {}
        r => put("result", r.and_then(Outcome::name).map(Into::into)),
    }
    put("etag", state.etag.as_deref().map(Into::into));
    put("latest", state.latest.map(|v| v.to_string().into()));
    put(
        "latest_date",
        state.latest_date.as_deref().and_then(|d| d.parse::<toml_edit::Datetime>().ok()).map(Into::into),
    );
    put("summary", state.summary.as_deref().map(Into::into));
    put("seen", state.seen.map(|v| v.to_string().into()));
    put("skipped", state.skipped.map(|v| v.to_string().into()));
    put(
        "reminded",
        state.reminded.map(|(v, at)| {
            let mut t = toml_edit::InlineTable::new();
            t.insert("version", v.to_string().into());
            t.insert("at", datetime(at).into());
            t.into()
        }),
    );
    crate::fsutil::atomic_write(path, doc.to_string().as_bytes())
}

/// A TOML date-time with an offset as UTC.
fn utc(d: &toml_edit::Datetime) -> Option<DateTime<Utc>> {
    d.offset?;
    DateTime::parse_from_rfc3339(&d.to_string()).ok().map(|t| t.with_timezone(&Utc))
}

/// `t` as a TOML date-time in UTC, to the second.
fn datetime(t: DateTime<Utc>) -> toml_edit::Datetime {
    t.format("%Y-%m-%dT%H:%M:%SZ").to_string().parse().expect("an RFC 3339 date-time")
}

/// The file as a document; `None` when it is not there.
fn read(path: &Path) -> Result<Option<toml_edit::DocumentMut>, Unreadable> {
    match std::fs::read_to_string(path) {
        Ok(text) => text.parse::<toml_edit::DocumentMut>().map(Some).map_err(|_| Unreadable),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(Unreadable),
    }
}

/// What turns the check off, besides the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Switches {
    /// `[updates] check` is on.
    pub config_on: bool,
    /// `DATARIG_NO_UPDATE_CHECK` has a value (any but an empty one): off for the process,
    /// whatever the config says.
    pub env_off: bool,
    /// `CI` has a value: a CI service.
    pub ci: bool,
    /// A debug build (tests and development): never on the network.
    pub debug_build: bool,
    /// This instance holds the workspace lock (a second instance only reads).
    pub lock_holder: bool,
}

impl Switches {
    /// For this build (a debug build never checks).
    pub fn new(config_on: bool, env_off: bool, ci: bool, lock_holder: bool) -> Self {
        Switches { config_on, env_off, ci, debug_build: cfg!(debug_assertions), lock_holder }
    }
}

/// Why there is no check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Off {
    /// `DATARIG_NO_UPDATE_CHECK`.
    Env,
    /// `[updates] check = "off"`.
    Config,
    /// `CI`.
    Ci,
    DebugBuild,
    NoStateDir,
    /// The file could not be read.
    Unreadable,
    /// Another instance holds the workspace lock: it checks.
    SecondInstance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Check now (after [`State::claim`] is written).
    Due,
    /// The last attempt was less than [`INTERVAL`] ago.
    NotDue,
    Off(Off),
}

/// Whether to check now.
pub fn decide(now: DateTime<Utc>, cache: &Cache, sw: &Switches) -> Decision {
    let off = if sw.env_off {
        Some(Off::Env)
    } else if !sw.config_on {
        Some(Off::Config)
    } else if sw.ci {
        Some(Off::Ci)
    } else if sw.debug_build {
        Some(Off::DebugBuild)
    } else {
        match cache {
            Cache::NoStateDir => Some(Off::NoStateDir),
            Cache::Unreadable(_) => Some(Off::Unreadable),
            Cache::Known(..) if !sw.lock_holder => Some(Off::SecondInstance),
            Cache::Known(..) => None,
        }
    };
    if let Some(off) = off {
        return Decision::Off(off);
    }
    match cache.state().and_then(|s| s.last_attempt) {
        Some(last) if last <= now + CLOCK_SLACK && now - last < INTERVAL => Decision::NotDue,
        _ => Decision::Due,
    }
}

/// The version to tell about at start, from what the file knows: newer than `running`, not
/// skipped, not closed in the last [`REMIND_AFTER`], and checks on (a second instance tells too,
/// it only does not check).
pub fn notice(now: DateTime<Utc>, cache: &Cache, sw: &Switches, running: &Version) -> Option<Version> {
    match decide(now, cache, sw) {
        Decision::Off(Off::SecondInstance) | Decision::Due | Decision::NotDue => {}
        Decision::Off(_) => return None,
    }
    let s = cache.state()?;
    let latest = s.latest.filter(|l| !l.is_rc() && l > running && s.skipped != Some(*l))?;
    let recently =
        |(v, at): &(Version, DateTime<Utc>)| *v == latest && *at <= now + CLOCK_SLACK && now - *at < REMIND_AFTER;
    (!s.reminded.as_ref().is_some_and(recently)).then_some(latest)
}

/// What's new at start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WhatsNew {
    /// Show the notes of the versions after this one up to the running one; `Some(None)`: the
    /// running version's only (it is not known which version ran before).
    pub show: Option<Option<Version>>,
    /// Write this as the newest version that ran here.
    pub seen: Option<Version>,
}

/// Whether What's new shows at start, and what is written. `existing_user`: this machine used
/// datarig before (the first steps are done or a workspace is kept), so a file without `seen` is
/// an update from a version before What's new, not a first start. `on`: `[updates] whats_new`.
pub fn whats_new(cache: &Cache, running: &Version, existing_user: bool, on: bool, lock_holder: bool) -> WhatsNew {
    let Some(s) = cache.state() else { return WhatsNew::default() };
    if !lock_holder {
        return WhatsNew::default();
    }
    let show = match s.seen {
        Some(seen) if *running > seen => Some(Some(seen)),
        Some(_) => return WhatsNew::default(),
        None if existing_user => Some(None),
        None => None,
    };
    WhatsNew { show: show.filter(|_| on), seen: Some(*running) }
}

#[cfg(test)]
mod tests;
