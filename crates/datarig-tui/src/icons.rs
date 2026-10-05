//! Nerd Font icons of connection profiles.
//!
//! The config stores an icon by **name** (`icon = "database"`), never the glyph, so a new Nerd
//! Fonts release only changes this table. Code points follow Nerd Fonts v3 (checked against
//! `glyphnames.json` of 3.5.1). A profile without an icon, or with a name that is not in
//! [`CURATED`], shows its driver's icon. With `icons = off` the icon cell is left blank but
//! keeps its width, so names stay aligned.
//!
//! The explorer, the quick connect list and the tab labels draw them; the explorer's schema
//! tree also has an icon for each kind of node and for each column's type. The UI
//! never draws an emoji: every mark is a Nerd Font glyph with icons on and plain text (or
//! nothing) with icons off.

use datarig_core::driver::structure::StructureGroup;
use datarig_core::profile::ConnectionConfig;

/// Columns every icon takes (the glyph, then a space).
pub const WIDTH: usize = 2;

/// `(name, Nerd Fonts glyph name, glyph)` of the icons a profile can pick, in chooser order.
pub const CURATED: [(&str, &str, &str); 24] = [
    ("database", "nf-fa-database", "\u{f1c0}"),
    ("server", "nf-fa-server", "\u{f233}"),
    ("cloud", "nf-fa-cloud", "\u{f0c2}"),
    ("cube", "nf-fa-cube", "\u{f1b2}"),
    ("cubes", "nf-fa-cubes", "\u{f1b3}"),
    ("leaf", "nf-fa-leaf", "\u{f06c}"),
    ("fire", "nf-fa-fire", "\u{f06d}"),
    ("bolt", "nf-fa-bolt", "\u{f0e7}"),
    ("shield", "nf-fa-shield", "\u{f132}"),
    ("lock", "nf-fa-lock", "\u{f023}"),
    ("flask", "nf-fa-flask", "\u{f0c3}"),
    ("bug", "nf-fa-bug", "\u{f188}"),
    ("rocket", "nf-fa-rocket", "\u{f135}"),
    ("home", "nf-fa-home", "\u{f015}"),
    ("laptop", "nf-fa-laptop", "\u{f109}"),
    ("building", "nf-fa-building", "\u{f1ad}"),
    ("briefcase", "nf-fa-briefcase", "\u{f0b1}"),
    ("globe", "nf-fa-globe", "\u{f0ac}"),
    ("chart", "nf-fa-bar_chart", "\u{f080}"),
    ("archive", "nf-fa-archive", "\u{f187}"),
    ("bookmark", "nf-fa-bookmark", "\u{f02e}"),
    ("star", "nf-fa-star", "\u{f005}"),
    ("heart", "nf-fa-heart", "\u{f004}"),
    ("warning", "nf-fa-warning", "\u{f071}"),
];

/// `(driver, Nerd Fonts glyph name, glyph)`. `nf-md-elasticsearch` does not exist
/// in Nerd Fonts v3; its devicon is used instead.
pub const DRIVERS: [(&str, &str, &str); 4] = [
    ("postgres", "nf-dev-postgresql", "\u{e76e}"),
    ("mysql", "nf-dev-mysql", "\u{e704}"),
    ("redis", "nf-dev-redis", "\u{e76d}"),
    ("elasticsearch", "nf-dev-elasticsearch", "\u{e7ca}"),
];

/// Icon of a driver the table does not know.
pub const UNKNOWN_DRIVER: &str = "\u{f1c0}";

/// A saved query (nf-fa-file_code_o).
pub const SCRIPT: &str = "\u{f1c9}";

/// A table tab (nf-fa-table); with icons off the tab shows the name alone.
pub const TABLE: &str = "\u{f0ce}";

/// A DDL tab (nf-fa-code); with icons off the tab says `DDL` in words.
pub const DDL: &str = "\u{f121}";

/// A database of a server in the explorer (Font Awesome `database`).
pub const DATABASE: &str = "\u{f1c0}";

/// The previous and next page in the results title (nf-fa-caret_left, nf-fa-caret_right).
pub const PAGE_PREV: &str = "\u{f0d9}";
pub const PAGE_NEXT: &str = "\u{f0da}";

/// The previous and next page marks: the carets, or `‹` and `›` with icons off.
pub fn page_arrows(on: bool) -> (&'static str, &'static str) {
    if on { (PAGE_PREV, PAGE_NEXT) } else { ("\u{2039}", "\u{203a}") }
}

/// A profile that connects through an SSH tunnel (nf-md-ssh).
pub const TUNNEL: &str = "\u{f08c0}";

/// The tunnel mark: the glyph, or `SSH` with icons off (never an emoji).
pub fn tunnel(on: bool) -> &'static str {
    if on { TUNNEL } else { "SSH" }
}

