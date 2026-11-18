//! `<E>::V` names a variant past the plain `E::V` form: reported like another name for `E`.
use datarig_core::sql::dialect::Dialect;

fn on_postgres(d: Dialect) -> bool {
    d == <Dialect>::Postgres
}

impl Dialect {
    fn is_postgres(&self) -> bool {
        *self == <Self>::Postgres
    }
}

type Identity<T> = T;

fn through_identity(d: Dialect) -> bool {
    d == <Identity<Dialect>>::Postgres
}
