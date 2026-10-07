//! An object's DDL as SQL text, from what the catalog has of it ([`DdlSource`]): the statements
//! that would create it again, in the order `pg_dump` writes them (sequences its columns own,
//! the `CREATE`, then its owner, indexes, triggers, row-level security, comments and grants).
//! PostgreSQL syntax: the catalog it comes from is PostgreSQL's.
//!
//! Names the catalog gives unquoted are written with [`sql_ident`] and strings with
//! [`quote_literal`]; types, expressions and the server's own definitions (an index's, a
//! constraint's, a view's query) are written as the server printed them.

use crate::driver::ddl::{
    ColumnDdl, DdlSource, FunctionDdl, Grant, IndexDdl, Policy, RelationDdl, ReplicaIdentity, SequenceDdl, TriggerDdl,
    TriggerMode,
};
use crate::driver::structure::{ColumnFill, RelationKind};
use crate::export::quote_literal;
use crate::sql::ident::sql_ident;

/// The first line of every DDL text.
pub const HEADER: &str = "-- Reconstructed by datarig from the catalog (not pg_dump)";

/// What a relation's DDL leaves out, said under the header.
const NOT_INCLUDED: &str = "-- Not included: rows, sequence values, rules, extended statistics, security labels";

/// `ddl` as SQL text: the header, then each statement followed by a blank line. The server's own
/// text ([`DdlSource::Verbatim`]) is the text as it is, without the header: it is not
/// reconstructed, so nothing is to be said about it.
pub fn ddl_text(ddl: &DdlSource) -> String {
    let (notes, statements) = match ddl {
        DdlSource::Verbatim { text, .. } => return text.clone(),
        DdlSource::Relation(r) => (Some(NOT_INCLUDED), relation(r)),
        DdlSource::Index(i) => (None, index(i)),
        DdlSource::Trigger(t) => (None, trigger(t)),
        DdlSource::Function(f) => (None, function(f)),
    };
    let mut out = String::from(HEADER);
    out.push('\n');
    if let Some(n) = notes {
        out.push_str(n);
        out.push('\n');
    }
    for s in statements {
        out.push('\n');
        out.push_str(&s);
        out.push('\n');
    }
    out
}

/// `schema.name`, each part quoted when it needs it.
fn qualified(schema: &str, name: &str) -> String {
    format!("{}.{}", sql_ident(schema), sql_ident(name))
}

/// `name=value` of a storage parameter or an option list as SQL writes it: the value as it is
/// when it is a plain word or number, else as a string.
fn option(raw: &str, prefix: &str) -> String {
    let (name, value) = raw.split_once('=').unwrap_or((raw, ""));
    let plain =
        !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '+'));
    let value = if plain { value.to_string() } else { quote_literal(value) };
    format!("{prefix}{name}={value}")
}

/// A foreign table's option: `name 'value'`.
fn fdw_option(raw: &str) -> String {
    let (name, value) = raw.split_once('=').unwrap_or((raw, ""));
    format!("{} {}", sql_ident(name), quote_literal(value))
}

/// The keyword of a relation of `kind` in `ALTER …` and `COMMENT ON …`.
fn relation_word(kind: RelationKind) -> &'static str {
    match kind {
        RelationKind::Table | RelationKind::PartitionedTable => "TABLE",
        RelationKind::ForeignTable => "FOREIGN TABLE",
        RelationKind::View => "VIEW",
        RelationKind::MaterializedView => "MATERIALIZED VIEW",
    }
}

/// The smallest and largest value of a sequence's type.
fn type_range(type_name: &str) -> (i64, i64) {
    match type_name {
        "smallint" => (i16::MIN.into(), i16::MAX.into()),
        "integer" => (i32::MIN.into(), i32::MAX.into()),
        _ => (i64::MIN, i64::MAX),
    }
}

