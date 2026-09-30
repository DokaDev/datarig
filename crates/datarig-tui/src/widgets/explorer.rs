//! Drawing the explorer: "＋ New connection" pinned on
//! top, folders, each profile with its state (`○` not connected, a spinner while connecting,
//! `●` connected, `✕` failed) in the profile's color, its icon and name (`RO` after it when its
//! policy is read-only), the error line under a
//! failed profile, and under a connected one its server's databases (its own first, marked
//! `default`), each with its schema tree. With icons on every node of the tree has
//! an icon of its kind, and a column that is no key the icon of its type. A table or a
//! materialized view has the estimates of its rows and size after its name, dim and on the
//! right, as many of them as there is room for (the name first). An open table shows
//! its structure (see `widgets::tree`): each group with its icon, each item with its group's.

use crate::app::explorer::{Row, RowKind};
use crate::app::{App, Keys, NodeState};
use crate::icons::{self, KeyMark, TreeIcon, TypeCategory};
use crate::text::{Align, clip, fit, human_bytes, human_count, width, wrap_words};
use crate::theme;
use crate::widgets::tree::{Group, Node, ObjectView, Structure, Tree};
use crate::widgets::{put, spinner_at};
use datarig_core::driver::KeyMarks;
use datarig_core::driver::structure::{
    ColumnFill, FkAction, ItemColumn, RelationStats, StructureColumn, StructureGroup, TableStructure, TriggerEvent,
};
use datarig_core::i18n::{Label, Msg};
use datarig_core::sql::complete::Catalog;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

/// State marks of a profile node.
pub const DISCONNECTED: &str = "○";
pub const CONNECTED: &str = "●";
pub const FAILED: &str = "✕";

