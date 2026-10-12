//! The names of the file's named tables a person types: a tunnel preset (`[tunnels.<name>]`)
//! and an environment (`[policy.<name>]`). A name is written as typed, so blanks around it are
//! an error rather than trimmed; two names that differ only in case are taken for the same.

/// The longest a name may be (characters).
pub const NAME_MAX: usize = 64;

/// Why a name cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameProblem {
    Empty,
    /// Blanks before or after it.
    Spaces,
    /// A control character (a tab, a line break, …).
    Control,
    /// Longer than [`NAME_MAX`].
    TooLong,
}

/// What keeps `name` from being a name (`None`: it can be one).
pub fn name_problem(name: &str) -> Option<NameProblem> {
    if name.is_empty() {
        Some(NameProblem::Empty)
    } else if name.trim() != name {
        Some(NameProblem::Spaces)
    } else if name.chars().any(char::is_control) {
        Some(NameProblem::Control)
    } else if name.chars().count() > NAME_MAX {
        Some(NameProblem::TooLong)
    } else {
        None
    }
}

/// Two names a person would take for the same one (they differ only in case).
pub fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}
