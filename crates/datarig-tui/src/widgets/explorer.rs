//! Drawing the explorer: "＋ New connection" pinned on
//! top, folders, each profile with its state (`○` not connected, a spinner while connecting,
//! `●` connected, `✕` failed) in the profile's color, its icon and name (`RO` after it when its
//! policy is read-only), the error line under a
//! failed profile, and under a connected one its server's databases (its own first, marked
//! `default`), each with its schema tree. With icons on every node of the tree has
//! an icon of its kind, and a column that is no key the icon of its type. An open table shows
//! its structure (see `widgets::tree`): each group with its icon, each item with its group's.

use crate::app::explorer::{Row, RowKind};
use crate::app::{App, Keys, NodeState};
use crate::icons::{self, KeyMark, TreeIcon, TypeCategory};
use crate::text::{Align, clip, fit, human_bytes, human_count, wrap_words};
use crate::theme;
use crate::widgets::tree::{Group, Node, ObjectView, Structure, Tree};
use crate::widgets::{put, spinner_at};
use datarig_core::driver::KeyMarks;
use datarig_core::driver::structure::{ColumnFill, FkAction, StructureGroup, TableStructure};
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
        Node::Stats(..) | Node::StructGroup(..) | Node::StructItem(..) | Node::StructDetail(..) | Node::StructNote(..)
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

/// A node of an open object's structure: its size estimate, a group (`Columns (7)`; an empty
/// one dim, without a count), an item with what it is in dim text, an item's line, or why the
/// structure is not there.
fn structure_parts(app: &App, tree: &Tree, n: Node) -> Vec<(String, Style)> {
    let dim = Style::new().fg(theme::FG_DIM);
    let (Node::Stats(i, g, j)
    | Node::StructGroup(i, g, j, _)
    | Node::StructItem(i, g, j, _, _)
    | Node::StructDetail(i, g, j, _, _, _)
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
        Node::Stats(..) => {
            let size = human_bytes(st.total_bytes.unwrap_or(0));
            let text = match st.estimated_rows {
                Some(r) => app.i18n.msg(&Msg::TreeStats { rows: human_count(r), size }),
                None => app.i18n.msg(&Msg::TreeStatsRowsUnknown { size }),
            };
            vec![(text.to_string(), dim)]
        }
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
        Node::StructDetail(.., StructureGroup::PrimaryKey, _, m) => {
            let col = st.primary_key.as_ref().and_then(|p| p.columns.get(m)).cloned().unwrap_or_default();
            vec![(col, Style::new().fg(theme::FG))]
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
            parts.push((format!("  {}", detail.join(", ")), dim));
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
            let mut detail = format!("({})", x.columns.join(", "));
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
            let events: Vec<&str> = t.events.iter().map(|e| e.sql()).collect();
            let each = if t.for_each_row { "FOR EACH ROW" } else { "FOR EACH STATEMENT" };
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

/// The whole line of `row` as the explorer draws it, not cut at its width: the indentation, the
/// arrow and the text.
pub(crate) fn row_text(app: &App, row: &Row) -> String {
    let arrow = match app.explorer_arrow(row) {
        Some(true) => "▾ ",
        Some(false) => "▸ ",
        None => "  ",
    };
    let text: String = row_parts(app, row).into_iter().map(|(t, _)| t).collect();
    format!("{}{arrow}{text}", "  ".repeat(row.depth))
}

/// The whole text of the row under the explorer's cursor when it is part of a table's
/// structure, for the status bar: the explorer is narrow, and cuts the details of deep lines.
pub(crate) fn structure_preview(app: &App) -> Option<String> {
    let row = app.explorer_row()?;
    let (RowKind::Node(_, n) | RowKind::AuxNode(_, _, n)) = &row.kind else { return None };
    if !is_structure(*n) {
        return None;
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
