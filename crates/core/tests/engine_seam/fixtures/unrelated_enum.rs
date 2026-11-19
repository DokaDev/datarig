//! An enum of its own whose variants share names with an engine enum's: not an engine enum.
enum D {
    Postgres,
    Other,
}

fn is_postgres(d: D) -> bool {
    matches!(d, D::Postgres)
}

fn label(d: D) -> &'static str {
    match d {
        D::Postgres => "line",
        _ => "bar",
    }
}
