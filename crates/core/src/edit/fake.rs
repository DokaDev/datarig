//! A format for the neutral layer's tests: PostgreSQL's quoting, a physical row locator for
//! "one of several", floats, JSON and unknown types inexact. Its statements may be broken on
//! purpose ([`Fake::broken`]) to show the planner refuses them.

use super::*;
use crate::driver::ValueKind;
use crate::sql::dialect::{Dialect, Language};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Broken {
    /// Writes as it should.
    No,
    /// An `UPDATE` and a `DELETE` without the condition.
    NoWhere,
    /// An `UPDATE` with a condition that reads no column.
    AlwaysTrue,
    /// A second statement after each write.
    Two,
    /// The condition it was given, then `OR` another.
    Widened,
    /// A check that writes.
    WritingCheck,
    /// A check that ends the transaction.
    CommittingCheck,
    /// A count that drops the table.
    DroppingCount,
    /// An insert that deletes.
    InsertDeletes,
    /// The condition in a comment, the write unlimited.
    Commented,
    /// The condition OR-ed into a wider one, in a subquery the classifier does not see through.
    Subquery,
    /// A `DELETE` asked for an `UPDATE`.
    Swapped,
    /// Another table's name in every statement.
    OtherTable,
    /// An `UPDATE` that joins another table and sets its column.
    Joined,
}

pub struct Fake {
    pub offers: &'static [Offer],
    pub broken: Broken,
    /// It can change just one of several rows.
    pub one: bool,
}

pub const ALL: Fake = Fake { offers: &[Offer::Primary, Offer::Unique, Offer::Values], broken: Broken::No, one: true };

impl Fake {
    pub const fn offering(offers: &'static [Offer]) -> Self {
        Self { offers, ..ALL }
    }

    pub const fn broken(broken: Broken) -> Self {
        Self { broken, ..ALL }
    }
}

const D: Dialect = Dialect::Postgres;

impl Fake {
    /// `w` as the statement holds it: as given, unless broken so.
    fn cond(&self, w: &Where) -> String {
        match self.broken {
            Broken::NoWhere => String::new(),
            Broken::AlwaysTrue => " WHERE 1 = 1".into(),
            Broken::Widened => format!(" WHERE {} OR true", w.as_str()),
            Broken::Commented => format!(" WHERE id > 0 /* WHERE {} */", w.as_str()),
            Broken::Subquery => format!(" WHERE id IN (SELECT id FROM app.t WHERE {}) OR name IS NULL", w.as_str()),
            _ => format!(" WHERE {}", w.as_str()),
        }
    }

    fn tail(&self) -> &'static str {
        if self.broken == Broken::Two { "; DELETE FROM \"app\".\"t\"" } else { "" }
    }

    /// `w` narrowed to one row for `scope`.
    fn scoped(&self, t: &TableRef, w: &Where, scope: Scope) -> Option<String> {
        match scope {
            Scope::All => Some(self.cond(w)),
            Scope::One if self.one => {
                Some(format!(" WHERE ctid = (SELECT ctid FROM {}{} LIMIT 1)", self.table(t), self.cond(w)))
            }
            Scope::One => None,
        }
    }
}

impl EditSql for Fake {
    fn offers(&self) -> &'static [Offer] {
        self.offers
    }

    fn exactness(&self, c: &ColumnInfo) -> Exactness {
        match c.kind {
            ValueKind::Float => Exactness::Inexact(Inexact::Approximate),
            ValueKind::Json => Exactness::Inexact(Inexact::Document),
            ValueKind::Other => Exactness::Inexact(Inexact::Unknown),
            _ => Exactness::Exact,
        }
    }

    fn language(&self) -> Language {
        Language::Sql(D)
    }

    fn table(&self, t: &TableRef) -> String {
        if self.broken == Broken::OtherTable {
            return "app.other".into();
        }
        format!("{}.{}", D.quote_ident(&t.schema), D.quote_ident(&t.name))
    }

    fn column(&self, name: &str) -> String {
        D.quote_ident(name)
    }

    fn value(&self, _: &TableColumn, v: &Value) -> String {
        match v {
            Value::Text(t) => D.quote_literal(t),
            Value::Null => "NULL".into(),
            Value::Default => "DEFAULT".into(),
        }
    }

    fn eq_exact(&self, c: &TableColumn, v: Option<&str>) -> String {
        let col = self.column(&c.column.name);
        match v {
            Some(v) => format!("{col} IS NOT DISTINCT FROM {}", D.quote_literal(v)),
            None => format!("{col} IS NULL"),
        }
    }

    fn count(&self, t: &TableRef, w: &Where) -> String {
        if self.broken == Broken::DroppingCount {
            return format!("DROP TABLE {}", self.table(t));
        }
        format!("SELECT count(*) FROM {} WHERE {}", self.table(t), w.as_str())
    }

    fn check(&self, t: &TableRef, w: &Where) -> String {
        match self.broken {
            Broken::WritingCheck => return format!("DELETE FROM {} WHERE {}", self.table(t), w.as_str()),
            Broken::CommittingCheck => return "COMMIT".into(),
            _ => {}
        }
        format!("SELECT 1 FROM {} WHERE {} FOR UPDATE NOWAIT", self.table(t), w.as_str())
    }

    fn update(&self, t: &TableRef, set: &[(&TableColumn, String)], w: &Where, scope: Scope) -> Option<String> {
        if self.broken == Broken::Swapped {
            return self.delete(t, w, scope);
        }
        let set: Vec<String> = set.iter().map(|(c, v)| format!("{} = {v}", self.column(&c.column.name))).collect();
        let set =
            if self.broken == Broken::Joined { format!("{} FROM app.other", set.join(", ")) } else { set.join(", ") };
        Some(format!("UPDATE {} SET {set}{}{}", self.table(t), self.scoped(t, w, scope)?, self.tail()))
    }

    fn delete(&self, t: &TableRef, w: &Where, scope: Scope) -> Option<String> {
        Some(format!("DELETE FROM {}{}{}", self.table(t), self.scoped(t, w, scope)?, self.tail()))
    }

    fn tails(&self) -> &'static [&'static str] {
        &["", " LIMIT 1)"]
    }

    fn insert(&self, t: &TableRef, cells: &[(&TableColumn, String)]) -> String {
        if self.broken == Broken::InsertDeletes {
            return format!("DELETE FROM {}", self.table(t));
        }
        if cells.is_empty() {
            return format!("INSERT INTO {} DEFAULT VALUES", self.table(t));
        }
        let cols: Vec<String> = cells.iter().map(|(c, _)| self.column(&c.column.name)).collect();
        let vals: Vec<&str> = cells.iter().map(|(_, v)| v.as_str()).collect();
        format!("INSERT INTO {} ({}) VALUES ({})", self.table(t), cols.join(", "), vals.join(", "))
    }
}