/// One drawn row: `(text, style)` parts after the indentation and the arrow.
fn row_parts(app: &App, row: &Row) -> Vec<(String, Style)> {
    let fg = |c: Color| Style::new().fg(c);
    match &row.kind {
        RowKind::NewConnection => {
            vec![(format!("＋ {}", app.i18n.label(Label::ExplorerNewConnection)), fg(theme::ACCENT))]
        }
        RowKind::Folder(f) => vec![(f.name().to_string(), fg(theme::FG_MUTED).add_modifier(Modifier::BOLD))],
        RowKind::Profile(id) => {
            let Some(p) = app.profile(*id) else { return Vec::new() };
            let color = theme::profile_color(p.display_color());
            let state = app.conns.state(*id);
            let mark = match state {
                NodeState::Disconnected => (DISCONNECTED.to_string(), fg(color)),
                NodeState::Connecting => {
                    let started = app.conns.get(*id).and_then(|c| c.connecting.as_ref()).map(|c| c.started);
                    (started.map_or("⠋", |t| spinner_at(t, app.now())).to_string(), fg(theme::ACCENT))
                }
                NodeState::Connected => (CONNECTED.to_string(), fg(color)),
                NodeState::Failed => (FAILED.to_string(), fg(theme::ERROR)),
            };
            let mut name = fg(theme::FG);
            if state == NodeState::Connected {
                name = name.add_modifier(Modifier::BOLD);
            }
            let mut parts = vec![
                mark,
                (" ".into(), fg(theme::FG)),
                (icons::cell(p, app.icons_on()), fg(color)),
                (p.name.clone(), name),
            ];
            // Through an SSH tunnel: a small mark.
            if p.tunnel().is_some() {
                parts.push((format!(" {}", icons::tunnel(app.icons_on())), fg(theme::FG_MUTED)));
            }
            // A read-only policy, in words (policies have no color).
            if app.read_only(*id) {
                let ro = app.i18n.label(Label::ExplorerReadOnly);
                parts.push((format!(" {ro}"), fg(theme::FG_MUTED).add_modifier(Modifier::BOLD)));
            }
            parts
        }
        RowKind::ProfileError(id) => {
            let text = app.conns.get(*id).and_then(|c| c.error.as_ref()).map(|e| e.render(&app.i18n).to_string());
            vec![(text.unwrap_or_default(), fg(theme::ERROR))]
        }
        RowKind::Node(id, n) => match (app.conns.get(*id), n) {
            (Some(c), Node::Column(i, g, j, k)) => column_parts(app, (&c.tree, &c.catalog, &c.keys), (*i, *g, *j), *k),
            (Some(c), Node::NoColumns(..)) => {
                let text = match &c.catalog_error {
                    Some(e) => app.i18n.msg(&Msg::TreeColumnsUnreadable { error: e.clone() }),
                    None => app.i18n.label(Label::TreeLoading),
                };
                vec![(text.to_string(), fg(theme::FG_DIM).add_modifier(Modifier::ITALIC))]
            }
            (Some(c), n) if is_structure(*n) => structure_parts(app, &c.tree, *n),
            (Some(c), _) => node_parts(app, &c.tree, *n),
            (None, _) => Vec::new(),
        },
        // The server's databases, the profile's own first and marked.
        RowKind::Database(id, db) => {
            let name = db.clone().unwrap_or_else(|| app.own_database(*id));
            let icon = if app.icons_on() { format!("{} ", icons::DATABASE) } else { String::new() };
            let mut parts = vec![(icon, fg(theme::ACCENT)), (name, fg(theme::FG))];
            if db.is_none() {
                parts.push((format!(" {}", app.i18n.label(Label::ExplorerDatabaseDefault)), fg(theme::FG_DIM)));
            }
            parts
        }
        RowKind::DatabasesNote(id) => match app.conns.get(*id).and_then(|c| c.databases.as_ref()) {
            Some(Err(e)) => vec![(
                app.i18n.msg(&Msg::ExplorerDatabasesUnreadable { error: e.clone() }).to_string(),
                fg(theme::ERROR),
            )],
            _ => {
                vec![(app.i18n.label(Label::TreeLoading).to_string(), fg(theme::FG_DIM).add_modifier(Modifier::ITALIC))]
            }
        },
        RowKind::DatabaseNote(id, db) => {
            let error = match app.conns.aux(*id, db).and_then(|a| a.schemas.clone()) {
                Some(Err(e)) => e,
                _ => String::new(),
            };
            vec![(app.i18n.msg(&Msg::ExplorerDatabaseUnreadable { error }).to_string(), fg(theme::ERROR))]
        }
        RowKind::AuxNode(id, db, n) => match (app.conns.aux(*id, db), n) {
            (Some(a), Node::Column(i, g, j, k)) => column_parts(app, (&a.tree, &a.catalog, &a.keys), (*i, *g, *j), *k),
            (Some(_), Node::NoColumns(..)) | (None, _) => {
                vec![(app.i18n.label(Label::TreeLoading).to_string(), fg(theme::FG_DIM).add_modifier(Modifier::ITALIC))]
            }
            (Some(a), n) if is_structure(*n) => structure_parts(app, &a.tree, *n),
            (Some(a), _) => node_parts(app, &a.tree, *n),
        },
        RowKind::ScriptsHeader => {
            let n = app.script_list.iter().filter(|e| !e.folder).count() as u64;
            vec![
                (app.i18n.label(Label::ExplorerScripts).to_string(), fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
                (format!(" {n}"), fg(theme::FG_DIM)),
            ]
        }
        RowKind::ScriptFolder(p) => {
            let mut parts = vec![(
                datarig_core::scripts::display_name(p, true).to_string(),
                fg(theme::FG_MUTED).add_modifier(Modifier::BOLD),
            )];
            if app.script_unreadable(p) {
                parts.push((format!(" {}", app.i18n.label(Label::ScriptTreeUnreadable)), fg(theme::WARNING)));
            }
            parts
        }
        RowKind::Script(p) => {
            let icon = if app.icons_on() { format!("{} ", icons::SCRIPT) } else { String::new() };
            let open = app.tabs.find_script(p).is_some();
            let name = if open { fg(theme::FG).add_modifier(Modifier::BOLD) } else { fg(theme::FG) };
            vec![(icon, fg(theme::ACCENT_WARM)), (datarig_core::scripts::display_name(p, false).to_string(), name)]
        }
        RowKind::ScriptsEmpty => vec![(app.i18n.label(Label::ExplorerScriptsEmpty).to_string(), fg(theme::FG_DIM))],
    }
}

/// A node of a schema tree: its icon with icons on, then its label.
fn node_parts(app: &App, tree: &Tree, n: Node) -> Vec<(String, Style)> {
    let icon = match n {
        _ if !app.icons_on() => None,
        Node::Schema(_) => Some(TreeIcon::Schema),
        Node::Group(_, Group::Tables) => Some(TreeIcon::Tables),
        Node::Group(_, Group::Views) => Some(TreeIcon::Views),
        Node::Object(_, Group::Tables, _) => Some(TreeIcon::Table),
        Node::Object(i, Group::Views, j) if tree.is_materialized(i, j) => Some(TreeIcon::MaterializedView),
        Node::Object(_, Group::Views, _) => Some(TreeIcon::View),
        _ => None,
    };
    let label = tree.label(n, &app.i18n);
    match icon {
        // A group's icon is as muted as its name; a schema's and an object's in the accent, as
        // a database's.
        Some(i) => {
            let color = if matches!(n, Node::Group(..)) { theme::FG_MUTED } else { theme::ACCENT };
            vec![(format!("{} ", i.glyph()), Style::new().fg(color)), label]
        }
        None => vec![label],
    }
}

/// A node of an open object's structure.
fn is_structure(n: Node) -> bool {
    matches!(
        n,
        Node::StructGroup(..)
            | Node::StructItem(..)
            | Node::StructDetail(..)
            | Node::StructColumns(..)
            | Node::StructColumn(..)
            | Node::StructNote(..)
    )
}

fn group_label(g: StructureGroup) -> Label {
    match g {
        StructureGroup::Columns => Label::TreeGroupColumns,
        StructureGroup::PrimaryKey => Label::TreeGroupPrimaryKey,
        StructureGroup::ForeignKeys => Label::TreeGroupForeignKeys,
        StructureGroup::Indexes => Label::TreeGroupIndexes,
        StructureGroup::UniqueConstraints => Label::TreeGroupUniqueConstraints,
        StructureGroup::CheckConstraints => Label::TreeGroupCheckConstraints,
        StructureGroup::Triggers => Label::TreeGroupTriggers,
    }
}

/// The color of a structure group's icon: a key's as its column mark, the others as muted as a
/// group's name.
fn group_icon_style(g: StructureGroup) -> Style {
    let color = match g {
        StructureGroup::PrimaryKey => theme::key_color(KeyMark::Pk),
        StructureGroup::ForeignKeys => theme::key_color(KeyMark::Fk),
        StructureGroup::UniqueConstraints => theme::key_color(KeyMark::Uq),
        _ => theme::FG_MUTED,
    };
    Style::new().fg(color)
}

/// A node of an open object's structure: a group (`Columns (7)`; an empty one dim, without a
/// count), an item with what it is in dim text, an item's line, an item's `Columns (2)` and
/// each of them, or why the structure is not there.
fn structure_parts(app: &App, tree: &Tree, n: Node) -> Vec<(String, Style)> {
    let dim = Style::new().fg(theme::FG_DIM);
    let (Node::StructGroup(i, g, j, _)
    | Node::StructItem(i, g, j, _, _)
    | Node::StructDetail(i, g, j, _, _, _)
    | Node::StructColumns(i, g, j, _, _)
    | Node::StructColumn(i, g, j, _, _, _)
    | Node::StructNote(i, g, j)) = n
    else {
        return Vec::new();
    };
    let view = tree.object_view(i, g, j);
    let Some(st) = view.and_then(ObjectView::loaded) else {
        return match view.map(|v| &v.structure) {
            Some(Structure::Failed(error)) => {
                let text = app.i18n.msg(&Msg::TreeStructureUnreadable { error: error.clone() });
                vec![(text.to_string(), Style::new().fg(theme::ERROR))]
            }
            _ => vec![(app.i18n.label(Label::TreeLoading).to_string(), dim.add_modifier(Modifier::ITALIC))],
        };
    };
    let on = app.icons_on();
    match n {
        Node::StructGroup(.., sg) => {
            let count = st.count(sg);
            let mut parts = Vec::new();
            if on {
                let style = if count == 0 { dim } else { group_icon_style(sg) };
                parts.push((format!("{} ", icons::structure(sg)), style));
            }
            let label = app.i18n.label(group_label(sg)).to_string();
            if count == 0 {
                parts.push((label, dim));
            } else {
                parts.push((label, Style::new().fg(theme::FG_MUTED)));
                // A table has one primary key at most: no count.
                if sg != StructureGroup::PrimaryKey {
                    parts.push((format!(" ({count})"), dim));
                }
            }
            parts
        }
        Node::StructItem(.., sg, k) => item_parts(app, st, sg, k),
        // As the table's Columns group, with its icon.
        Node::StructColumns(.., sg, k) => {
            let count = st.item_columns(sg, k).len();
            let mut parts = Vec::new();
            if on {
                parts.push((
                    format!("{} ", icons::structure(StructureGroup::Columns)),
                    Style::new().fg(theme::FG_MUTED),
                ));
            }
            parts.push((app.i18n.label(Label::TreeGroupColumns).to_string(), Style::new().fg(theme::FG_MUTED)));
            parts.push((format!(" ({count})"), dim));
            parts
        }
        Node::StructColumn(.., sg, k, m) => {
            let Some(c) = st.item_columns(sg, k).into_iter().nth(m) else { return Vec::new() };
            let schema = tree.object_name(i, g, j).map(|(s, _)| s).unwrap_or_default();
            item_column_parts(st, sg, k, &c, &schema, on)
        }
        Node::StructDetail(.., StructureGroup::ForeignKeys, k, _) => {
            let Some(f) = st.foreign_keys.get(k) else { return Vec::new() };
            let mut text =
                format!("{} → {}.{}({})", f.columns.join(", "), f.ref_schema, f.ref_table, f.ref_columns.join(", "));
            // `NO ACTION`, the default, is left out.
            for (what, action) in [("ON DELETE", f.on_delete), ("ON UPDATE", f.on_update)] {
                if action != FkAction::NoAction {
                    text.push_str(&format!(" · {what} {}", action.sql()));
                }
            }
            vec![(text, Style::new().fg(theme::FG_MUTED))]
        }
        Node::StructDetail(.., StructureGroup::Triggers, k, _) => {
            let Some(c) = st.triggers.get(k).and_then(|t| t.condition.as_ref()) else { return Vec::new() };
            vec![(format!("WHEN ({c})"), Style::new().fg(theme::FG_MUTED))]
        }
        _ => Vec::new(),
    }
}

/// Item `k` of group `sg`: a column with its key marks (or its type's icon) and `type, not
/// null, default …`; anything else with its group's icon, its name and what it is.
fn item_parts(app: &App, st: &TableStructure, sg: StructureGroup, k: usize) -> Vec<(String, Style)> {
    let on = app.icons_on();
    let dim = Style::new().fg(theme::FG_DIM);
    let name = |n: &str| (n.to_string(), Style::new().fg(theme::FG));
    let icon = if on { vec![(format!("{} ", icons::structure(sg)), group_icon_style(sg))] } else { Vec::new() };
    let mut parts = match sg {
        StructureGroup::Columns => {
            let Some(c) = st.columns.get(k) else { return Vec::new() };
            let mut parts = mark_parts(st.marks(&c.name), &c.type_name, on);
            parts.push(name(&c.name));
            parts.push((format!("  {}", column_detail(c)), dim));
            return parts;
        }
        StructureGroup::PrimaryKey => {
            let Some(p) = &st.primary_key else { return Vec::new() };
            vec![name(&p.name)]
        }
        StructureGroup::ForeignKeys => {
            let Some(f) = st.foreign_keys.get(k) else { return Vec::new() };
            vec![name(&f.name), (format!("  → {}.{}", f.ref_schema, f.ref_table), dim)]
        }
        StructureGroup::Indexes => {
            let Some(x) = st.indexes.get(k) else { return Vec::new() };
            let mut detail = format!("({})", x.keys().join(", "));
            if !x.include.is_empty() {
                detail.push_str(&format!(" INCLUDE ({})", x.include.join(", ")));
            }
            if x.unique {
                detail.push_str(" UNIQUE");
            }
            detail.push_str(&format!(" {}", x.method));
            if let Some(p) = &x.predicate {
                detail.push_str(&format!(" WHERE {p}"));
            }
            let backs = if x.primary {
                Some(Label::TreeIndexPrimary)
            } else if x.constraint {
                Some(Label::TreeIndexConstraint)
            } else {
                None
            };
            if let Some(l) = backs {
                detail.push_str(&format!(" · {}", app.i18n.label(l)));
            }
            vec![name(&x.name), (format!("  {detail}"), dim)]
        }
        StructureGroup::UniqueConstraints => {
            let Some(u) = st.unique_constraints.get(k) else { return Vec::new() };
            vec![name(&u.name), (format!("  ({})", u.columns.join(", ")), dim)]
        }
        StructureGroup::CheckConstraints => {
            let Some(c) = st.checks.get(k) else { return Vec::new() };
            vec![name(&c.name), (format!("  {}", c.expression), dim)]
        }
        StructureGroup::Triggers => {
            let Some(t) = st.triggers.get(k) else { return Vec::new() };
            let events: Vec<String> = t
                .events
                .iter()
                .map(|e| match e {
                    TriggerEvent::Update if !t.update_columns.is_empty() => {
                        format!("UPDATE OF {}", t.update_columns.join(", "))
                    }
                    e => e.sql().to_string(),
                })
                .collect();
            let each = if t.for_each_row { "FOR EACH ROW" } else { "FOR EACH STATEMENT" };
            // Its `WHEN` condition has a line of its own, under it.
            let detail = format!("  {} {} · {each} · {}()", t.timing.sql(), events.join(" OR "), t.function);
            let mut parts = vec![name(&t.name), (detail, dim)];
            if !t.enabled {
                let off = app.i18n.label(Label::TreeTriggerDisabled);
                parts.push((format!(" · {off}"), Style::new().fg(theme::WARNING)));
            }
            parts
        }
    };
    let mut out = icon;
    out.append(&mut parts);
    out
}

/// What a column is, as its line says after its name: `bigint, not null, default …`.
fn column_detail(c: &StructureColumn) -> String {
    let mut detail = vec![c.type_name.clone()];
    if c.not_null {
        detail.push("not null".into());
    }
    if let Some(d) = &c.default {
        detail.push(format!("default {d}"));
    }
    match &c.fill {
        ColumnFill::Default => {}
        ColumnFill::Stored(e) => detail.push(format!("generated ({e})")),
        ColumnFill::Virtual(e) => detail.push(format!("generated ({e}) virtual")),
        ColumnFill::IdentityAlways => detail.push("identity always".into()),
        ColumnFill::IdentityByDefault => detail.push("identity".into()),
    }
    detail.join(", ")
}

/// Column `c` of item `k` of group `sg` (in schema `schema`): as the table's Columns group
/// shows it (its key marks or its type's icon, its name, what it is), a foreign key's with the
/// column it references (its schema left out when it is the table's), an index key's with its
/// options, an `INCLUDE` column marked; an expression's key as its text, marked.
fn item_column_parts(
    st: &TableStructure,
    sg: StructureGroup,
    k: usize,
    c: &ItemColumn,
    schema: &str,
    on: bool,
) -> Vec<(String, Style)> {
    let dim = Style::new().fg(theme::FG_DIM);
    let table_column = c.column.as_deref().and_then(|n| st.column(n));
    let mut parts = match table_column {
        Some(col) => mark_parts(st.marks(&col.name), &col.type_name, on),
        None if on => vec![(format!("{} ", TypeCategory::Other.glyph()), dim)],
        None => Vec::new(),
    };
    parts.push((c.text.clone(), Style::new().fg(theme::FG)));
    if let (Some(to), Some(f)) = (&c.references, st.foreign_keys.get(k).filter(|_| sg == StructureGroup::ForeignKeys)) {
        let table =
            if f.ref_schema == schema { f.ref_table.clone() } else { format!("{}.{}", f.ref_schema, f.ref_table) };
        parts.push((format!(" → {table}.{to}"), Style::new().fg(theme::FG_MUTED)));
    }
    let mut detail = match (table_column, &c.column) {
        (Some(col), _) => column_detail(col),
        (None, None) => "expression".to_string(),
        (None, Some(_)) => String::new(),
    };
    for extra in [c.options.as_str(), if c.include { "include" } else { "" }] {
        if !extra.is_empty() {
            detail.push_str(&format!("{}{extra}", if detail.is_empty() { "" } else { " · " }));
        }
    }
    if !detail.is_empty() {
        parts.push((format!("  {detail}"), dim));
    }
    parts
}

/// A column's key marks (PK/FK/UQ) or, with icons on, the icon of its type's category when it
/// is no key.
fn mark_parts(marks: KeyMarks, type_name: &str, on: bool) -> Vec<(String, Style)> {
    let mut parts: Vec<(String, Style)> = icons::key_marks(marks)
        .into_iter()
        .map(|m| (format!("{} ", m.text(on)), Style::new().fg(theme::key_color(m))))
        .collect();
    if on && parts.is_empty() {
        parts.push((format!("{} ", TypeCategory::of(type_name).glyph()), Style::new().fg(theme::FG_DIM)));
    }
    parts
}

/// A column of an open table: its key marks (PK/FK/UQ, from the profile's key cache) or, with
/// icons on, the icon of its type's category when it is no key, its name and its
/// type.
fn column_parts(
    app: &App,
    (tree, catalog, keys): (&Tree, &Catalog, &Keys),
    (i, g, j): (usize, Group, usize),
    k: usize,
) -> Vec<(String, Style)> {
    let Some((schema, table)) = tree.object_name(i, g, j) else { return Vec::new() };
    let Some(col) =
        catalog.relations.iter().find(|r| r.schema == schema && r.name == table).and_then(|r| r.columns.get(k))
    else {
        return Vec::new();
    };
    let marks = keys.catalog().map(|keys| keys.marks_by_name(&schema, &table, &col.name)).unwrap_or_default();
    let mut parts = mark_parts(marks, &col.type_name, app.icons_on());
    parts.push((col.name.clone(), Style::new().fg(theme::FG)));
    parts.push((format!("  {}", col.type_name), Style::new().fg(theme::FG_DIM)));
    parts
}

/// The tree `row` is a node of, and the node.
fn row_node<'a>(app: &'a App, row: &Row) -> Option<(&'a Tree, Node)> {
    match &row.kind {
        RowKind::Node(id, n) => Some((&app.conns.get(*id)?.tree, *n)),
        RowKind::AuxNode(id, db, n) => Some((&app.conns.aux(*id, db)?.tree, *n)),
        _ => None,
    }
}

