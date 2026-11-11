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
//!
//! What it reads is bounded ([`rows_within`]): the answer's columns as the server describes them
//! must send only short values or values measured and cut by the server ([`bounded`]), and each
//! row is checked against the answer's budget ([`GroupBudget`]) as it comes.
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
        OwnGot::Refused(why) => Err(why),
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

/// The rows of `text` (a group count), each value as the session shows a cell of it, read one by
/// one: only when its columns say its values are bounded ([`bounded`]), and each row within the
/// answer's budget ([`GroupBudget`]). Once either fails, the rest of the answer is read and
/// dropped row by row (nothing more is kept), and why it is refused is the answer.
pub(super) async fn rows_within(
    conn: &mut Conn,
    text: &str,
) -> Result<Result<(Vec<Vec<Cell>>, bool), DbError>, mysql_async::Error> {
    let mut qr = conn.query_iter(text).await?;
    let columns = qr.columns().unwrap_or_else(|| Arc::from(Vec::new()));
    let mut refused = bounded(&columns).err();
    let mut budget = GroupBudget::default();
    let mut rows = Vec::new();
    while let Some(row) = qr.next().await? {
        if refused.is_some() {
            continue;
        }
        let bytes = (0..row.len()).map(|i| row.as_ref(i).map_or(0, value_bytes)).sum();
        match budget.take(bytes) {
            Ok(()) => rows.push(cells(&columns, &row)),
            Err(why) => {
                refused = Some(why);
                rows = Vec::new();
            }
        }
    }
    let prefixed = columns.first().is_some_and(prefixed);
    Ok(refused.map_or(Ok((rows, prefixed)), Err))
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

/// Whether the columns of a group count's answer, as the server describes them for this run,
/// send only values of a bounded size: the value comes with the length the server measured (the
/// second column a `BIGINT`, as `LENGTH` answers it; the value then only up to the preview's
/// size), or the server says no value of its column is longer than the preview (the column's
/// length in bytes). A column whose type changed since the count was written (from a number to
/// text) is refused ([`DbError::SchemaChanged`]), not read as one of its old type.
fn bounded(columns: &[Column]) -> Result<(), DbError> {
    let [value, length, _] = columns else { return Err(DbError::NoResult) };
    let measured = length.column_type() == ColumnType::MYSQL_TYPE_LONGLONG;
    if measured || value.column_length() as usize <= GROUP_PREVIEW { Ok(()) } else { Err(DbError::SchemaChanged) }
}

/// The bytes value `v` took as it came.
fn value_bytes(v: &Value) -> usize {
    match v {
        Value::NULL => 0,
        Value::Bytes(b) => b.len(),
        _ => 8,
    }
}
