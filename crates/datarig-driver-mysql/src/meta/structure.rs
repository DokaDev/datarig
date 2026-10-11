//! One table's structure (`DbCommand::LoadStructure`): a single `information_schema` statement
//! that builds the whole structure as one JSON document on the server (`JSON_OBJECT` of
//! `JSON_ARRAYAGG` subqueries), so opening a table's node costs one round trip. Every part is
//! read for the one table (`TABLE_SCHEMA` and `TABLE_NAME`, as literals), and the table itself is
//! never read: no `CARDINALITY` (which can open the table), no `SHOW` statement.
//!
//! Like every read of the metadata session it runs in autocommit, gives up after the session's
//! `lock_wait_timeout` behind another session's metadata lock ([`DbError::Locked`]) and holds
//! none once it has answered. It does not read the table's estimates (`TABLE_ROWS`,
//! `DATA_LENGTH`): the server opens a table to estimate it when it has no statistics of it
//! cached. The schema's listing has them ([`TableStructure::estimates`] is false).
//!
//! MySQL keeps the text of an expression (a check's condition, a generated column's, a
//! functional key's, an expression default) with one level of string escaping (`\'`, `\\`):
//! it is unescaped once ([`unescaped`]) so that it is SQL again. MariaDB keeps it as it is.
//!
//! `COLUMNS` lists only the columns the user has a privilege on (a column-level grant shows a
//! part of the table). MariaDB before 10.5 has no `JSON_ARRAYAGG`: the read fails there with the
//! server's message. With `lower_case_table_names = 2` (macOS, Windows) the names are compared
//! as the server stores them, which is not tested.
//!
//! What the server's version has not is not asked for: an index key's `EXPRESSION` (functional
//! keys, MySQL 8.0.13) and `CHECK_CONSTRAINTS` (MySQL 8.0.16). A MySQL older than 8.0.16 parses
//! a check and drops it, so a table there has none and the list is empty in fact. MariaDB keeps
//! its checks (every one enforced) and has no functional keys.
//!
//! MySQL lists a table's triggers only to a user with the `TRIGGER` privilege on it: with none
//! listed and no such grant of the user's own (`information_schema`'s privilege tables, which do
//! not show a role's), the group is unknown ([`TableStructure::hidden`]), not empty.

use crate::session::Server;
use datarig_core::driver::DbError;
use datarig_core::driver::structure::{
    CheckConstraint, ColumnFill, FkAction, ForeignKey, Index, KeyConstraint, RelationKind, StructureColumn,
    StructureGroup, TableStructure, Trigger, TriggerEvent, TriggerTiming,
};
use datarig_core::sql::dialect::{Dialect, MySqlMode};
use mysql_async::Conn;
use serde::Deserialize;

