//! `DbCommand::GroupCount`: the values of one column and the rows that hold each, read as a count
//! is read ([`count`](super::count)): one statement after the allowlist's question, bounded by the
//! statement's own hints ([`bounded_select`]), in a read-only transaction of its own at
//! `REPEATABLE READ` when the session runs without autocommit and none is open
//! ([`read_own_as`]). Every value is shown as a cell of its column is ([`display`]).
//!
//! It is refused unsent where reading would take row locks: a transaction open while the level
//! reads run in is `SERIALIZABLE` or not known ([`DbError::LockingRead`]); and where it would use
//! up the next transaction's settings ([`keeps_next`]). With autocommit off and no transaction
//! open it reads in a `REPEATABLE READ` read-only transaction of its own, without row locks,
//! whatever the session's level ([`read_own_as`]).
//!
//! What it reads is bounded by the statement: every value is measured and sent only up to the
//! preview's size, whatever type its column was said to have. [`rows_within`] checks the
//! answer's columns say so ([`bounded`]) and each row against the answer's budget
//! ([`GroupBudget`]) as it comes; past either it stops reading and the connection is cut.
//!
//! The server groups strings and bytes by their first `max_sort_length` bytes: values of such a
//! column at least that long (the session's value, read with the allowlist's question right
//! before the count, on the same session) are marked as possibly sharing a group with others
//! ([`GroupCounts::mark_prefix`]), apart from whether they were sent whole.

use super::*;
use datarig_core::driver::{GroupBudget, GroupCounts, Isolation};
use datarig_core::query::GROUP_PREVIEW;
use mysql_async::Value;
use mysql_async::consts::{ColumnFlags, ColumnType};

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
        OwnGot::Rows(rows, prefixed) => GroupCounts::from_rows(rows).map(|mut counts| {
            // Strings and bytes are grouped by their first `max_sort_length` bytes.
            if prefixed {
                counts.mark_prefix(asked.sort_length);
            }
            counts
        }),
        OwnGot::First(_) => Err(DbError::NoResult),
    });
    Ok((counted, snapshot))
}

/// Refused ([`DbError::LockingRead`]) when a read of the app's own would lock the rows it reads:
/// it runs in the user's transaction (one open before it, `was_open`, or open now) whose level
/// is `SERIALIZABLE` or not known (InnoDB reads every row of such a transaction with a shared
/// lock). With autocommit off and none open it runs in a transaction of its own at `REPEATABLE
/// READ` ([`read_own_as`]), which reads without locks.
fn no_locking_read(s: &State, was_open: bool) -> Result<(), DbError> {
    let in_tx = was_open || s.tx.open;
    let plain = matches!(
        s.tracked.isolation(),
        Some(Isolation::ReadUncommitted | Isolation::ReadCommitted | Isolation::RepeatableRead)
    );
    if in_tx && !plain { Err(DbError::LockingRead) } else { Ok(()) }
}

/// The rows of `text` (a group count), each value as the session shows a cell of it, read one by
/// one: only when its columns say every value comes measured ([`bounded`]), and each row within
/// the answer's budget ([`GroupBudget`]). When either fails, reading stops there: why, with the
/// rest of the answer unread (the caller cuts the connection, so none of it is received or
/// kept). With whether the server groups the values by a prefix ([`prefixed`]).
pub(super) async fn rows_within(
    conn: &mut Conn,
    text: &str,
) -> Result<Result<(Vec<Vec<Cell>>, bool), DbError>, mysql_async::Error> {
    let mut qr = conn.query_iter(text).await?;
    let columns = qr.columns().unwrap_or_else(|| Arc::from(Vec::new()));
    if let Err(why) = bounded(&columns) {
        return Ok(Err(why));
    }
    // As text: the server sends at most the preview's bytes of the value in its column's
    // character set, which the connection's (`utf8mb4`) writes in at most 4 bytes for each.
    let mut budget = GroupBudget::new(4 * GROUP_PREVIEW);
    let mut rows = Vec::new();
    while let Some(row) = qr.next().await? {
        let bytes = (0..row.len()).map(|i| row.as_ref(i).map_or(0, value_bytes)).sum();
        if let Err(why) = budget.take(bytes) {
            return Ok(Err(why));
        }
        rows.push(cells(&columns, &row));
    }
    let prefixed = columns.first().is_some_and(prefixed);
    Ok(Ok((rows, prefixed)))
}

/// Whether the columns of a group count's answer, as the server describes them for this run,
/// send only values of a bounded size: the value comes with the length the server measured (the
/// second column a `BIGINT`, as `LENGTH` answers it; the value then only up to the preview's
/// size), as the statement always has it. Any other answer is refused ([`DbError::SchemaChanged`])
/// before a row of it is read.
fn bounded(columns: &[Column]) -> Result<(), DbError> {
    let [_, length, _] = columns else { return Err(DbError::NoResult) };
    if length.column_type() == ColumnType::MYSQL_TYPE_LONGLONG { Ok(()) } else { Err(DbError::SchemaChanged) }
}

/// The bytes value `v` took as it came.
fn value_bytes(v: &Value) -> usize {
    match v {
        Value::NULL => 0,
        Value::Bytes(b) => b.len(),
        _ => 8,
    }
}

/// Whether the server groups the values of result column `c` by their first `max_sort_length`
/// bytes: strings and bytes (`CHAR`, `VARCHAR`, `BINARY`, the `TEXT` and `BLOB` types), unless
/// the column says it is an `ENUM` or a `SET` (compared by their numbers).
fn prefixed(c: &Column) -> bool {
    use ColumnType::*;
    let stringy = matches!(
        c.column_type(),
        MYSQL_TYPE_STRING
            | MYSQL_TYPE_VAR_STRING
            | MYSQL_TYPE_VARCHAR
            | MYSQL_TYPE_BLOB
            | MYSQL_TYPE_TINY_BLOB
            | MYSQL_TYPE_MEDIUM_BLOB
            | MYSQL_TYPE_LONG_BLOB
    );
    stringy && !c.flags().intersects(ColumnFlags::ENUM_FLAG | ColumnFlags::SET_FLAG)
}