/// The estimates of the object `row` is, when it has storage and they were read.
fn row_stats(app: &App, row: &Row) -> Option<RelationStats> {
    match row_node(app, row)? {
        (tree, Node::Object(i, g, j)) => tree.object_stats(i, g, j),
        _ => None,
    }
}

/// The estimates as an object's line shows them, the longest first: the rows and the size,
/// then the rows alone (a short line drops the size first); the size alone when the rows are
/// not known; nothing when neither is (no statistics yet: the status bar says so).
fn inline_stats(app: &App, s: RelationStats) -> Vec<String> {
    let rows = s.rows.map(|r| app.i18n.msg(&Msg::TreeStatsRows { rows: human_count(r) }).to_string());
    match (rows, s.bytes.map(human_bytes)) {
        (Some(r), Some(size)) => vec![format!("{r} · {size}"), r],
        (Some(r), None) => vec![r],
        (None, Some(size)) => vec![format!("~{size}")],
        (None, None) => Vec::new(),
    }
}

/// The estimates in full words, for the status bar: both are estimates from the server's
/// statistics (`VACUUM`, `ANALYZE`).
fn full_stats(app: &App, s: RelationStats) -> String {
    match (s.rows, s.bytes.map(human_bytes)) {
        (Some(r), Some(size)) => app.i18n.msg(&Msg::TreeStats { rows: human_count(r), size }).to_string(),
        (None, Some(size)) => app.i18n.msg(&Msg::TreeStatsRowsUnknown { size }).to_string(),
        (_, None) => app.i18n.label(Label::TreeStatsUnknown).to_string(),
    }
}