/// A sequence's options that are not the defaults of its type and direction, as `CREATE
/// SEQUENCE` writes them; with its type first when `with_type` and it is not `bigint`.
fn sequence_options(s: &SequenceDdl, with_type: bool) -> Vec<String> {
    let (lo, hi) = type_range(&s.type_name);
    let up = s.increment > 0;
    let (min, max) = if up { (1, hi) } else { (lo, -1) };
    let mut v = Vec::new();
    if with_type && s.type_name != "bigint" {
        v.push(format!("AS {}", s.type_name));
    }
    if s.increment != 1 {
        v.push(format!("INCREMENT BY {}", s.increment));
    }
    if s.min != min {
        v.push(format!("MINVALUE {}", s.min));
    }
    if s.max != max {
        v.push(format!("MAXVALUE {}", s.max));
    }
    if s.start != if up { s.min } else { s.max } {
        v.push(format!("START WITH {}", s.start));
    }
    if s.cache != 1 {
        v.push(format!("CACHE {}", s.cache));
    }
    if s.cycle {
        v.push("CYCLE".to_string());
    }
    v
}

/// One column of `CREATE TABLE`: its name, type, collation, how it is filled and `NOT NULL`.
fn column_line(r: &RelationDdl, i: usize) -> String {
    let c = &r.structure.columns[i];
    let extra = r.columns.get(i);
    let mut s = format!("{} {}", sql_ident(&c.name), c.type_name);
    if let Some(opts) = extra.map(|e| &e.fdw_options).filter(|o| !o.is_empty()) {
        let opts: Vec<String> = opts.iter().map(|o| fdw_option(o)).collect();
        s.push_str(&format!(" OPTIONS ({})", opts.join(", ")));
    }
    if let Some((schema, name)) = extra.and_then(|e| e.collation.as_ref()) {
        s.push_str(&format!(" COLLATE {}", qualified(schema, name)));
    }
    let identity = |always: bool| {
        let word = if always { "ALWAYS" } else { "BY DEFAULT" };
        let mut opts = Vec::new();
        if let Some(seq) = extra.and_then(|e| e.identity.as_ref()) {
            if seq.schema != r.schema || seq.name != format!("{}_{}_seq", r.name, c.name) {
                opts.push(format!("SEQUENCE NAME {}", qualified(&seq.schema, &seq.name)));
            }
            opts.extend(sequence_options(seq, false));
        }
        match opts.is_empty() {
            true => format!(" GENERATED {word} AS IDENTITY"),
            false => format!(" GENERATED {word} AS IDENTITY ({})", opts.join(" ")),
        }
    };
    match &c.fill {
        ColumnFill::Default => {
            if let Some(d) = &c.default {
                s.push_str(&format!(" DEFAULT {d}"));
            }
        }
        ColumnFill::Stored(e) => s.push_str(&format!(" GENERATED ALWAYS AS ({e}) STORED")),
        ColumnFill::Virtual(e) => s.push_str(&format!(" GENERATED ALWAYS AS ({e}) VIRTUAL")),
        ColumnFill::IdentityAlways => s.push_str(&identity(true)),
        ColumnFill::IdentityByDefault => s.push_str(&identity(false)),
    }
    // A named `NOT NULL` constraint is a line of its own.
    if c.not_null && !r.not_null_constraints.iter().any(|(_, col, _)| *col == c.name) {
        s.push_str(" NOT NULL");
    }
    s
}

/// A typed table's line for a column with a default or `NOT NULL` of its own: `name WITH
/// OPTIONS …`; `None` for a column that is the type's as it is.
fn typed_column_line(r: &RelationDdl, i: usize) -> Option<String> {
    let c = &r.structure.columns[i];
    let mut s = String::new();
    if let (ColumnFill::Default, Some(d)) = (&c.fill, &c.default) {
        s.push_str(&format!(" DEFAULT {d}"));
    }
    if c.not_null && !r.not_null_constraints.iter().any(|(_, col, _)| *col == c.name) {
        s.push_str(" NOT NULL");
    }
    (!s.is_empty()).then(|| format!("{} WITH OPTIONS{s}", sql_ident(&c.name)))
}

