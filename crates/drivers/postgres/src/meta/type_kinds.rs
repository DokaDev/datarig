//! The kinds of a database's own types (`pg_type.typtype`), for the columns a result has
//! that the connection reports as base types but are not built in: the connection's own type
//! lookup reads `typtype` only to tell enums and pseudo-types apart, so a multirange of the
//! database's own comes back as a base type too ([`datarig_core::driver::Dated::Lookup`]).
//! Read on the metadata session, in one catalog read of its own ([`super::read`]).

use datarig_core::driver::DbError;
use tokio_postgres::Client;
use tokio_postgres::types::Type;

/// Each type of `$1` (an `oid[]`) that exists, with its `typtype`.
pub const TYPE_KINDS: &str = "SELECT t.oid, t.typtype::pg_catalog.text FROM pg_catalog.pg_type t \
     WHERE t.oid OPERATOR(pg_catalog.=) ANY ($1::pg_catalog.oid[])";

/// The kinds of the types `oids` that exist (a type not among them was dropped).
pub(crate) async fn load_type_kinds(client: &Client, oids: &[u32]) -> Result<Vec<(u32, char)>, DbError> {
    let oids = oids.to_vec();
    let rows = super::read(client, TYPE_KINDS, &[(&oids, Type::OID_ARRAY)]).await?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let oid: u32 = r.get(0);
            let kind: String = r.get(1);
            kind.chars().next().map(|k| (oid, k))
        })
        .collect())
}