/// Blanks at least between an object's name and its estimates.
const STATS_GAP: usize = 2;

/// The whole line of `row` as the explorer draws it, not cut at its width: the indentation, the
/// arrow and the text (an object's estimates two blanks after its name).
pub(crate) fn row_text(app: &App, row: &Row) -> String {
    let arrow = match app.explorer_arrow(row) {
        Some(true) => "▾ ",
        Some(false) => "▸ ",
        None => "  ",
    };
    let mut text: String = row_parts(app, row).into_iter().map(|(t, _)| t).collect();
    if let Some(stats) = row_stats(app, row).and_then(|s| inline_stats(app, s).into_iter().next()) {
        text.push_str(&format!("{}{stats}", " ".repeat(STATS_GAP)));
    }
    format!("{}{arrow}{text}", "  ".repeat(row.depth))
}

/// The whole text of the row under the explorer's cursor, for the status bar, when it is part
/// of a table's structure (the explorer is narrow, and cuts the details of deep lines) or an
/// object with storage: its name and its estimates in words, also when it has none yet. A
/// structure that could not be read shows why alone (the reason and what to do), which the
/// explorer's line wraps in "structure unavailable".
pub(crate) fn line_preview(app: &App) -> Option<String> {
    let row = app.explorer_row()?;
    let (tree, n) = row_node(app, &row)?;
    if let (Node::Object(..), Some(stats)) = (n, row_stats(app, &row)) {
        return Some(format!("{}{}{}", tree.label(n, &app.i18n).0, " ".repeat(STATS_GAP), full_stats(app, stats)));
    }
    if !is_structure(n) {
        return None;
    }
    if let Node::StructNote(i, g, j) = n
        && let Some(Structure::Failed(error)) = tree.object_view(i, g, j).map(|v| &v.structure)
    {
        return Some(error.clone());
    }
    let text: String = row_parts(app, &row).into_iter().map(|(t, _)| t).collect();
    Some(text.trim().to_string()).filter(|t| !t.is_empty())
}