/// The table's own constraints (not inherited ones), as `CONSTRAINT name definition`: the
/// primary key, then the unique, check, exclusion and foreign key constraints, each by name.
fn constraint_lines(r: &RelationDdl) -> Vec<String> {
    let s = &r.structure;
    let own = |name: &str| !r.inherited_constraints.iter().any(|n| n == name);
    let line = |name: &str, def: &str| format!("CONSTRAINT {} {def}", sql_ident(name));
    let mut v = Vec::new();
    if let Some(pk) = s.primary_key.as_ref().filter(|k| own(&k.name)) {
        v.push(line(&pk.name, &pk.definition));
    }
    v.extend(s.unique_constraints.iter().filter(|k| own(&k.name)).map(|k| line(&k.name, &k.definition)));
    v.extend(s.checks.iter().filter(|k| own(&k.name)).map(|k| line(&k.name, &k.definition)));
    v.extend(r.not_null_constraints.iter().filter(|(n, _, _)| own(n)).map(|(n, _, d)| line(n, d)));
    v.extend(r.exclusions.iter().filter(|(n, _)| own(n)).map(|(n, d)| line(n, d)));
    v.extend(s.foreign_keys.iter().filter(|k| own(&k.name)).map(|k| line(&k.name, &k.definition)));
    v
}

/// `GRANT …` statements of `grants` on `object` (`TABLE shop.users`), one per grantee: the
/// privileges in the order they come, those with the grant option apart; `column` makes them
/// a column's (`SELECT (email)`).
fn grant_lines(grants: &[Grant], object: &str, column: Option<&str>) -> Vec<String> {
    let mut groups: Vec<(&Option<String>, bool, Vec<String>)> = Vec::new();
    for g in grants {
        let privilege = match column {
            Some(c) => format!("{} ({})", g.privilege, sql_ident(c)),
            None => g.privilege.clone(),
        };
        match groups.iter_mut().find(|(who, opt, _)| **who == g.grantee && *opt == g.grantable) {
            Some((_, _, list)) => list.push(privilege),
            None => groups.push((&g.grantee, g.grantable, vec![privilege])),
        }
    }
    groups
        .into_iter()
        .map(|(who, grantable, list)| {
            let who = who.as_deref().map_or_else(|| "PUBLIC".to_string(), sql_ident);
            let option = if grantable { " WITH GRANT OPTION" } else { "" };
            format!("GRANT {} ON {object} TO {who}{option};", list.join(", "))
        })
        .collect()
}

/// `CREATE POLICY …` of policy `p` on `table`.
fn policy_line(p: &Policy, table: &str) -> String {
    let mut s = format!("CREATE POLICY {} ON {table}", sql_ident(&p.name));
    if !p.permissive {
        s.push_str(" AS RESTRICTIVE");
    }
    if let Some(c) = &p.command {
        s.push_str(&format!(" FOR {c}"));
    }
    if p.roles.iter().any(Option::is_some) {
        let roles: Vec<String> =
            p.roles.iter().map(|r| r.as_deref().map_or_else(|| "PUBLIC".to_string(), sql_ident)).collect();
        s.push_str(&format!(" TO {}", roles.join(", ")));
    }
    if let Some(u) = &p.using {
        s.push_str(&format!(" USING ({u})"));
    }
    if let Some(c) = &p.check {
        s.push_str(&format!(" WITH CHECK ({c})"));
    }
    s.push(';');
    s
}

/// `COMMENT ON … IS '…';`
fn comment_line(what: &str, text: &str) -> String {
    format!("COMMENT ON {what} IS {};", quote_literal(text))
}

/// A view's or a materialized view's query without the `;` the server ends it with.
fn view_query(def: &str) -> &str {
    def.trim_end().trim_end_matches(';').trim_end()
}

/// The `WITH (…)` of a relation's storage parameters and its TOAST table's.
fn with_options(r: &RelationDdl) -> Option<String> {
    let all: Vec<String> =
        r.options.iter().map(|o| option(o, "")).chain(r.toast_options.iter().map(|o| option(o, "toast."))).collect();
    (!all.is_empty()).then(|| format!("WITH ({})", all.join(", ")))
}

