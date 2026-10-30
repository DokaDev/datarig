//! `<state>/usage.toml`: how often each action ran, by key, from an action menu and from the
//! command line, so a tip about a key the user already knows is not shown again and an action
//! run from a menu a few times can name its key once. Local only, never sent anywhere.
//!
//! The file holds action ids and command names with a count each (at most [`MAX_COUNT`]), the
//! actions whose key was named once (`nudged`), the first steps taken some other way than by
//! running their action (`steps`: a menu opened by a right click, a run from the explorer) and
//! whether the first steps of the welcome panel are done (`onboarding = "done"`). No times, no
//! statements, no names of connections or anything else of the user's:
//!
//! ```toml
//! onboarding = "done"
//! nudged = ["explorer.show_ddl"]
//! steps = ["menu.open"]
//!
//! [keys]
//! "query.execute_current" = 12
//!
//! [menu]
//! "explorer.show_ddl" = 3
//! ```
//!
//! A file that is not there is empty usage. A file that is there but cannot be read or parsed is
//! never written over ([`save`] refuses): unknown is not "nothing used". Keys and ids this version
//! does not know are kept as they are when the file is written, and a count only grows (a second
//! instance that counted more keeps its count).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The file's name, in the state directory.
pub const FILE: &str = "usage.toml";

/// A count stops growing here.
pub const MAX_COUNT: u32 = 9999;

/// How an action was run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Key,
    /// An action menu (the keys of the menu itself are not counted as the action's key).
    Menu,
    /// The `:` command line.
    Command,
}

impl Via {
    fn table(self) -> &'static str {
        match self {
            Via::Key => "keys",
            Via::Menu => "menu",
            Via::Command => "command",
        }
    }
}

/// What the file says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub keys: BTreeMap<String, u32>,
    pub menu: BTreeMap<String, u32>,
    pub command: BTreeMap<String, u32>,
    /// The actions whose key a status line named once.
    pub nudged: BTreeSet<String>,
    /// The first steps (by action id) taken without running their action.
    pub steps: BTreeSet<String>,
    /// The first steps of the welcome panel are done or were dismissed.
    pub onboarding_done: bool,
}

impl Usage {
    fn counts(&self, via: Via) -> &BTreeMap<String, u32> {
        match via {
            Via::Key => &self.keys,
            Via::Menu => &self.menu,
            Via::Command => &self.command,
        }
    }

    /// How often `id` ran `via`.
    pub fn count(&self, via: Via, id: &str) -> u32 {
        self.counts(via).get(id).copied().unwrap_or(0)
    }

    /// `id` ran once more `via`; returns the new count.
    pub fn bump(&mut self, via: Via, id: &str) -> u32 {
        let counts = match via {
            Via::Key => &mut self.keys,
            Via::Menu => &mut self.menu,
            Via::Command => &mut self.command,
        };
        match counts.get_mut(id) {
            Some(n) => {
                *n = n.saturating_add(1).min(MAX_COUNT);
                *n
            }
            None => {
                counts.insert(id.to_string(), 1);
                1
            }
        }
    }

    /// Every count added up.
    pub fn total(&self) -> u64 {
        [&self.keys, &self.menu, &self.command].iter().flat_map(|m| m.values()).map(|n| u64::from(*n)).sum()
    }
}

/// The file exists but could not be read or parsed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unreadable;

/// Read the file: empty usage when there is none. Entries of a type this version does not read
/// (a count that is not a number) are left out here and kept in the file by [`save`].
pub fn load(path: &Path) -> Result<Usage, Unreadable> {
    let doc = read(path)?.unwrap_or_default();
    let mut u = Usage::default();
    for via in [Via::Key, Via::Menu, Via::Command] {
        let Some(t) = doc.get(via.table()).and_then(toml_edit::Item::as_table_like) else { continue };
        let counts = match via {
            Via::Key => &mut u.keys,
            Via::Menu => &mut u.menu,
            Via::Command => &mut u.command,
        };
        for (id, v) in t.iter() {
            if let Some(n) = v.as_integer().and_then(|n| u32::try_from(n).ok()) {
                counts.insert(id.to_string(), n.min(MAX_COUNT));
            }
        }
    }
    u.nudged = names(&doc, "nudged");
    u.steps = names(&doc, "steps");
    u.onboarding_done = doc.get("onboarding").and_then(toml_edit::Item::as_str) == Some("done");
    Ok(u)
}

/// Write `usage` into the file atomically, merged with what the file has now: a count is the
/// larger of the two, `nudged` and a done onboarding are kept from both, and everything else of
/// the file stays as it is. A file that cannot be read or parsed is not written over (an
/// `InvalidData` error).
pub fn save(path: &Path, usage: &Usage) -> std::io::Result<()> {
    let unreadable = |_| std::io::Error::new(std::io::ErrorKind::InvalidData, "usage file unreadable");
    let mut doc = read(path).map_err(unreadable)?.unwrap_or_default();
    for via in [Via::Key, Via::Menu, Via::Command] {
        let ours = usage.counts(via);
        if ours.is_empty() {
            continue;
        }
        match doc.get(via.table()) {
            None => doc[via.table()] = toml_edit::table(),
            // Something else under that name: left as it is.
            Some(i) if !i.is_table_like() => continue,
            Some(_) => {}
        }
        let Some(t) = doc[via.table()].as_table_like_mut() else { continue };
        for (id, n) in ours {
            let theirs = t.get(id).and_then(|v| v.as_integer()).and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
            t.insert(id, toml_edit::value(i64::from((*n).max(theirs).min(MAX_COUNT))));
        }
    }
    for (key, ours) in [("nudged", &usage.nudged), ("steps", &usage.steps)] {
        // Something else under that name: left as it is.
        if ours.is_empty() || doc.get(key).is_some_and(|n| n.as_array().is_none()) {
            continue;
        }
        let mut all = names(&doc, key);
        all.extend(ours.iter().cloned());
        doc[key] = toml_edit::value(all.iter().map(String::as_str).collect::<toml_edit::Array>());
    }
    if usage.onboarding_done {
        doc["onboarding"] = toml_edit::value("done");
    }
    crate::fsutil::atomic_write(path, doc.to_string().as_bytes())
}

/// The strings of the array `key` (none: empty).
fn names(doc: &toml_edit::DocumentMut, key: &str) -> BTreeSet<String> {
    let a = doc.get(key).and_then(toml_edit::Item::as_array);
    a.into_iter().flat_map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string))).collect()
}

/// The file as a document; `None` when it is not there.
fn read(path: &Path) -> Result<Option<toml_edit::DocumentMut>, Unreadable> {
    match std::fs::read_to_string(path) {
        Ok(text) => text.parse::<toml_edit::DocumentMut>().map(Some).map_err(|_| Unreadable),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(Unreadable),
    }
}

#[cfg(test)]
mod tests;
