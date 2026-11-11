//! `DbCommand::GroupCount`: the values of one column and the rows that hold each, read as a count
//! is read ([`count`](super::count)): one statement after the allowlist's question, bounded by the
//! statement's own hints ([`bounded_select`]), in a read-only transaction of its own at
//! `REPEATABLE READ` when the session runs without autocommit and none is open
//! ([`read_own_as`]). Every value is shown as a cell of its column is ([`display`]).
//!
//! It is refused unsent where reading would take row locks: a transaction open, or autocommit
//! off, while the level reads run in is `SERIALIZABLE` or not known
//! ([`DbError::LockingRead`]); and where it would use up the next transaction's settings
//! ([`keeps_next`]).

use super::*;
use datarig_core::driver::{GroupCounts, Isolation};

/// Count the rows of each value with `sql` (the app's group count, on the allowlist, checked
/// again here); also whether it counted in the snapshot of the user's transaction, as
/// [`count`](super::count) says it.
pub(super) async fn group_count(
    conn: &mut Conn,
    link: &mut Link,
    env: &Env<'_>,
    s: &mut State,
    sql: &str,
    limit: Option<Duration>,
) -> Result<(Result<GroupCounts, DbError>, bool), Stop> {
    if let Err(why) = risk::repeatable(sql, s.mode) {
        return Ok((Err(DbError::NotRepeatable(why)), false));
    }
    let was_open = open_before(conn, link, s).await?;
    if let Err(why) = keeps_next(s, was_open) {
        return Ok((Err(why), false));
    }
    if let Err(why) = no_locking_read(s, was_open) {
        return Ok((Err(why), false));
    }
    let asked = match asked_server(conn, link, env, s, sql).await? {
        Ok(asked) => asked,
        Err(why) => return Ok((Err(why), false)),
    };
    let snapshot = s.tx.open && matches!(asked.isolation.as_str(), "REPEATABLE-READ" | "SERIALIZABLE");
    let bounded = limit.and_then(|l| bounded_select(sql, l, &asked, env.server.mariadb, ""));
    let (text, applied) = match &bounded {
        Some((text, applied)) => (text.as_str(), Some(*applied)),
        None => (sql, None),
    };
    let read = read_own_as(conn, link, env, s, text, was_open, applied, OwnWant::Rows).await?;
    let counted = read.and_then(|got| match got {
        OwnGot::Rows(rows) => GroupCounts::from_rows(rows),
        OwnGot::First(_) => Err(DbError::NoResult),
    });
    Ok((counted, snapshot))
}

/// Refused ([`DbError::LockingRead`]) when a read of the app's own would lock the rows it reads:
/// it runs in a transaction (one open before it, `was_open`, or one autocommit off opens) whose
/// level is `SERIALIZABLE` or not known (InnoDB reads every row of such a transaction with a
/// shared lock).
fn no_locking_read(s: &State, was_open: bool) -> Result<(), DbError> {
    let in_tx = was_open || s.tx.open || !s.tx.autocommit;
    let plain = matches!(
        s.tracked.isolation(),
        Some(Isolation::ReadUncommitted | Isolation::ReadCommitted | Isolation::RepeatableRead)
    );
    if in_tx && !plain { Err(DbError::LockingRead) } else { Ok(()) }
}