/// A warning (nf-fa-warning): a lost connection, a read-only workspace, a tab without a
/// connection. `!` with icons off (never an emoji).
pub const WARNING: &str = "\u{f071}";

/// A hot node of a query plan: nf-md-fire.
pub const HOT: &str = "\u{f0238}";

/// The warning mark: the glyph, or `!` with icons off.
pub fn warning(on: bool) -> &'static str {
    if on { WARNING } else { "!" }
}

/// Key column marks (nf-fa-key, nf-fa-link, nf-fa-fingerprint), each followed by a space;
/// `PK`, `FK` and `UQ` with icons off.
pub const KEY_PK: &str = "\u{f084}";
pub const KEY_FK: &str = "\u{f0c1}";
pub const KEY_UQ: &str = "\u{ee40}";

/// One key a column is part of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyMark {
    Pk,
    Fk,
    Uq,
}

impl KeyMark {
    /// The glyph with icons on, the letters with icons off.
    pub fn text(self, on: bool) -> &'static str {
        match (self, on) {
            (KeyMark::Pk, true) => KEY_PK,
            (KeyMark::Fk, true) => KEY_FK,
            (KeyMark::Uq, true) => KEY_UQ,
            (KeyMark::Pk, false) => "PK",
            (KeyMark::Fk, false) => "FK",
            (KeyMark::Uq, false) => "UQ",
        }
    }
}

/// The marks of `m`, primary key first.
pub fn key_marks(m: datarig_core::driver::KeyMarks) -> Vec<KeyMark> {
    [(m.pk, KeyMark::Pk), (m.fk, KeyMark::Fk), (m.unique, KeyMark::Uq)]
        .into_iter()
        .filter_map(|(on, k)| on.then_some(k))
        .collect()
}

/// The marks of `m` as text, each followed by a space (`PK FK `, or the glyphs).
pub fn key_marks_text(m: datarig_core::driver::KeyMarks, on: bool) -> String {
    key_marks(m).into_iter().map(|k| format!("{} ", k.text(on))).collect()
}

/// A node of the explorer's schema tree. With icons off the tree has no icons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeIcon {
    Schema,
    Tables,
    Table,
    Views,
    View,
    MaterializedView,
}

/// `(node, Nerd Fonts glyph name, glyph)` of the schema tree's nodes: Material Design icons.
/// A database keeps [`DATABASE`] (Font Awesome, as the profile icon of that name).
pub const TREE: [(TreeIcon, &str, &str); 6] = [
    (TreeIcon::Schema, "nf-md-folder_table", "\u{f12e3}"),
    (TreeIcon::Tables, "nf-md-table_multiple", "\u{f13c8}"),
    (TreeIcon::Table, "nf-md-table", "\u{f04eb}"),
    (TreeIcon::Views, "nf-md-eye_outline", "\u{f06d0}"),
    (TreeIcon::View, "nf-md-table_eye", "\u{f1094}"),
    (TreeIcon::MaterializedView, "nf-md-table_refresh", "\u{f13a0}"),
];

impl TreeIcon {
    pub fn glyph(self) -> &'static str {
        TREE.iter().find(|t| t.0 == self).map_or("", |t| t.2)
    }
}

/// `(group, Nerd Fonts glyph name, glyph)` of the groups of a table's structure in the
/// explorer: Material Design icons. The items of a group have its icon (a column keeps its key
/// marks or its type's icon).
pub const STRUCTURE: [(StructureGroup, &str, &str); 7] = [
    (StructureGroup::Columns, "nf-md-table_column", "\u{f0835}"),
    (StructureGroup::PrimaryKey, "nf-md-key", "\u{f0306}"),
    (StructureGroup::ForeignKeys, "nf-md-key_link", "\u{f119f}"),
    (StructureGroup::Indexes, "nf-md-format_list_numbered", "\u{f027b}"),
    (StructureGroup::UniqueConstraints, "nf-md-fingerprint", "\u{f0237}"),
    (StructureGroup::CheckConstraints, "nf-md-checkbox_marked_outline", "\u{f0135}"),
    (StructureGroup::Triggers, "nf-md-lightning_bolt", "\u{f140b}"),
];

/// The icon of a structure group (and of its items).
pub fn structure(g: StructureGroup) -> &'static str {
    STRUCTURE.iter().find(|t| t.0 == g).map_or("", |t| t.2)
}

/// What kind of values a column holds, from its type's name: the explorer draws
/// a column that is no key with the icon of its category.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeCategory {
    Text,
    Number,
    DateTime,
    Boolean,
    Json,
    Uuid,
    Binary,
    Array,
    Other,
}

