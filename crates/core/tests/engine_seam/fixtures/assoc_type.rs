//! An associated type is another name for an engine enum too.
use datarig_core::sql::dialect::Dialect;

trait Has {
    type D;
}

impl Has for u8 {
    type D = Dialect;
}

trait HasToo {
    type D = Dialect;
}

fn on_postgres(d: Dialect) -> bool {
    d == <u8 as Has>::D::Postgres
}

type P = (Dialect);
