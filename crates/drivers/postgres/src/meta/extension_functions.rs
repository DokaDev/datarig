//! The functions of the extensions a read-only environment allows (`read_only_extensions`),
//! for the calls a read-only policy lets run: which of the extensions the database has, each
//! function that belongs to one of them, and every other function of the same name in any
//! schema (`pg_catalog` too), since the server may pick that one instead. Aggregates, window
//! functions and procedures are listed too. Read on the metadata session, in one catalog read
//! of its own ([`super::read`]).

use datarig_core::driver::DbError;
use datarig_core::sql::risk::{CatalogFunction, ExtensionFunctions};
use tokio_postgres::Client;
use tokio_postgres::types::Type;

/// For the extensions named in `$1` (a `text[]`): a row of each one installed, with no schema
/// and its name third; then a row of each function whose name one of their functions has,
/// with its schema, its name and the extension of `$1` it belongs to (NULL for any other).
pub const EXTENSION_FUNCTIONS: &str = "WITH ext AS (\
       SELECT e.oid, e.extname::pg_catalog.text AS extname FROM pg_catalog.pg_extension e \
       WHERE e.extname::pg_catalog.text OPERATOR(pg_catalog.=) ANY ($1::pg_catalog.text[])), \
     own AS (\
       SELECT d.objid, ext.extname FROM pg_catalog.pg_depend d \
       JOIN ext ON ext.oid OPERATOR(pg_catalog.=) d.refobjid \
       WHERE d.classid OPERATOR(pg_catalog.=) 'pg_catalog.pg_proc'::pg_catalog.regclass \
         AND d.refclassid OPERATOR(pg_catalog.=) 'pg_catalog.pg_extension'::pg_catalog.regclass \
         AND d.deptype OPERATOR(pg_catalog.=) 'e') \
     SELECT NULL::pg_catalog.text, NULL::pg_catalog.text, ext.extname FROM ext \
     UNION ALL \
     SELECT n.nspname::pg_catalog.text, p.proname::pg_catalog.text, own.extname \
     FROM pg_catalog.pg_proc p \
     JOIN pg_catalog.pg_namespace n ON n.oid OPERATOR(pg_catalog.=) p.pronamespace \
     LEFT JOIN own ON own.objid OPERATOR(pg_catalog.=) p.oid \
     WHERE p.proname OPERATOR(pg_catalog.=) ANY (\
       SELECT q.proname FROM pg_catalog.pg_proc q JOIN own ON own.objid OPERATOR(pg_catalog.=) q.oid)";

/// The functions of the extensions `extensions` in the session's database
/// ([`EXTENSION_FUNCTIONS`]).
pub(crate) async fn load_extension_functions(
    client: &Client,
    extensions: &[String],
) -> Result<ExtensionFunctions, DbError> {
    let names = extensions.to_vec();
    let rows = super::read(client, EXTENSION_FUNCTIONS, &[(&names, Type::TEXT_ARRAY)]).await?;
    let mut answer = ExtensionFunctions::default();
    for r in &rows {
        let schema: Option<String> = r.get(0);
        let name: Option<String> = r.get(1);
        let extension: Option<String> = r.get(2);
        match (schema, name, extension) {
            (Some(schema), Some(name), extension) => answer.functions.push(CatalogFunction { schema, name, extension }),
            (None, _, Some(extension)) => answer.installed.push(extension),
            _ => {}
        }
    }
    Ok(answer)
}