/// `(category, Nerd Fonts glyph name, glyph)`: Material Design icons.
pub const TYPES: [(TypeCategory, &str, &str); 9] = [
    (TypeCategory::Text, "nf-md-format_text", "\u{f0284}"),
    (TypeCategory::Number, "nf-md-numeric", "\u{f03a0}"),
    (TypeCategory::DateTime, "nf-md-calendar_clock", "\u{f00f0}"),
    (TypeCategory::Boolean, "nf-md-toggle_switch_outline", "\u{f0a1a}"),
    (TypeCategory::Json, "nf-md-code_json", "\u{f0626}"),
    (TypeCategory::Uuid, "nf-md-identifier", "\u{f0efe}"),
    (TypeCategory::Binary, "nf-md-hexadecimal", "\u{f12a7}"),
    (TypeCategory::Array, "nf-md-code_brackets", "\u{f016a}"),
    (TypeCategory::Other, "nf-md-shape_outline", "\u{f0832}"),
];

impl TypeCategory {
    /// The category of a type named as the server formats it (PostgreSQL's `format_type`:
    /// `character varying(20)`, `timestamp with time zone`, `interval day to second`, `integer[]`;
    /// MySQL's names too, `int unsigned`). A name it does not know (an enum, a domain, a
    /// geometric type) is `Other`.
    pub fn of(type_name: &str) -> Self {
        let t = type_name.trim().to_ascii_lowercase();
        if t.ends_with("[]") {
            return TypeCategory::Array;
        }
        // The name without its modifiers (`(20)`, `(10,2)`), quotes (`"char"`) and MySQL's
        // `unsigned` and `zerofill`.
        let base = t.split('(').next().unwrap_or("").trim().trim_matches('"');
        let base = base.trim_end_matches(" zerofill").trim_end_matches(" unsigned").trim_end_matches(" signed");
        let first = base.split_whitespace().next().unwrap_or("");
        match base {
            "text" | "character varying" | "character" | "varchar" | "char" | "bpchar" | "name" | "citext"
            | "tinytext" | "mediumtext" | "longtext" | "nvarchar" | "nchar" | "string" => TypeCategory::Text,
            "smallint" | "integer" | "bigint" | "int" | "int2" | "int4" | "int8" | "tinyint" | "mediumint"
            | "numeric" | "decimal" | "real" | "double precision" | "double" | "float" | "float4" | "float8"
            | "money" | "oid" | "serial" | "smallserial" | "bigserial" => TypeCategory::Number,
            "date" | "interval" | "datetime" | "year" => TypeCategory::DateTime,
            "boolean" | "bool" => TypeCategory::Boolean,
            "json" | "jsonb" | "jsonpath" => TypeCategory::Json,
            "uuid" => TypeCategory::Uuid,
            "bytea" | "bit" | "bit varying" | "varbit" | "blob" | "tinyblob" | "mediumblob" | "longblob" | "binary"
            | "varbinary" => TypeCategory::Binary,
            // `time`, `timetz`, `timestamp`, `timestamptz` and their `with(out) time zone` forms, an
            // `interval` with its fields (`day to second`).
            _ if matches!(first, "time" | "timetz" | "timestamp" | "timestamptz" | "interval") => {
                TypeCategory::DateTime
            }
            _ => TypeCategory::Other,
        }
    }

    pub fn glyph(self) -> &'static str {
        TYPES.iter().find(|t| t.0 == self).map_or("", |t| t.2)
    }
}

/// The glyph of a curated icon name.
pub fn by_name(name: &str) -> Option<&'static str> {
    CURATED.iter().find(|c| c.0 == name).map(|c| c.2)
}

/// The glyph of a driver's default icon (`postgresql`/`pg` count as `postgres`).
pub fn for_driver(driver: &str) -> &'static str {
    let d = match driver {
        "postgresql" | "pg" => "postgres",
        "mariadb" => "mysql",
        d => d,
    };
    DRIVERS.iter().find(|x| x.0 == d).map_or(UNKNOWN_DRIVER, |x| x.2)
}

/// The glyph of a profile: its own icon, else its driver's.
pub fn glyph(p: &ConnectionConfig) -> &'static str {
    p.icon.as_deref().and_then(by_name).unwrap_or_else(|| for_driver(&p.driver))
}

/// The icon cell of a profile, [`WIDTH`] columns wide: the glyph and a space, or blanks when
/// icons are off.
pub fn cell(p: &ConnectionConfig, on: bool) -> String {
    if on { format!("{} ", glyph(p)) } else { " ".repeat(WIDTH) }
}

/// A few glyphs for the `icons` setting's preview (does this terminal font have them?).
pub fn preview() -> String {
    [for_driver("postgres"), for_driver("mysql"), by_name("database").unwrap_or(""), by_name("rocket").unwrap_or("")]
        .join(" ")
}

#[cfg(test)]
mod tests;
