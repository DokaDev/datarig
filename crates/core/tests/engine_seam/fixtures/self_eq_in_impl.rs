//! `==` through `Self`, inside the enum's own impl.
pub enum Dialect { Postgres, MySql(MySqlMode) }

impl Dialect {
    /// Whether `E'...'` strings exist.
    pub fn escape_strings(self) -> bool {
        self == Self::Postgres
    }
}

impl (Dialect) {
    fn dollar_quotes(&self) -> bool {
        matches!(self, Self::Postgres)
    }
}