/// Background of a row: the cursor's row is highlighted (dimmer without the focus).
fn row_bg(selected: bool, focused: bool) -> Color {
    match (selected, focused) {
        (true, true) => theme::SELECTION_BG,
        (true, false) => theme::CURSOR_LINE_BG,
        _ => theme::BG,
    }
}

/// Draw one row on the one-line `line`.
fn draw_row(app: &App, row: &Row, line: Rect, bg: Color, buf: &mut Buffer) {
    let (x, y, w) = (line.x, line.y, line.width as usize);
    buf.set_stringn(x, y, fit("", w, Align::Left), w, Style::new().bg(bg));
    let indent = row.depth * 2;
    let arrow = match app.explorer_arrow(row) {
        Some(true) => "▾ ",
        Some(false) => "▸ ",
        None => "  ",
    };
    let mut cx = indent;
    if cx < w {
        put(buf, x + cx as u16, y, arrow, w - cx, Style::new().fg(theme::FG_MUTED).bg(bg));
    }
    cx += 2;
    for (text, style) in row_parts(app, row) {
        if cx >= w {
            break;
        }
        let used = put(buf, x + cx as u16, y, &text, w - cx, style.bg(bg));
        cx += used as usize;
    }
    // An object's estimates on the right, in the room its name leaves: never over the name.
    let room = w.saturating_sub(cx + STATS_GAP);
    let stats = row_stats(app, row).map(|s| inline_stats(app, s)).unwrap_or_default();
    if let Some(text) = stats.into_iter().find(|t| width(t) <= room) {
        let at = w - width(&text);
        put(buf, x + at as u16, y, &text, width(&text), Style::new().fg(theme::FG_DIM).bg(bg));
    }
}