fn relation(r: &RelationDdl) -> Vec<String> {
    let kind = r.structure.kind;
    let name = qualified(&r.schema, &r.name);
    let word = relation_word(kind);
    let mut out = Vec::new();
    // Sequences the columns own come first: a column's default names its sequence.
    for o in &r.sequences {
        let opts = sequence_options(&o.sequence, true);
        let seq = qualified(&o.sequence.schema, &o.sequence.name);
        let create = if o.sequence.unlogged { "CREATE UNLOGGED SEQUENCE" } else { "CREATE SEQUENCE" };
        out.push(match opts.is_empty() {
            true => format!("{create} {seq};"),
            false => format!("{create} {seq}\n    {};", opts.join("\n    ")),
        });
    }
    out.push(match kind {
        RelationKind::View => create_view(r, &name),
        RelationKind::MaterializedView => create_materialized_view(r, &name),
        _ => create_table(r, &name),
    });
    // View columns can have defaults (for `INSERT` through the view).
    if kind == RelationKind::View {
        for c in r.structure.columns.iter() {
            if let Some(d) = &c.default {
                out.push(format!("ALTER VIEW {name} ALTER COLUMN {} SET DEFAULT {d};", sql_ident(&c.name)));
            }
        }
    }
    // What a column that comes from a parent has of its own.
    let created = r.partition_of.is_none() && r.of_type.is_none();
    for (c, col) in r.structure.columns.iter().zip(&r.columns) {
        if created && col.local {
            continue;
        }
        let alter = format!("ALTER TABLE ONLY {name} ALTER COLUMN {}", sql_ident(&c.name));
        if col.own_default {
            out.push(match (&c.fill, &c.default) {
                (ColumnFill::Default, Some(d)) => format!("{alter} SET DEFAULT {d};"),
                _ => format!("{alter} DROP DEFAULT;"),
            });
        }
        if col.own_not_null {
            out.push(format!("{alter} SET NOT NULL;"));
        }
    }
    for c in &r.columns {
        out.extend(column_settings(&name, c));
    }
    for o in &r.sequences {
        let seq = qualified(&o.sequence.schema, &o.sequence.name);
        out.push(format!("ALTER SEQUENCE {seq} OWNED BY {name}.{};", sql_ident(&o.column)));
    }
    out.push(format!("ALTER {word} {name} OWNER TO {};", sql_ident(&r.owner)));
    for x in &r.structure.indexes {
        if !x.constraint && !r.inherited_indexes.contains(&x.name) {
            out.push(format!("{};", x.definition));
        }
    }
    for t in &r.structure.triggers {
        let extra = r.triggers.iter().find(|e| e.name == t.name);
        if extra.is_some_and(|e| e.inherited) {
            continue;
        }
        out.push(format!("{};", t.definition));
        if let Some(m) = extra.map(|e| e.mode).filter(|m| *m != TriggerMode::Origin) {
            out.push(trigger_mode_line(&name, &t.name, m));
        }
    }
    match &r.replica_identity {
        ReplicaIdentity::Default => {}
        ReplicaIdentity::Nothing => out.push(format!("ALTER TABLE ONLY {name} REPLICA IDENTITY NOTHING;")),
        ReplicaIdentity::Full => out.push(format!("ALTER TABLE ONLY {name} REPLICA IDENTITY FULL;")),
        ReplicaIdentity::Index(i) => {
            out.push(format!("ALTER TABLE ONLY {name} REPLICA IDENTITY USING INDEX {};", sql_ident(i)))
        }
    }
    if r.row_security {
        out.push(format!("ALTER TABLE {name} ENABLE ROW LEVEL SECURITY;"));
    }
    if r.force_row_security {
        out.push(format!("ALTER TABLE {name} FORCE ROW LEVEL SECURITY;"));
    }
    for p in &r.policies {
        out.push(policy_line(p, &name));
    }
    if let Some(c) = &r.comment {
        out.push(comment_line(&format!("{word} {name}"), c));
    }
    for c in &r.columns {
        if let Some(text) = &c.comment {
            out.push(comment_line(&format!("COLUMN {name}.{}", sql_ident(&c.name)), text));
        }
    }
    for c in &r.constraint_comments {
        out.push(comment_line(&format!("CONSTRAINT {} ON {name}", sql_ident(&c.name)), &c.text));
    }
    for c in &r.index_comments {
        out.push(comment_line(&format!("INDEX {}", qualified(&r.schema, &c.name)), &c.text));
    }
    for t in &r.triggers {
        if let (Some(text), false) = (&t.comment, t.inherited) {
            out.push(comment_line(&format!("TRIGGER {} ON {name}", sql_ident(&t.name)), text));
        }
    }
    for p in &r.policies {
        if let Some(text) = &p.comment {
            out.push(comment_line(&format!("POLICY {} ON {name}", sql_ident(&p.name)), text));
        }
    }
    let object = format!("TABLE {name}");
    out.extend(grant_lines(&r.grants, &object, None));
    for c in &r.columns {
        out.extend(grant_lines(&c.grants, &object, Some(&c.name)));
    }
    out
}