/// The statement, `{schema}` and `{table}` the table's names as literals. `{expression}` is an
/// index key's expression (or `NULL`), `{visible}` whether an index is visible (or `NULL`),
/// `{checks}` the checks' subquery (or `NULL`), `{grantee}`
/// the session's user ([`GRANTEE`]). No subquery is a derived table that names the outer table:
/// MariaDB and MySQL before 8.0.14 have no such reference.
const SQL: &str = "SELECT JSON_OBJECT(
  'kind', t.TABLE_TYPE,
  'columns', (SELECT JSON_ARRAYAGG(JSON_OBJECT('position', c.ORDINAL_POSITION, 'name', c.COLUMN_NAME,
      'type', c.COLUMN_TYPE, 'data_type', c.DATA_TYPE, 'nullable', c.IS_NULLABLE, 'default', c.COLUMN_DEFAULT,
      'extra', c.EXTRA, 'generation', c.GENERATION_EXPRESSION))
    FROM information_schema.COLUMNS c WHERE c.TABLE_SCHEMA = t.TABLE_SCHEMA AND c.TABLE_NAME = t.TABLE_NAME),
  'indexes', (SELECT JSON_ARRAYAGG(JSON_OBJECT('name', x.INDEX_NAME, 'seq', x.SEQ_IN_INDEX,
      'column', x.COLUMN_NAME, 'expression', {expression}, 'sub_part', x.SUB_PART, 'collation', x.COLLATION,
      'non_unique', x.NON_UNIQUE, 'type', x.INDEX_TYPE, 'visible', {visible}))
    FROM information_schema.STATISTICS x WHERE x.TABLE_SCHEMA = t.TABLE_SCHEMA AND x.TABLE_NAME = t.TABLE_NAME),
  'foreign_keys', (SELECT JSON_ARRAYAGG(JSON_OBJECT('name', k.CONSTRAINT_NAME, 'position', k.ORDINAL_POSITION,
      'column', k.COLUMN_NAME, 'ref_schema', k.REFERENCED_TABLE_SCHEMA, 'ref_table', k.REFERENCED_TABLE_NAME,
      'ref_column', k.REFERENCED_COLUMN_NAME, 'on_delete', r.DELETE_RULE, 'on_update', r.UPDATE_RULE))
    FROM information_schema.KEY_COLUMN_USAGE k JOIN information_schema.REFERENTIAL_CONSTRAINTS r
      ON r.CONSTRAINT_SCHEMA = k.CONSTRAINT_SCHEMA AND r.CONSTRAINT_NAME = k.CONSTRAINT_NAME
      AND r.TABLE_NAME = k.TABLE_NAME
    WHERE k.TABLE_SCHEMA = t.TABLE_SCHEMA AND k.TABLE_NAME = t.TABLE_NAME AND k.REFERENCED_TABLE_NAME IS NOT NULL),
  'checks', {checks},
  'triggers', (SELECT JSON_ARRAYAGG(JSON_OBJECT('name', g.TRIGGER_NAME, 'timing', g.ACTION_TIMING,
      'event', g.EVENT_MANIPULATION, 'orientation', g.ACTION_ORIENTATION, 'statement', g.ACTION_STATEMENT,
      'order', g.ACTION_ORDER))
    FROM information_schema.TRIGGERS g
    WHERE g.EVENT_OBJECT_SCHEMA = t.TABLE_SCHEMA AND g.EVENT_OBJECT_TABLE = t.TABLE_NAME),
  'trigger_privilege', EXISTS (SELECT 1 FROM information_schema.USER_PRIVILEGES p
      WHERE p.PRIVILEGE_TYPE = 'TRIGGER' AND p.GRANTEE = {grantee})
    OR EXISTS (SELECT 1 FROM information_schema.SCHEMA_PRIVILEGES p
      WHERE p.PRIVILEGE_TYPE = 'TRIGGER' AND p.GRANTEE = {grantee} AND t.TABLE_SCHEMA LIKE p.TABLE_SCHEMA)
    OR EXISTS (SELECT 1 FROM information_schema.TABLE_PRIVILEGES p
      WHERE p.PRIVILEGE_TYPE = 'TRIGGER' AND p.GRANTEE = {grantee}
      AND p.TABLE_SCHEMA = t.TABLE_SCHEMA AND p.TABLE_NAME = t.TABLE_NAME))
FROM information_schema.TABLES t WHERE t.TABLE_SCHEMA = {schema} AND t.TABLE_NAME = {table}";

/// The user the session is, as the privilege tables name a grantee (`'user'@'host'`): the host
/// is after the last `@` (a user name may hold one).
const GRANTEE: &str = "CONCAT('''', LEFT(CURRENT_USER(), CHAR_LENGTH(CURRENT_USER()) \
     - CHAR_LENGTH(SUBSTRING_INDEX(CURRENT_USER(), '@', -1)) - 1), \
     '''@''', SUBSTRING_INDEX(CURRENT_USER(), '@', -1), '''')";

/// The checks' subquery of MySQL 8.0.16 and later (`ENFORCED`; a check's name is the
/// database's).
const MYSQL_CHECKS: &str = "(SELECT JSON_ARRAYAGG(JSON_OBJECT('name', tc.CONSTRAINT_NAME,
      'clause', cc.CHECK_CLAUSE, 'enforced', tc.ENFORCED))
    FROM information_schema.TABLE_CONSTRAINTS tc JOIN information_schema.CHECK_CONSTRAINTS cc
      ON cc.CONSTRAINT_SCHEMA = tc.CONSTRAINT_SCHEMA AND cc.CONSTRAINT_NAME = tc.CONSTRAINT_NAME
    WHERE tc.TABLE_SCHEMA = t.TABLE_SCHEMA AND tc.TABLE_NAME = t.TABLE_NAME AND tc.CONSTRAINT_TYPE = 'CHECK')";

