//! Which language a tab's text is written in. The SQL tools (lexing, quoting, the risk
//! classifier) pick their rules from it; a driver says which one its sessions speak
//! (`driver::Capabilities::language`).

/// The SQL dialect a tab's text is in. PostgreSQL is the only one for now; a MySQL variant
/// will join it, and an exhaustive `match` makes every tool decide what to do with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Dialect {
    #[default]
    Postgres,
}

/// The language of an editor's text: SQL of some dialect (other kinds of consoles may join
/// it later).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Sql(Dialect),
}

impl Default for Language {
    /// PostgreSQL SQL: what a tab without a profile (or with one whose driver is unknown) is
    /// written in.
    fn default() -> Self {
        Self::Sql(Dialect::default())
    }
}

impl Language {
    /// The SQL dialect of the text.
    pub fn dialect(self) -> Dialect {
        match self {
            Self::Sql(d) => d,
        }
    }
}

#[cfg(test)]
mod tests;