/// A column's settings `CREATE TABLE` has no clause for: its storage, compression, statistics
/// target and options.
fn column_settings(table: &str, c: &ColumnDdl) -> Vec<String> {
    let alter = format!("ALTER TABLE ONLY {table} ALTER COLUMN {}", sql_ident(&c.name));
    let mut v = Vec::new();
    if let Some(s) = &c.storage {
        v.push(format!("{alter} SET STORAGE {s};"));
    }
    if let Some(m) = &c.compression {
        v.push(format!("{alter} SET COMPRESSION {m};"));
    }
    if let Some(n) = c.statistics {
        v.push(format!("{alter} SET STATISTICS {n};"));
    }
    if !c.options.is_empty() {
        let opts: Vec<String> = c.options.iter().map(|o| option(o, "")).collect();
        v.push(format!("{alter} SET ({});", opts.join(", ")));
    }
    v
}

fn create_table(r: &RelationDdl, name: &str) -> String {
    let kind = r.structure.kind;
    let mut s = String::from("CREATE ");
    if r.unlogged {
        s.push_str("UNLOGGED ");
    }
    s.push_str(if kind == RelationKind::ForeignTable { "FOREIGN TABLE " } else { "TABLE " });
    s.push_str(name);
    let constraints = constraint_lines(r);
    match &r.partition_of {
        // A typed table's columns are its type's: their defaults and its constraints.
        None if r.of_type.is_some() => {
            s.push_str(&format!(" OF {}", r.of_type.as_deref().unwrap_or_default()));
            let mut lines: Vec<String> =
                (0..r.structure.columns.len()).filter_map(|i| typed_column_line(r, i)).collect();
            lines.extend(constraints);
            if !lines.is_empty() {
                s.push_str(&format!(" (\n    {}\n)", lines.join(",\n    ")));
            }
        }
        // A partition's columns are its parent's: only its own constraints.
        Some(p) => {
            s.push_str(&format!(" PARTITION OF {}", qualified(&p.schema, &p.name)));
            if !constraints.is_empty() {
                s.push_str(&format!(" (\n    {}\n)", constraints.join(",\n    ")));
            }
            s.push_str(&format!(" {}", p.bound));
        }
        None => {
            // An inherited-only column comes with `INHERITS`.
            let mut lines: Vec<String> = (0..r.structure.columns.len())
                .filter(|&i| r.columns.get(i).is_none_or(|c| c.local))
                .map(|i| column_line(r, i))
                .collect();
            lines.extend(constraints);
            match lines.is_empty() {
                true => s.push_str(" ()"),
                false => s.push_str(&format!(" (\n    {}\n)", lines.join(",\n    "))),
            }
            if !r.inherits.is_empty() {
                let parents: Vec<String> = r.inherits.iter().map(|(sc, n)| qualified(sc, n)).collect();
                s.push_str(&format!("\nINHERITS ({})", parents.join(", ")));
            }
        }
    }
    if let Some(k) = &r.partition_key {
        s.push_str(&format!("\nPARTITION BY {k}"));
    }
    if let Some(am) = &r.access_method {
        s.push_str(&format!("\nUSING {}", sql_ident(am)));
    }
    if let Some(f) = &r.foreign {
        s.push_str(&format!("\nSERVER {}", sql_ident(&f.server)));
        if !f.options.is_empty() {
            let opts: Vec<String> = f.options.iter().map(|o| fdw_option(o)).collect();
            s.push_str(&format!("\nOPTIONS ({})", opts.join(", ")));
        }
    }
    if let Some(w) = with_options(r) {
        s.push_str(&format!("\n{w}"));
    }
    if let Some(t) = &r.tablespace {
        s.push_str(&format!("\nTABLESPACE {}", sql_ident(t)));
    }
    s.push(';');
    s
}