/// MariaDB's (a check's name is the table's; every check is enforced).
const MARIADB_CHECKS: &str = "(SELECT JSON_ARRAYAGG(JSON_OBJECT('name', cc.CONSTRAINT_NAME,
      'clause', cc.CHECK_CLAUSE, 'enforced', 'YES'))
    FROM information_schema.CHECK_CONSTRAINTS cc
    WHERE cc.CONSTRAINT_SCHEMA = t.TABLE_SCHEMA AND cc.TABLE_NAME = t.TABLE_NAME)";

/// The statement for `schema.table` on `server`, the names written as `mode` reads literals.
pub(crate) fn statement(server: Server, mode: MySqlMode, schema: &str, table: &str) -> String {
    let functional = !server.mariadb && server.version >= (8, 0, 13);
    let checks = match server.mariadb {
        true => MARIADB_CHECKS,
        false if server.version >= (8, 0, 16) => MYSQL_CHECKS,
        false => "NULL",
    };
    let d = Dialect::MySql(mode);
    SQL.replace("{expression}", if functional { "x.EXPRESSION" } else { "NULL" })
        .replace("{visible}", if server.mariadb { "NULL" } else { "x.IS_VISIBLE" })
        .replace("{checks}", checks)
        .replace("{grantee}", GRANTEE)
        .replace("{schema}", &d.quote_literal(schema))
        .replace("{table}", &d.quote_literal(table))
}

/// Read the structure of `schema.table` in one round trip: [`DbError::NotFound`] when the
/// server lists no such table (or view) to the user.
pub(crate) async fn load_structure(
    conn: &mut Conn,
    server: Server,
    mode: MySqlMode,
    schema: &str,
    table: &str,
) -> Result<TableStructure, DbError> {
    let rows: Vec<String> = super::rows_of(conn, statement(server, mode, schema, table)).await?;
    match rows.first() {
        Some(json) => parse(json, server.mariadb, mode, schema, table),
        None => Err(DbError::NotFound),
    }
}

#[derive(Deserialize)]
pub(crate) struct Raw {
    kind: String,
    columns: Option<Vec<RawColumn>>,
    indexes: Option<Vec<RawIndexPart>>,
    foreign_keys: Option<Vec<RawForeignKeyPart>>,
    checks: Option<Vec<RawCheck>>,
    triggers: Option<Vec<RawTrigger>>,
    trigger_privilege: Flag,
}

/// A boolean as a server writes it into JSON: `true`, or `1` (MariaDB).
#[derive(Deserialize)]
#[serde(untagged)]
enum Flag {
    Bool(bool),
    Int(i64),
}

impl Flag {
    fn on(&self) -> bool {
        match self {
            Flag::Bool(b) => *b,
            Flag::Int(n) => *n != 0,
        }
    }
}

#[derive(Deserialize)]
struct RawColumn {
    position: u32,
    name: String,
    #[serde(rename = "type")]
    type_name: String,
    data_type: String,
    nullable: String,
    default: Option<String>,
    extra: Option<String>,
    generation: Option<String>,
}

/// One key part of an index (`STATISTICS` has a row per part).
#[derive(Deserialize)]
struct RawIndexPart {
    name: String,
    seq: u32,
    /// `None` for a functional key part.
    column: Option<String>,
    expression: Option<String>,
    /// The length of a prefix key part.
    sub_part: Option<u32>,
    /// `D` for a descending key part.
    collation: Option<String>,
    non_unique: i64,
    #[serde(rename = "type")]
    method: String,
    /// `NO` for an invisible index (MySQL; `None` on MariaDB).
    #[serde(default)]
    visible: Option<String>,
}

/// One column of a foreign key.
#[derive(Deserialize)]
struct RawForeignKeyPart {
    name: String,
    position: u32,
    column: String,
    ref_schema: String,
    ref_table: String,
    ref_column: String,
    on_delete: String,
    on_update: String,
}