/// The explorer inside `area`; returns the hardware cursor while the filter is typed.
pub(crate) fn draw_explorer(app: &mut App, area: Rect, buf: &mut Buffer, focused: bool) -> Option<(u16, u16)> {
    let rows = app.explorer_rows();
    let sel = app.explorer.index(&rows);
    let w = area.width as usize;
    let mut y = area.y;
    let bottom = area.y + area.height;
    let mut cursor = None;
    // The `/` filter line while it is typed or set.
    let filter_shown = app.explorer.filtering || !app.explorer.filter.text().is_empty();
    if filter_shown && area.height > 2 {
        let style = Style::new().fg(theme::FG).bg(theme::SURFACE);
        buf.set_style(Rect::new(area.x, y, area.width, 1), style);
        put(buf, area.x, y, "/", 1, Style::new().fg(theme::ACCENT).bg(theme::SURFACE).add_modifier(Modifier::BOLD));
        let input = Rect::new(area.x + 2, y, area.width.saturating_sub(2), 1);
        let filtering = app.explorer.filtering;
        let cx = app.explorer.filter.render(input, buf, style, filtering, false, None);
        if filtering {
            cursor = Some((cx, y));
        }
        y += 1;
    }
    if y >= bottom {
        return cursor;
    }
    // The first row is pinned; the others scroll below it.
    let first = rows.first().cloned();
    if let Some(r) = &first {
        draw_row(app, r, Rect::new(area.x, y, area.width, 1), row_bg(sel == 0, focused), buf);
    }
    let list = Rect::new(area.x, y, area.width, bottom - y);
    app.explorer.area = list;
    y += 1;
    let h = (bottom - y) as usize;
    let n = rows.len().saturating_sub(1);
    if sel > 0 && !app.explorer.detached {
        let s = sel - 1;
        if s < app.explorer.scroll {
            app.explorer.scroll = s;
        } else if h > 0 && s >= app.explorer.scroll + h {
            app.explorer.scroll = s + 1 - h;
        }
    }
    app.explorer.scroll = app.explorer.scroll.min(n.saturating_sub(h));
    let scroll = app.explorer.scroll;
    for (i, r) in rows.iter().enumerate().skip(1 + scroll).take(h) {
        let ry = y + (i - 1 - scroll) as u16;
        draw_row(app, r, Rect::new(area.x, ry, area.width, 1), row_bg(i == sel, focused), buf);
    }
    // Empty states: no profile at all, or none matching the filter.
    if n == 0 && h > 1 {
        let (title, hint) = if app.profiles.is_empty() {
            (
                app.i18n.label(Label::ExplorerEmptyTitle).to_string(),
                app.i18n.label(Label::ExplorerEmptyHint).to_string(),
            )
        } else {
            let query = app.explorer.filter.text().to_string();
            (app.i18n.msg(&Msg::ExplorerFilterNone { query }).to_string(), String::new())
        };
        let mut ly = y + 1;
        for (text, style) in [
            (title, Style::new().fg(theme::FG).bg(theme::BG).add_modifier(Modifier::BOLD)),
            (hint, Style::new().fg(theme::FG_MUTED).bg(theme::BG)),
        ] {
            for l in wrap_words(&text, w.saturating_sub(2)) {
                if ly >= bottom {
                    break;
                }
                put(buf, area.x + 1, ly, &clip(&l, w.saturating_sub(2)), w.saturating_sub(2), style);
                ly += 1;
            }
        }
    }
    cursor
}
