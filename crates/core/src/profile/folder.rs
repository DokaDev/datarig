//! Profile folders: nested, `/`-separated paths such as `work/prod`.
//! Folders only organize profiles; they change no behavior. [`Folders`] is the set the config
//! file keeps (`folders = [...]`, so empty folders survive) plus which folders are expanded.
//! The explorer draws them; where the expanded state is stored between runs is the
//! tab-restore state file.

use std::collections::BTreeSet;
use std::fmt;

/// Why a folder path is invalid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderError {
    Empty,
    /// An empty segment (`a//b`, a leading or trailing `/`).
    EmptySegment,
    /// Spaces around a segment.
    Spaces,
    /// `.` or `..`.
    Dots,
}

impl fmt::Display for FolderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FolderError::Empty => "empty folder path",
            FolderError::EmptySegment => "empty folder name",
            FolderError::Spaces => "spaces around a folder name",
            FolderError::Dots => "`.` or `..` as a folder name",
        })
    }
}

/// A valid folder path (`work/prod`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FolderPath(String);

impl FolderPath {
    pub fn parse(s: &str) -> Result<Self, FolderError> {
        if s.is_empty() {
            return Err(FolderError::Empty);
        }
        for seg in s.split('/') {
            if seg.is_empty() {
                return Err(FolderError::EmptySegment);
            }
            if seg.trim() != seg {
                return Err(FolderError::Spaces);
            }
            if seg == "." || seg == ".." {
                return Err(FolderError::Dots);
            }
        }
        Ok(FolderPath(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The last segment.
    pub fn name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    pub fn parent(&self) -> Option<FolderPath> {
        self.0.rsplit_once('/').map(|(p, _)| FolderPath(p.to_string()))
    }

    pub fn depth(&self) -> usize {
        self.0.matches('/').count()
    }

    /// This path and every ancestor, outermost first (`a`, `a/b`, `a/b/c`).
    pub fn with_ancestors(&self) -> Vec<FolderPath> {
        let mut out: Vec<FolderPath> = std::iter::successors(Some(self.clone()), FolderPath::parent).collect();
        out.reverse();
        out
    }

    /// `self` is `other` or inside it.
    pub fn is_within(&self, other: &FolderPath) -> bool {
        self.0 == other.0 || self.0.starts_with(&format!("{}/", other.0))
    }

    /// `self` with its prefix `from` replaced by `to` (a moved or renamed folder).
    pub fn rebase(&self, from: &FolderPath, to: &FolderPath) -> Option<FolderPath> {
        if self.0 == from.0 {
            Some(to.clone())
        } else {
            self.0.strip_prefix(&format!("{}/", from.0)).map(|rest| FolderPath(format!("{}/{rest}", to.0)))
        }
    }
}

impl fmt::Display for FolderPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The folder tree: every folder (ancestors included) and which ones are expanded.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Folders {
    paths: BTreeSet<FolderPath>,
    expanded: BTreeSet<FolderPath>,
}

impl Folders {
    /// Add `path` and its ancestors.
    pub fn insert(&mut self, path: &FolderPath) {
        self.paths.extend(path.with_ancestors());
    }

    pub fn contains(&self, path: &FolderPath) -> bool {
        self.paths.contains(path)
    }

    /// Every folder, sorted by path.
    pub fn iter(&self) -> impl Iterator<Item = &FolderPath> {
        self.paths.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Direct children of `parent` (`None`: the top level), by name, ignoring case.
    pub fn children(&self, parent: Option<&FolderPath>) -> Vec<&FolderPath> {
        let mut v: Vec<&FolderPath> = self.paths.iter().filter(|p| p.parent().as_ref() == parent).collect();
        v.sort_by_key(|p| p.name().to_lowercase());
        v
    }

    /// Remove `path` and everything inside it. Profiles in it are the caller's to move.
    pub fn remove(&mut self, path: &FolderPath) {
        self.paths.retain(|p| !p.is_within(path));
        self.expanded.retain(|p| !p.is_within(path));
    }

    /// Rename or move `from` (and its subfolders) to `to`; the expanded state follows.
    pub fn rename(&mut self, from: &FolderPath, to: &FolderPath) {
        let rebase = |set: &BTreeSet<FolderPath>| -> BTreeSet<FolderPath> {
            set.iter().map(|p| p.rebase(from, to).unwrap_or_else(|| p.clone())).collect()
        };
        let paths = rebase(&self.paths);
        self.expanded = rebase(&self.expanded);
        self.paths = BTreeSet::new();
        for p in &paths {
            self.insert(p);
        }
    }

    pub fn is_expanded(&self, path: &FolderPath) -> bool {
        self.expanded.contains(path)
    }

    /// Expand or collapse a known folder; returns the new state.
    pub fn toggle(&mut self, path: &FolderPath) -> bool {
        if !self.paths.contains(path) {
            return false;
        }
        if !self.expanded.remove(path) {
            self.expanded.insert(path.clone());
            return true;
        }
        false
    }

    /// The expanded folders (what the state file keeps).
    pub fn expanded(&self) -> impl Iterator<Item = &FolderPath> {
        self.expanded.iter()
    }

    /// Restore the expanded state; unknown folders are ignored.
    pub fn set_expanded<'a>(&mut self, paths: impl IntoIterator<Item = &'a FolderPath>) {
        self.expanded = paths.into_iter().filter(|p| self.paths.contains(*p)).cloned().collect();
    }
}

#[cfg(test)]
mod tests;