#[derive(Deserialize)]
struct RawCheck {
    name: String,
    clause: String,
    enforced: String,
}

#[derive(Deserialize)]
struct RawTrigger {
    name: String,
    timing: String,
    event: String,
    orientation: String,
    statement: String,
    /// Its place among the table's triggers of the same timing and event (1 fires first).
    #[serde(default)]
    order: u32,
}

/// The document the statement built, as the model (see [`model`]).
fn parse(json: &str, mariadb: bool, mode: MySqlMode, schema: &str, table: &str) -> Result<TableStructure, DbError> {
    let raw: Raw = serde_json::from_str(json).map_err(|e| DbError::Server(format!("table structure: {e}")))?;
    model(raw, mariadb, mode, schema, table)
}

/// The structure the statement's document describes, of `schema.table`. Lists are in the
/// server's order: columns by position, key parts by their place in the key, the rest by name.
/// A kind of table the explorer does not list is not supported; a trigger or a foreign key rule
/// the model has no words for fails the read (it is not left out).
pub(crate) fn model(
    raw: Raw,
    mariadb: bool,
    mode: MySqlMode,
    schema: &str,
    table: &str,
) -> Result<TableStructure, DbError> {
    let d = Dialect::MySql(mode);
    let kind = match raw.kind.as_str() {
        "BASE TABLE" | "SYSTEM VERSIONED" => RelationKind::Table,
        "VIEW" => RelationKind::View,
        _ => return Err(DbError::NotSupported),
    };
    let mut s = TableStructure::new(kind);
    s.estimates = false;
    // MySQL's expressions are kept escaped once (see the module's documentation).
    let sql = |text: String| if mariadb { text } else { unescaped(&text) };
    let mut columns = raw.columns.unwrap_or_default();
    columns.sort_by_key(|c| c.position);
    s.columns = columns
        .into_iter()
        .map(|mut c| {
            c.generation = c.generation.map(sql);
            let generated = c.extra.as_deref().is_some_and(|e| words(e).any(|w| w == "DEFAULT_GENERATED"));
            if generated {
                c.default = c.default.map(sql);
            }
            column(c, mariadb, d)
        })
        .collect();
    let qualified = format!("{}.{}", d.quote_ident(schema), d.quote_ident(table));

    let mut parts = raw.indexes.unwrap_or_default();
    parts.sort_by(|a, b| (&a.name, a.seq).cmp(&(&b.name, b.seq)));
    // The indexes with a prefix key part, and those that are invisible.
    let (mut prefixed, mut invisible) = (Vec::new(), Vec::new());
    for p in parts {
        if s.indexes.last().is_none_or(|x| x.name != p.name) {
            s.indexes.push(Index {
                primary: p.name == "PRIMARY",
                name: p.name.clone(),
                columns: Vec::new(),
                options: Vec::new(),
                include: Vec::new(),
                key_columns: Vec::new(),
                include_columns: Vec::new(),
                unique: p.non_unique == 0,
                method: p.method.clone(),
                predicate: None,
                constraint: false,
                definition: String::new(),
            });
        }
        let Some(x) = s.indexes.last_mut() else { continue };
        if p.visible.as_deref() == Some("NO") && !invisible.contains(&p.name) {
            invisible.push(p.name.clone());
        }
        // A spatial key has no prefix of its own, whatever `SUB_PART` says (32 on MySQL 8.4).
        let sub_part = p.sub_part.filter(|_| p.method != "SPATIAL");
        if sub_part.is_some() && !prefixed.contains(&p.name) {
            prefixed.push(p.name.clone());
        }
        let text = match (&p.column, p.expression) {
            (Some(c), _) => match sub_part {
                Some(n) => format!("{}({n})", d.quote_ident(c)),
                None => d.quote_ident(c),
            },
            (None, Some(e)) => format!("({})", sql(e)),
            (None, None) => {
                return Err(DbError::Server(format!("table structure: index {} has a key part of nothing", p.name)));
            }
        };
        x.columns.push(text);
        x.options.push(if p.collation.as_deref() == Some("D") { "DESC".into() } else { String::new() });
        x.key_columns.push(p.column);
    }
    for x in &mut s.indexes {
        let keys = x.keys().join(", ");
        // Whole columns: no expression, no prefix.
        let on_columns = x.key_columns.iter().all(Option::is_some) && !prefixed.contains(&x.name);
        if x.primary {
            x.constraint = true;
            x.definition = format!("ALTER TABLE {qualified} ADD PRIMARY KEY ({keys})");
            let columns = x.key_columns.iter().flatten().cloned().collect();
            s.primary_key =
                Some(KeyConstraint { name: x.name.clone(), columns, definition: format!("PRIMARY KEY ({keys})") });
            continue;
        }
        let what = match x.method.as_str() {
            "FULLTEXT" => "FULLTEXT ",
            "SPATIAL" => "SPATIAL ",
            _ if x.unique => "UNIQUE ",
            _ => "",
        };
        let using = if x.method == "HASH" { " USING HASH" } else { "" };
        let hidden = if invisible.contains(&x.name) { " INVISIBLE" } else { "" };
        x.definition = format!("CREATE {what}INDEX {} ON {qualified} ({keys}){using}{hidden}", d.quote_ident(&x.name));
        // A unique key on whole columns is the table's unique constraint; one on an expression
        // or a prefix only an index (only the prefix is unique).
        if x.unique && on_columns {
            x.constraint = true;
            s.unique_constraints.push(KeyConstraint {
                name: x.name.clone(),
                columns: x.key_columns.iter().flatten().cloned().collect(),
                definition: format!("UNIQUE KEY {} ({keys})", d.quote_ident(&x.name)),
            });
        }
    }

    let mut fk_parts = raw.foreign_keys.unwrap_or_default();
    fk_parts.sort_by(|a, b| (&a.name, a.position).cmp(&(&b.name, b.position)));
    for p in fk_parts {
        if s.foreign_keys.last().is_none_or(|f| f.name != p.name) {
            s.foreign_keys.push(ForeignKey {
                name: p.name.clone(),
                columns: Vec::new(),
                ref_schema: p.ref_schema.clone(),
                ref_table: p.ref_table.clone(),
                ref_columns: Vec::new(),
                on_delete: fk_action(&p.name, &p.on_delete)?,
                on_update: fk_action(&p.name, &p.on_update)?,
                definition: String::new(),
            });
        }
        if let Some(f) = s.foreign_keys.last_mut() {
            f.columns.push(p.column);
            f.ref_columns.push(p.ref_column);
        }
    }
    for f in &mut s.foreign_keys {
        let list = |cols: &[String]| cols.iter().map(|c| d.quote_ident(c)).collect::<Vec<_>>().join(", ");
        let mut def = format!(
            "CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {}.{} ({})",
            d.quote_ident(&f.name),
            list(&f.columns),
            d.quote_ident(&f.ref_schema),
            d.quote_ident(&f.ref_table),
            list(&f.ref_columns)
        );
        for (what, action) in [("ON DELETE", f.on_delete), ("ON UPDATE", f.on_update)] {
            if action != FkAction::NoAction {
                def.push_str(&format!(" {what} {}", action.sql()));
            }
        }
        f.definition = def;
    }

    let mut checks = raw.checks.unwrap_or_default();
    checks.sort_by(|a, b| a.name.cmp(&b.name));
    for c in checks {
        let expression = unwrapped(&sql(c.clause)).to_string();
        // A column's name is not case sensitive: the condition may write it otherwise.
        let names = quoted_names(&expression);
        let columns = s
            .columns
            .iter()
            .filter(|col| names.iter().any(|n| n.eq_ignore_ascii_case(&col.name)))
            .map(|col| col.name.clone())
            .collect();
        let mut definition = format!("CONSTRAINT {} CHECK ({expression})", d.quote_ident(&c.name));
        if c.enforced == "NO" {
            definition.push(' ');
            definition.push_str(CheckConstraint::MYSQL_NOT_ENFORCED);
        }
        s.checks.push(CheckConstraint { name: c.name, expression, columns, definition });
    }

    // In the order they fire: by timing and event, then their order among those.
    let mut triggers = raw.triggers.unwrap_or_default();
    let rank = |t: &RawTrigger| {
        let timing = ["BEFORE", "AFTER"].iter().position(|w| *w == t.timing);
        let event = ["INSERT", "UPDATE", "DELETE"].iter().position(|w| *w == t.event);
        (timing, event, t.order)
    };
    triggers.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.name.cmp(&b.name)));
    for t in triggers {
        let timing = match t.timing.as_str() {
            "BEFORE" => TriggerTiming::Before,
            "AFTER" => TriggerTiming::After,
            other => return Err(DbError::Server(format!("table structure: trigger {} fires {other}", t.name))),
        };
        let event = match t.event.as_str() {
            "INSERT" => TriggerEvent::Insert,
            "UPDATE" => TriggerEvent::Update,
            "DELETE" => TriggerEvent::Delete,
            other => return Err(DbError::Server(format!("table structure: trigger {} fires on {other}", t.name))),
        };
        let for_each_row = t.orientation == "ROW";
        let definition = format!(
            "CREATE TRIGGER {} {} {} ON {qualified} FOR EACH {} {}",
            d.quote_ident(&t.name),
            timing.sql(),
            event.sql(),
            if for_each_row { "ROW" } else { "STATEMENT" },
            t.statement
        );
        s.triggers.push(Trigger {
            name: t.name,
            timing,
            events: vec![event],
            for_each_row,
            function: String::new(),
            enabled: true,
            update_columns: Vec::new(),
            condition: None,
            definition,
        });
    }
    if kind == RelationKind::Table && s.triggers.is_empty() && !raw.trigger_privilege.on() {
        s.hidden.push(StructureGroup::Triggers);
    }
    Ok(s)
}

