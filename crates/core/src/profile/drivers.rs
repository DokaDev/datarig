//! The database drivers the app knows by name: the one place that lists them, with the
//! spellings a profile or a file may use and what the interface calls them. Which of them have
//! an implementation is the front end's (`datarig-tui`'s driver registry); a name here with none
//! is shown as not yet supported.

/// A driver the app knows by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Known {
    /// The name the profiles and the config file write.
    pub name: &'static str,
    /// Other spellings a profile may use for it.
    pub aliases: &'static [&'static str],
    /// The name the interface shows.
    pub display: &'static str,
}

/// Every driver the app knows, in the order the profile form lists them.
pub const KNOWN: [Known; 5] = [
    Known { name: "postgres", aliases: &["postgresql", "pg"], display: "PostgreSQL" },
    Known { name: "mysql", aliases: &["mariadb"], display: "MySQL" },
    Known { name: "sqlite", aliases: &["sqlite3"], display: "SQLite" },
    Known { name: "redis", aliases: &[], display: "Redis" },
    Known { name: "elasticsearch", aliases: &[], display: "Elasticsearch" },
];

/// The name of the driver `driver` is spelled as (case-insensitive, aliases included), `None`
/// for a name the app does not know.
pub fn canonical(driver: &str) -> Option<&'static str> {
    KNOWN
        .iter()
        .find(|k| k.name.eq_ignore_ascii_case(driver) || k.aliases.iter().any(|a| a.eq_ignore_ascii_case(driver)))
        .map(|k| k.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_resolve_to_their_driver() {
        assert_eq!(canonical("PostgreSQL"), Some("postgres"));
        assert_eq!(canonical("pg"), Some("postgres"));
        assert_eq!(canonical("MariaDB"), Some("mysql"));
        assert_eq!(canonical("sqlite3"), Some("sqlite"));
    }

    #[test]
    fn unknown_names_resolve_to_nothing() {
        assert_eq!(canonical("oracle"), None);
        assert_eq!(canonical(""), None);
    }
}
