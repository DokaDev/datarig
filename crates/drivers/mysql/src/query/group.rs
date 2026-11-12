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
//!
//! The value is sent as an expression's, which the server writes without its column's
//! attributes: each one is made the grid's text where that can be told from the length the
//! server measured of the column's own text, and taken back (not shown, never one to pick)
//! where it cannot ([`as_the_column`], [`GroupCounts::mark_inexact`]).

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
        OwnGot::Rows(GroupRows { rows, prefixed, inexact }) => GroupCounts::from_rows(rows).map(|mut counts| {
            counts.mark_inexact(&inexact);
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

/// What [`rows_within`] read.
pub(super) struct GroupRows {
    /// Each row's values as the session shows cells of them, the first as the grid shows it
    /// ([`as_the_column`]).
    pub rows: Vec<Vec<Cell>>,
    /// The server groups the values by a prefix ([`prefixed`]).
    pub prefixed: bool,
    /// For each row, whether its value's text could not be told to be the grid's
    /// ([`as_the_column`]).
    pub inexact: Vec<bool>,
}

/// The rows of `text` (a group count), each value as the session shows a cell of it, read one by
/// one: only when its columns say every value comes measured ([`bounded`]), and each row within
/// the answer's budget ([`GroupBudget`]). When either fails, reading stops there: why, with the
/// rest of the answer unread (the caller cuts the connection, so none of it is received or
/// kept).
pub(super) async fn rows_within(conn: &mut Conn, text: &str) -> Result<Result<GroupRows, DbError>, mysql_async::Error> {
    let mut qr = conn.query_iter(text).await?;
    let columns = qr.columns().unwrap_or_else(|| Arc::from(Vec::new()));
    if let Err(why) = bounded(&columns) {
        return Ok(Err(why));
    }
    // As text: the server sends at most the preview's bytes of the value in its column's
    // character set, which the connection's (`utf8mb4`) writes in at most 4 bytes for each.
    let mut budget = GroupBudget::new(4 * GROUP_PREVIEW);
    let (mut rows, mut inexact) = (Vec::new(), Vec::new());
    while let Some(row) = qr.next().await? {
        let bytes = (0..row.len()).map(|i| row.as_ref(i).map_or(0, value_bytes)).sum();
        if let Err(why) = budget.take(bytes) {
            return Ok(Err(why));
        }
        let mut cells = cells(&columns, &row);
        let told = match cells.as_mut_slice() {
            [value, length, ..] => as_the_column(&columns[0], value, length.as_deref()),
            _ => true,
        };
        rows.push(cells);
        inexact.push(!told);
    }
    let prefixed = columns.first().is_some_and(prefixed);
    Ok(Ok(GroupRows { rows, prefixed, inexact }))
}

/// Make `value`, a value of the measured expression `c` (`IF(OCTET_LENGTH(col) <= …, col, NULL)`),
/// the text the grid shows for its column, whose own text the server measured as `length`
/// bytes; whether that could be told. The server writes a column by its attributes and an
/// expression without them: a `YEAR` 0 is `0000` as a column and `0` as an expression, a
/// `ZEROFILL` number is padded with zeros to its width as a column only. Strings, bytes, `ENUM`
/// and `SET` values are sent as they are (their length is in the column's character set, not
/// the text's: not compared). Any other value (numbers, dates and times) is its column's text
/// when it is as long as the server measured it; an exact number (an integer, `YEAR`,
/// `DECIMAL`) that is shorter is padded with zeros on the left to that length, which is all a
/// `YEAR` or a `ZEROFILL` column adds; any other difference is not told (`false`). NULL is NULL.
fn as_the_column(c: &Column, value: &mut Cell, length: Option<&str>) -> bool {
    use ColumnType::*;
    let Some(v) = value.as_mut() else { return true };
    let exact = matches!(
        c.column_type(),
        MYSQL_TYPE_TINY
            | MYSQL_TYPE_SHORT
            | MYSQL_TYPE_INT24
            | MYSQL_TYPE_LONG
            | MYSQL_TYPE_LONGLONG
            | MYSQL_TYPE_YEAR
            | MYSQL_TYPE_DECIMAL
            | MYSQL_TYPE_NEWDECIMAL
    );
    let other = matches!(
        c.column_type(),
        MYSQL_TYPE_FLOAT
            | MYSQL_TYPE_DOUBLE
            | MYSQL_TYPE_DATE
            | MYSQL_TYPE_NEWDATE
            | MYSQL_TYPE_TIME
            | MYSQL_TYPE_TIME2
            | MYSQL_TYPE_DATETIME
            | MYSQL_TYPE_DATETIME2
            | MYSQL_TYPE_TIMESTAMP
            | MYSQL_TYPE_TIMESTAMP2
    );
    if !exact && !other {
        return true;
    }
    let Some(length) = length.and_then(|l| l.trim().parse::<usize>().ok()) else { return false };
    if v.len() == length {
        return true;
    }
    let number = |s: &str| {
        let (whole, fraction) = s.split_once('.').unwrap_or((s, "0"));
        !whole.is_empty() && !fraction.is_empty() && (whole.bytes().chain(fraction.bytes())).all(|b| b.is_ascii_digit())
    };
    if exact && v.len() < length && number(v) {
        *v = format!("{}{v}", "0".repeat(length - v.len()));
        return true;
    }
    false
}

/// Whether the columns of a group count's answer, as the server describes them for this run,
/// send only values of a bounded size: the value comes with the length the server measured (the
/// second column an integer, as `OCTET_LENGTH` answers it: a `BIGINT` on MySQL, an `INT` on MariaDB;
/// the value then only up to the preview's size), as the statement always has it. Any other
/// answer is refused ([`DbError::SchemaChanged`]) before a row of it is read.
fn bounded(columns: &[Column]) -> Result<(), DbError> {
    let [_, length, _] = columns else { return Err(DbError::NoResult) };
    match length.column_type() {
        ColumnType::MYSQL_TYPE_LONGLONG | ColumnType::MYSQL_TYPE_LONG => Ok(()),
        _ => Err(DbError::SchemaChanged),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// What [`as_the_column`] makes of `value` of an expression of type `ty` whose column's own
    /// text the server measured as `length` bytes: the text, and whether it was told.
    fn told(ty: ColumnType, value: Option<&str>, length: Option<&str>) -> (Option<String>, bool) {
        let mut cell = value.map(str::to_string);
        let ok = as_the_column(&Column::new(ty), &mut cell, length);
        (cell, ok)
    }

    #[test]
    fn an_answer_is_bounded_with_the_length_as_mysql_and_mariadb_type_it() {
        use ColumnType::*;
        let answer = |length: ColumnType| {
            bounded(&[Column::new(MYSQL_TYPE_VAR_STRING), Column::new(length), Column::new(MYSQL_TYPE_LONGLONG)])
        };
        // `OCTET_LENGTH` is a `BIGINT` on MySQL and an `INT` on MariaDB.
        assert!(answer(MYSQL_TYPE_LONGLONG).is_ok());
        assert!(answer(MYSQL_TYPE_LONG).is_ok());
        // Anything else is not the statement's measured length.
        for other in [MYSQL_TYPE_VAR_STRING, MYSQL_TYPE_BLOB, MYSQL_TYPE_NEWDECIMAL, MYSQL_TYPE_DOUBLE] {
            assert!(matches!(answer(other), Err(DbError::SchemaChanged)), "{other:?}");
        }
        assert!(matches!(bounded(&[Column::new(MYSQL_TYPE_LONGLONG)]), Err(DbError::NoResult)));
    }

    #[test]
    fn a_value_is_made_its_columns_text_or_not_told() {
        use ColumnType::*;
        let s = |v: &str| Some(v.to_string());
        // `YEAR` 0 and `ZEROFILL` numbers: padded to what the column writes.
        assert_eq!(told(MYSQL_TYPE_LONG, Some("0"), Some("4")), (s("0000"), true));
        assert_eq!(told(MYSQL_TYPE_YEAR, Some("0"), Some("4")), (s("0000"), true));
        assert_eq!(told(MYSQL_TYPE_LONG, Some("42"), Some("5")), (s("00042"), true));
        assert_eq!(told(MYSQL_TYPE_NEWDECIMAL, Some("1.50"), Some("7")), (s("0001.50"), true));
        // As long as the column's own text: as it is.
        assert_eq!(told(MYSQL_TYPE_LONGLONG, Some("-7"), Some("2")), (s("-7"), true));
        assert_eq!(told(MYSQL_TYPE_DOUBLE, Some("1.500"), Some("5")), (s("1.500"), true));
        assert_eq!(
            told(MYSQL_TYPE_DATETIME, Some("2026-01-02 03:04:05.100000"), Some("26")),
            (s("2026-01-02 03:04:05.100000"), true)
        );
        // Anything else is not told: a sign, a float or a date of another length, a length
        // that does not read.
        assert!(!told(MYSQL_TYPE_LONG, Some("-7"), Some("4")).1);
        assert!(!told(MYSQL_TYPE_LONG, Some("12345"), Some("4")).1);
        assert!(!told(MYSQL_TYPE_FLOAT, Some("1.5"), Some("4")).1);
        assert!(!told(MYSQL_TYPE_DATETIME, Some("2026-01-02 03:04:05"), Some("26")).1);
        assert!(!told(MYSQL_TYPE_LONG, Some("42"), None).1);
        assert!(!told(MYSQL_TYPE_NEWDECIMAL, Some("1e5"), Some("7")).1);
        // Strings and bytes are sent as they are; NULL is NULL.
        assert_eq!(told(MYSQL_TYPE_VAR_STRING, Some("é"), Some("1")), (s("é"), true));
        assert_eq!(told(MYSQL_TYPE_BLOB, Some("0x41"), Some("1")), (s("0x41"), true));
        assert_eq!(told(MYSQL_TYPE_LONG, None, None), (None, true));
    }
}