/// Column `c`: how it is filled (generated, `AUTO_INCREMENT`, its default) from `EXTRA`.
fn column(c: RawColumn, mariadb: bool, d: Dialect) -> StructureColumn {
    let extra = c.extra.unwrap_or_default();
    let has = |word: &str| extra.split_whitespace().any(|w| w.eq_ignore_ascii_case(word));
    // MySQL puts some expressions in parentheses (`(`qty` * 2)`), others not: the model has
    // them without, as a check's condition.
    let generation = unwrapped(&c.generation.unwrap_or_default()).to_string();
    let fill = if has("STORED") && has("GENERATED") {
        ColumnFill::Stored(generation)
    } else if has("VIRTUAL") && has("GENERATED") {
        ColumnFill::Virtual(generation)
    } else if has("auto_increment") {
        ColumnFill::AutoIncrement
    } else {
        ColumnFill::Default
    };
    let default = match fill {
        ColumnFill::Default => default_text(c.default, &c.data_type, &extra, mariadb, d),
        _ => None,
    };
    StructureColumn { name: c.name, type_name: c.type_name, not_null: c.nullable == "NO", default, fill }
}

/// A column's default as SQL writes it, with what an `UPDATE` sets the column to
/// (`… ON UPDATE CURRENT_TIMESTAMP`); `None` when it has none.
///
/// MySQL's `COLUMN_DEFAULT` is the value, unquoted: a literal is quoted here unless it is a
/// number (or a bit or hexadecimal value, written as such); an expression (`DEFAULT_GENERATED`)
/// is written in its parentheses, but `CURRENT_TIMESTAMP`, which a temporal column takes bare.
/// MariaDB's is already SQL (`'text'`, `current_timestamp()`, `NULL` for none).
fn default_text(value: Option<String>, data_type: &str, extra: &str, mariadb: bool, d: Dialect) -> Option<String> {
    let current = |v: &str| v.to_ascii_uppercase().starts_with("CURRENT_TIMESTAMP");
    let value = match value {
        None => None,
        Some(v) if mariadb => (!v.eq_ignore_ascii_case("NULL")).then_some(v),
        Some(v) if words(extra).any(|w| w == "DEFAULT_GENERATED") && !current(&v) => Some(format!("({v})")),
        Some(v) if current(&v) && matches!(data_type, "timestamp" | "datetime") => Some(v),
        Some(v) if is_number_type(data_type) => Some(v),
        Some(v) if data_type == "bit" && v.starts_with("b'") => Some(v),
        Some(v) if v.starts_with("0x") && is_binary_type(data_type) => Some(v),
        Some(v) => Some(d.quote_literal(&v)),
    };
    // `EXTRA` lists other words too (`DEFAULT_GENERATED`, `INVISIBLE`): only the word after
    // `on update` is the timestamp an `UPDATE` sets.
    let mut w = words(extra);
    let on_update = loop {
        match w.next() {
            Some(a) if a.eq_ignore_ascii_case("on") => {
                if w.next().is_some_and(|b| b.eq_ignore_ascii_case("update")) {
                    break w.next();
                }
            }
            Some(_) => {}
            None => break None,
        }
    };
    match on_update {
        Some(t) => Some(format!("{} ON UPDATE {t}", value.as_deref().unwrap_or("NULL"))),
        None => value,
    }
}

