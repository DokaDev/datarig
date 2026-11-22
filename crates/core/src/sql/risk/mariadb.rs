//! MariaDB's own classifier data: the built-ins of the family it does not have, the geometry
//! constructors it reads by their arguments, how its grammar reads the calls of built-ins, the
//! words of the family's reserved list it does not reserve, and the lists that make them one
//! engine's ([`MARIADB_TABLES`]), which the family classifier ([`super::mysql`]) judges with.

use super::mysql::FamilyTables;

/// The names of [`BUILTINS`](super::mysql::BUILTINS) that are no function of MariaDB's own in a version the driver
/// connects to (10.6 on), upper case, one per line in byte order. MariaDB looks an unqualified
/// call of one up as a stored function of the current database, so on MariaDB such a call is
/// an unchecked call, as any function that is not a built-in. They are the names MariaDB 11.4
/// does not know as functions, and those MariaDB added after 10.6 (`FORMAT_PICO_TIME`,
/// `JSON_OVERLAPS`, `JSON_PRETTY`, `JSON_SCHEMA_VALID`, `RANDOM_BYTES`). A test runs each on
/// MariaDB.
pub const NOT_ON_MARIADB: &str = include_str!("mysql_builtins_not_mariadb.txt");

/// MariaDB's geometry constructors of [`NOT_ON_MARIADB`], with the fewest and the most
/// arguments its grammar reads them with: called with another number of arguments (`POINT()`),
/// the name is a stored function's there.
pub const MARIADB_CONSTRUCTORS: &[(&str, usize, usize)] = &[
    ("GEOMETRYCOLLECTION", 1, usize::MAX),
    ("LINESTRING", 1, usize::MAX),
    ("MULTILINESTRING", 1, usize::MAX),
    ("MULTIPOINT", 1, usize::MAX),
    ("MULTIPOLYGON", 1, usize::MAX),
    ("POINT", 2, 2),
    ("POLYGON", 1, usize::MAX),
];

/// [`MYSQL_TIGHT_CALLS`](super::mysql::MYSQL_TIGHT_CALLS) as MariaDB reads them: the same names less `ST_COLLECT` (no function
/// there) and `SYSDATE` (a blank is fine there), and its window functions.
pub const MARIADB_TIGHT_CALLS: &[&str] = &[
    "ADDDATE",
    "BIT_AND",
    "BIT_OR",
    "BIT_XOR",
    "CAST",
    "COUNT",
    "CUME_DIST",
    "CURDATE",
    "CURTIME",
    "DATE_ADD",
    "DATE_SUB",
    "DENSE_RANK",
    "EXTRACT",
    "FIRST_VALUE",
    "GROUP_CONCAT",
    "JSON_ARRAYAGG",
    "JSON_OBJECTAGG",
    "LAG",
    "LEAD",
    "MAX",
    "MID",
    "MIN",
    "NOW",
    "NTH_VALUE",
    "NTILE",
    "PERCENT_RANK",
    "POSITION",
    "RANK",
    "SESSION_USER",
    "STD",
    "STDDEV",
    "STDDEV_POP",
    "STDDEV_SAMP",
    "SUBDATE",
    "SUBSTR",
    "SUBSTRING",
    "SUM",
    "SYSTEM_USER",
    "TRIM",
    "VARIANCE",
    "VAR_POP",
    "VAR_SAMP",
];

/// [`MYSQL_WORD_CALLS`](super::mysql::MYSQL_WORD_CALLS) as MariaDB reads them.
pub const MARIADB_WORD_CALLS: &[&str] = &[
    "ASCII",
    "AVG",
    "CHAR",
    "CHARSET",
    "CONVERT",
    "CURRENT_DATE",
    "CURRENT_ROLE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURRENT_USER",
    "DATE",
    "DAY",
    "DEFAULT",
    "GET_FORMAT",
    "HOUR",
    "IF",
    "INSERT",
    "INTERVAL",
    "JSON_TABLE",
    "LAST_VALUE",
    "LEFT",
    "LOCALTIME",
    "LOCALTIMESTAMP",
    "MINUTE",
    "MONTH",
    "REPEAT",
    "REPLACE",
    "RIGHT",
    "ROW_NUMBER",
    "SECOND",
    "SYSDATE",
    "TIME",
    "TIMESTAMP",
    "TIMESTAMPADD",
    "TIMESTAMPDIFF",
    "TRUNCATE",
    "USER",
    "UTC_DATE",
    "UTC_TIME",
    "UTC_TIMESTAMP",
    "VALUES",
    "WEIGHT_STRING",
    "YEAR",
];

/// MariaDB's lists.
pub static MARIADB_TABLES: FamilyTables = FamilyTables {
    not_builtin: NOT_ON_MARIADB,
    constructors: MARIADB_CONSTRUCTORS,
    tight_calls: MARIADB_TIGHT_CALLS,
    word_calls: MARIADB_WORD_CALLS,
    unreserved_not_calls: MARIADB_NOT_RESERVED,
    json_table_needs_columns: true,
};

/// Words of [`NOT_CALLS`](super::mysql::NOT_CALLS) that MariaDB does not reserve: written before `(`, they name a
/// function there (a stored one, as no built-in has the name).
pub(super) const MARIADB_NOT_RESERVED: &[&str] = &["LATERAL", "OF"];
