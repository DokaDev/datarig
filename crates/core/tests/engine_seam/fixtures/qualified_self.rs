//! `<E>::V` names a variant past the plain `E::V` form: reported like another name for `E`.
use datarig_core::sql::dialect::Dialect;

fn on_postgres(d: Dialect) -> bool {
    d == <Dialect>::Postgres
}