fn is_number_type(t: &str) -> bool {
    matches!(t, "tinyint" | "smallint" | "mediumint" | "int" | "bigint" | "decimal" | "float" | "double")
}

fn is_binary_type(t: &str) -> bool {
    matches!(t, "binary" | "varbinary" | "tinyblob" | "blob" | "mediumblob" | "longblob")
}

/// The words of `EXTRA`.
fn words(extra: &str) -> impl Iterator<Item = &str> {
    extra.split_whitespace()
}

/// `REFERENTIAL_CONSTRAINTS.DELETE_RULE` / `UPDATE_RULE` of foreign key `name`: a rule the model
/// has no words for fails the read.
fn fk_action(name: &str, rule: &str) -> Result<FkAction, DbError> {
    Ok(match rule {
        "NO ACTION" => FkAction::NoAction,
        "RESTRICT" => FkAction::Restrict,
        "CASCADE" => FkAction::Cascade,
        "SET NULL" => FkAction::SetNull,
        "SET DEFAULT" => FkAction::SetDefault,
        other => return Err(DbError::Server(format!("table structure: foreign key {name} has the rule {other}"))),
    })
}

/// `text` with one level of MySQL's string escaping undone (`\'` is `'`, `\\` is `\`, `\n` a
/// line break, …), as MySQL keeps an expression's text in `information_schema`. A backslash
/// before any other character stays as it is.
fn unescaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('0') => out.push('\0'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('b') => out.push('\u{8}'),
            Some('Z') => out.push('\u{1a}'),
            Some(e @ ('\\' | '\'' | '"')) => out.push(e),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// `clause` without the parentheses MySQL puts around a check's condition (`(`qty` > 0)`):
/// only when the first one closes at its end, outside quotes (a backslash in a string literal
/// escapes the character after it).
fn unwrapped(clause: &str) -> &str {
    let t = clause.trim();
    if !t.starts_with('(') || !t.ends_with(')') {
        return t;
    }
    let (mut quote, mut depth, mut escaped) = (None, 0usize, false);
    for (i, b) in t.bytes().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, b) {
            (Some(b'\'' | b'"'), b'\\') => escaped = true,
            (Some(q), _) if b == q => quote = None,
            (Some(_), _) => {}
            (None, b'`' | b'\'' | b'"') => quote = Some(b),
            (None, b'(') => depth += 1,
            (None, b')') => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return if i == t.len() - 1 { &t[1..i] } else { t };
                }
            }
            _ => {}
        }
    }
    t
}

/// The names `text` quotes with backticks (a doubled backtick inside is one), outside string
/// literals: the columns a check's condition reads, as the server prints it.
fn quoted_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    let mut literal = None;
    while let Some(c) = chars.next() {
        match (literal, c) {
            (Some(q), _) if c == q => literal = None,
            (Some(_), '\\') => {
                chars.next();
            }
            (Some(_), _) => {}
            (None, '\'' | '"') => literal = Some(c),
            (None, '`') => {
                let mut name = String::new();
                while let Some(n) = chars.next() {
                    if n == '`' {
                        if chars.peek() == Some(&'`') {
                            chars.next();
                            name.push('`');
                            continue;
                        }
                        break;
                    }
                    name.push(n);
                }
                out.push(name);
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests;