fn create_view(r: &RelationDdl, name: &str) -> String {
    let mut s = format!("CREATE OR REPLACE VIEW {name}");
    if let Some(w) = with_options(r) {
        s.push_str(&format!(" {w}"));
    }
    let query = r.view_definition.as_deref().map(view_query).unwrap_or_default();
    format!("{s} AS\n{query};")
}

fn create_materialized_view(r: &RelationDdl, name: &str) -> String {
    let mut s = format!("CREATE MATERIALIZED VIEW {name}");
    if let Some(am) = &r.access_method {
        s.push_str(&format!("\nUSING {}", sql_ident(am)));
    }
    if let Some(w) = with_options(r) {
        s.push_str(&format!("\n{w}"));
    }
    if let Some(t) = &r.tablespace {
        s.push_str(&format!("\nTABLESPACE {}", sql_ident(t)));
    }
    let query = r.view_definition.as_deref().map(view_query).unwrap_or_default();
    format!("{s} AS\n{query}\nWITH NO DATA;")
}

/// `ALTER TABLE … DISABLE TRIGGER …` (or `ENABLE REPLICA` / `ALWAYS`) of a trigger whose mode
/// is not the default.
fn trigger_mode_line(table: &str, trigger: &str, mode: TriggerMode) -> String {
    let what = match mode {
        TriggerMode::Origin => "ENABLE",
        TriggerMode::Disabled => "DISABLE",
        TriggerMode::Replica => "ENABLE REPLICA",
        TriggerMode::Always => "ENABLE ALWAYS",
    };
    format!("ALTER TABLE {table} {what} TRIGGER {};", sql_ident(trigger))
}

fn index(i: &IndexDdl) -> Vec<String> {
    let table = qualified(&i.schema, &i.table);
    let mut out = Vec::new();
    match &i.constraint {
        // An index that backs a constraint is created by the constraint.
        Some((name, def)) => {
            out.push(format!("ALTER TABLE ONLY {table} ADD CONSTRAINT {} {def};", sql_ident(name)));
            if let Some(c) = &i.comment {
                out.push(comment_line(&format!("CONSTRAINT {} ON {table}", sql_ident(name)), c));
            }
        }
        None => {
            out.push(format!("{};", i.definition));
            if let Some(c) = &i.comment {
                out.push(comment_line(&format!("INDEX {}", qualified(&i.schema, &i.name)), c));
            }
        }
    }
    out
}

fn trigger(t: &TriggerDdl) -> Vec<String> {
    let table = qualified(&t.schema, &t.table);
    let mut out = vec![format!("{};", t.definition)];
    if t.mode != TriggerMode::Origin {
        out.push(trigger_mode_line(&table, &t.name, t.mode));
    }
    if let Some(c) = &t.comment {
        out.push(comment_line(&format!("TRIGGER {} ON {table}", sql_ident(&t.name)), c));
    }
    out
}

fn function(f: &FunctionDdl) -> Vec<String> {
    let word = if f.procedure { "PROCEDURE" } else { "FUNCTION" };
    let signature = format!("{}({})", qualified(&f.schema, &f.name), f.arguments);
    let mut out = vec![format!("{};", f.definition.trim_end())];
    out.push(format!("ALTER {word} {signature} OWNER TO {};", sql_ident(&f.owner)));
    if let Some(c) = &f.comment {
        out.push(comment_line(&format!("{word} {signature}"), c));
    }
    // Privileges other than the defaults: `PUBLIC` may lose `EXECUTE` (which a new function
    // has), and others get theirs.
    if let Some(grants) = &f.grants {
        let public_execute = |g: &Grant| g.grantee.is_none() && g.privilege == "EXECUTE" && !g.grantable;
        if !grants.iter().any(public_execute) {
            out.push(format!("REVOKE ALL ON {word} {signature} FROM PUBLIC;"));
        }
        let rest: Vec<Grant> = grants.iter().filter(|g| !public_execute(g)).cloned().collect();
        out.extend(grant_lines(&rest, &format!("{word} {signature}"), None));
    }
    out
}

#[cfg(test)]
mod tests;
