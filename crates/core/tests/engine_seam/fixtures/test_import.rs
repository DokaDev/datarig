//! A test-only import of a variant under a plain name does not make that name a variant in
//! the product code of the same file.
fn width() -> usize {
    let postgres = 1;
    postgres
}

#[cfg(test)]
mod tests {
    use crate::sql::dialect::Dialect::Postgres as postgres;
}
