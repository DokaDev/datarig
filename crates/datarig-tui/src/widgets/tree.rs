//! The schema tree of one connected profile: schema -> Tables / Views -> objects ->
//! columns, with lazy loading of schema children. An object's columns come from the profile's
//! completion catalog (no request of their own); the tree only knows which objects are open. The explorer ([`crate::app::explorer`]) shows it under
//! the profile's node and owns the selection; this is the model and the text of each node.

use crate::theme;
use datarig_core::driver::SchemaObjects;
use datarig_core::i18n::{I18n, Label};
use ratatui::style::{Modifier, Style};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Children {
    NotLoaded,
    Loading,
    /// Materialized views are among `views` and named again in `materialized`.
    Loaded {
        tables: Vec<String>,
        views: Vec<String>,
        materialized: BTreeSet<String>,
    },
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct SchemaNode {
    pub name: String,
    pub expanded: bool,
    pub children: Children,
    pub tables_open: bool,
    pub views_open: bool,
    /// Objects whose columns are shown.
    pub open: BTreeSet<(Group, usize)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    Tables,
    Views,
}

/// A node below the profile. Schema indices are stable until the schemas are listed again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Node {
    Schema(usize),
    Group(usize, Group),
    Object(usize, Group, usize),
    /// Column `k` of an open object, in the catalog's order.
    Column(usize, Group, usize, usize),
    /// An open object whose columns the catalog does not have (still loading, or it could not
    /// be read).
    NoColumns(usize, Group, usize),
    /// Loading the schemas (`None`) or a schema's objects.
    Loading(Option<usize>),
    Empty(usize),
    Error(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct Row {
    /// Below the profile node: schemas are at depth 0.
    pub depth: usize,
    pub node: Node,
}

pub enum TreeAction {
    None,
    LoadObjects(String),
    LoadSchemas,
    Open { schema: String, name: String },
}

pub struct Tree {
    pub schemas: Vec<SchemaNode>,
    pub schemas_loading: bool,
}

impl Default for Tree {
    fn default() -> Self {
        Self::new()
    }
}

impl Tree {
    pub fn new() -> Self {
        Self { schemas: Vec::new(), schemas_loading: true }
    }

    pub fn set_schemas(&mut self, names: Vec<String>) {
        let old = std::mem::take(&mut self.schemas);
        self.schemas = names
            .into_iter()
            .map(|name| {
                old.iter().find(|s| s.name == name).cloned().unwrap_or(SchemaNode {
                    name,
                    expanded: false,
                    children: Children::NotLoaded,
                    tables_open: true,
                    views_open: true,
                    open: BTreeSet::new(),
                })
            })
            .collect();
        self.schemas_loading = false;
    }

    /// The schema node `n` is (or is under).
    pub fn schema_of(&self, n: Node) -> Option<&str> {
        let i = match n {
            Node::Schema(i)
            | Node::Group(i, _)
            | Node::Object(i, _, _)
            | Node::Column(i, _, _, _)
            | Node::NoColumns(i, _, _)
            | Node::Empty(i)
            | Node::Error(i)
            | Node::Loading(Some(i)) => i,
            Node::Loading(None) => return None,
        };
        self.schemas.get(i).map(|s| s.name.as_str())
    }

    pub fn set_objects(&mut self, schema: &str, result: Result<SchemaObjects, String>) {
        if let Some(s) = self.schemas.iter_mut().find(|s| s.name == schema) {
            s.children = match result {
                Ok(SchemaObjects { tables, views, materialized }) => Children::Loaded { tables, views, materialized },
                Err(e) => Children::Failed(e),
            };
        }
    }

    /// The visible nodes, in order (the columns of open objects left out).
    pub fn rows(&self) -> Vec<Row> {
        self.rows_with(|_, _| None)
    }

    /// The visible nodes, in order, with the columns of open objects: `columns(schema, name)`
    /// is how many the catalog has (`None`: it does not have the object).
    pub fn rows_with(&self, columns: impl Fn(&str, &str) -> Option<usize>) -> Vec<Row> {
        let mut out = Vec::new();
        if self.schemas_loading {
            out.push(Row { depth: 0, node: Node::Loading(None) });
        }
        for (i, s) in self.schemas.iter().enumerate() {
            out.push(Row { depth: 0, node: Node::Schema(i) });
            if !s.expanded {
                continue;
            }
            match &s.children {
                Children::NotLoaded | Children::Loading => out.push(Row { depth: 1, node: Node::Loading(Some(i)) }),
                Children::Failed(_) => out.push(Row { depth: 1, node: Node::Error(i) }),
                Children::Loaded { tables, views, .. } => {
                    if tables.is_empty() && views.is_empty() {
                        out.push(Row { depth: 1, node: Node::Empty(i) });
                    }
                    for (g, items, open) in
                        [(Group::Tables, tables, s.tables_open), (Group::Views, views, s.views_open)]
                    {
                        if items.is_empty() {
                            continue;
                        }
                        out.push(Row { depth: 1, node: Node::Group(i, g) });
                        if open {
                            for (j, name) in items.iter().enumerate() {
                                out.push(Row { depth: 2, node: Node::Object(i, g, j) });
                                if !s.open.contains(&(g, j)) {
                                    continue;
                                }
                                match columns(&s.name, name) {
                                    Some(n) => {
                                        out.extend((0..n).map(|k| Row { depth: 3, node: Node::Column(i, g, j, k) }))
                                    }
                                    None => out.push(Row { depth: 3, node: Node::NoColumns(i, g, j) }),
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// Whether `node` is still in the tree (schemas can change when they are listed again).
    pub fn has(&self, node: Node) -> bool {
        self.rows().iter().any(|r| r.node == node)
    }

    /// `(schema, object)` of an object node.
    pub fn object_name(&self, i: usize, g: Group, j: usize) -> Option<(String, String)> {
        let s = self.schemas.get(i)?;
        match &s.children {
            Children::Loaded { tables, views, .. } => {
                let list = if g == Group::Tables { tables } else { views };
                list.get(j).map(|n| (s.name.clone(), n.clone()))
            }
            _ => None,
        }
    }

    /// Whether view `j` of schema `i` is a materialized view.
    pub fn is_materialized(&self, i: usize, j: usize) -> bool {
        match self.schemas.get(i).map(|s| &s.children) {
            Some(Children::Loaded { views, materialized, .. }) => {
                views.get(j).is_some_and(|v| materialized.contains(v))
            }
            _ => false,
        }
    }

    /// Expand (`Some(true)`), collapse (`Some(false)`) or toggle (`None`) `node`.
    pub fn set_expanded(&mut self, node: Node, want: Option<bool>) -> TreeAction {
        match node {
            Node::Schema(i) => {
                let Some(s) = self.schemas.get_mut(i) else { return TreeAction::None };
                s.expanded = want.unwrap_or(!s.expanded);
                if s.expanded && matches!(s.children, Children::NotLoaded | Children::Failed(_)) {
                    s.children = Children::Loading;
                    return TreeAction::LoadObjects(s.name.clone());
                }
            }
            Node::Group(i, g) => {
                let Some(s) = self.schemas.get_mut(i) else { return TreeAction::None };
                let open = if g == Group::Tables { &mut s.tables_open } else { &mut s.views_open };
                *open = want.unwrap_or(!*open);
            }
            Node::Object(i, g, j) => {
                let Some(s) = self.schemas.get_mut(i) else { return TreeAction::None };
                let open = s.open.contains(&(g, j));
                if want.unwrap_or(!open) {
                    s.open.insert((g, j));
                } else {
                    s.open.remove(&(g, j));
                }
            }
            _ => {}
        }
        TreeAction::None
    }

    /// `Some(open)` for a node that can be expanded.
    pub fn is_expanded(&self, node: Node) -> Option<bool> {
        match node {
            Node::Schema(i) => self.schemas.get(i).map(|s| s.expanded),
            Node::Group(i, Group::Tables) => self.schemas.get(i).map(|s| s.tables_open),
            Node::Group(i, Group::Views) => self.schemas.get(i).map(|s| s.views_open),
            Node::Object(i, g, j) => self.schemas.get(i).map(|s| s.open.contains(&(g, j))),
            _ => None,
        }
    }

    /// Enter / double-click: an object opens, anything else toggles.
    pub fn activate(&mut self, node: Node) -> TreeAction {
        match node {
            Node::Object(i, g, j) => match self.object_name(i, g, j) {
                Some((schema, name)) => TreeAction::Open { schema, name },
                None => TreeAction::None,
            },
            n => self.set_expanded(n, None),
        }
    }

    /// Reload what `node` shows (`None`: the profile itself, i.e. the schemas).
    pub fn refresh(&mut self, node: Option<Node>) -> TreeAction {
        let schema = match node {
            None | Some(Node::Loading(None)) => None,
            Some(
                Node::Schema(i)
                | Node::Group(i, _)
                | Node::Object(i, _, _)
                | Node::Column(i, _, _, _)
                | Node::NoColumns(i, _, _)
                | Node::Loading(Some(i))
                | Node::Empty(i)
                | Node::Error(i),
            ) => Some(i),
        };
        match schema.and_then(|i| self.schemas.get_mut(i)) {
            Some(s) => {
                s.children = Children::Loading;
                s.expanded = true;
                TreeAction::LoadObjects(s.name.clone())
            }
            None => {
                self.schemas_loading = true;
                TreeAction::LoadSchemas
            }
        }
    }

    /// The text of `node` and its style.
    pub fn label(&self, node: Node, i18n: &I18n) -> (String, Style) {
        let dim = Style::new().fg(theme::FG_DIM).add_modifier(Modifier::ITALIC);
        match node {
            Node::Schema(i) => {
                (self.schemas.get(i).map(|s| s.name.clone()).unwrap_or_default(), Style::new().fg(theme::FG))
            }
            Node::Group(_, g) => (
                i18n.label(if g == Group::Tables { Label::TreeGroupTables } else { Label::TreeGroupViews }).to_string(),
                Style::new().fg(theme::FG_MUTED),
            ),
            Node::Object(i, g, j) => {
                (self.object_name(i, g, j).map(|(_, n)| n).unwrap_or_default(), Style::new().fg(theme::FG))
            }
            // Drawn by the explorer from the catalog (see `widgets::explorer`).
            Node::Column(..) | Node::NoColumns(..) => (String::new(), Style::new()),
            Node::Loading(_) => (i18n.label(Label::TreeLoading).to_string(), dim),
            Node::Empty(_) => (i18n.label(Label::TreeEmpty).to_string(), dim),
            Node::Error(i) => match self.schemas.get(i).map(|s| &s.children) {
                Some(Children::Failed(e)) => (e.clone(), Style::new().fg(theme::ERROR)),
                _ => (String::new(), Style::new()),
            },
        }
    }
}
