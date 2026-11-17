//! Boolean engine tests inside macro forms that are not plain arguments.
use super::dialect::Dialect;

fn flags(d: Dialect) -> Vec<bool> {
    let mut all = Vec::new();
    all.extend(vec![matches!(d, Dialect::Postgres); 2]);
    all.extend(odd_macro!(=> [matches!(d, Dialect::MySql(_))]));
    all
}

macro_rules! check {
    () => {
        fn check(d: Dialect) -> bool {
            let x = d;
            x == Dialect::Postgres
        }
    };
}
