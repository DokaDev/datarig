//! A datarig version: `MAJOR.MINOR.PATCH` or a release candidate `MAJOR.MINOR.PATCH-rc.N`, the
//! same forms the release script accepts. No leading zeros, nothing else after the patch.

use std::cmp::Ordering;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// `Some(n)` for `-rc.n` (at least 1), which comes before the version without it.
    pub rc: Option<u64>,
}

impl Version {
    /// The version of this build.
    pub fn running() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("the package version is MAJOR.MINOR.PATCH")
    }

    /// `X.Y.Z` or `X.Y.Z-rc.N`; `None` for anything else.
    pub fn parse(s: &str) -> Option<Self> {
        let (core, rc) = match s.split_once('-') {
            Some((core, pre)) => {
                let n = number(pre.strip_prefix("rc.")?).filter(|n| *n > 0)?;
                (core, Some(n))
            }
            None => (s, None),
        };
        let mut parts = core.split('.');
        let v =
            Version { major: number(parts.next()?)?, minor: number(parts.next()?)?, patch: number(parts.next()?)?, rc };
        parts.next().is_none().then_some(v)
    }

    /// A tag `vX.Y.Z` (the `v` optional).
    pub fn parse_tag(tag: &str) -> Option<Self> {
        Self::parse(tag.strip_prefix('v').unwrap_or(tag))
    }

    pub fn is_rc(&self) -> bool {
        self.rc.is_some()
    }
}

/// Digits without a leading zero (but `0` itself).
fn number(s: &str) -> Option<u64> {
    let ok = !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'));
    ok.then(|| s.parse().ok()).flatten()
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        let rc = |v: &Version| v.rc.unwrap_or(u64::MAX);
        (self.major, self.minor, self.patch, rc(self)).cmp(&(other.major, other.minor, other.patch, rc(other)))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(n) = self.rc {
            write!(f, "-rc.{n}")?;
        }
        Ok(())
    }
}
