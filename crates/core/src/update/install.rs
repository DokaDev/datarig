//! How this datarig was installed, from where its executable is, so an update names the way that
//! fits. When it is not clear, it is [`Install::Other`]: a download, never a wrong command.

use std::path::{Component, Path, PathBuf};

/// The command that updates a Homebrew install (the full name: never another formula's).
pub const BREW_COMMAND: &str = "brew upgrade dokadev/tap/datarig";

/// The commands that update a build from the source, run in its folder.
pub const SOURCE_COMMANDS: [&str; 2] = ["git pull", "cargo install --locked --path crates/tui"];

/// The page of the latest release, with its archives.
pub const RELEASES_URL: &str = "https://github.com/DokaDev/datarig/releases/latest";

/// What's new on the docs site.
pub const WHATS_NEW_URL: &str = "https://dokadev.github.io/datarig/whats-new.html";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Install {
    /// In a Homebrew `Cellar/datarig/` (any prefix).
    Homebrew,
    /// Built from the source: in Cargo's `bin` folder, or in a `target/release` or
    /// `target/debug` folder.
    Source,
    /// Anything else: an archive from a release, or not known.
    Other,
}

impl Install {
    /// For the running executable, with Cargo's home `cargo_home` (both with their links
    /// followed).
    pub fn current(cargo_home: Option<&Path>) -> Self {
        let Ok(exe) = std::env::current_exe() else { return Install::Other };
        let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
        let cargo_home = cargo_home.map(|h| std::fs::canonicalize(h).unwrap_or_else(|_| h.to_path_buf()));
        Self::detect(&exe, cargo_home.as_deref())
    }

    /// For the executable `exe` (its links already followed) with Cargo's home `cargo_home`.
    pub fn detect(exe: &Path, cargo_home: Option<&Path>) -> Self {
        let names: Vec<_> =
            exe.components().filter_map(|c| if let Component::Normal(n) = c { n.to_str() } else { None }).collect();
        let follows = |a: &str, b: &[&str]| names.windows(2).any(|w| w[0] == a && b.contains(&w[1]));
        if follows("Cellar", &["datarig"]) {
            return Install::Homebrew;
        }
        if cargo_home.is_some_and(|h| exe.parent() == Some(&h.join("bin"))) || follows("target", &["release", "debug"])
        {
            return Install::Source;
        }
        Install::Other
    }
}

/// Cargo's home: `CARGO_HOME` (`cargo_home_var`), else `.cargo` in the home folder. Empty values
/// count as not set.
pub fn cargo_home(cargo_home_var: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    let set = |p: Option<PathBuf>| p.filter(|p| !p.as_os_str().is_empty());
    set(cargo_home_var).or_else(|| set(home).map(|h| h.join(".cargo")))
}

#[cfg(test)]
mod tests;
