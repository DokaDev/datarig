//! The schema tree of one connected profile: schema -> Tables / Views -> objects -> their
//! structure, with lazy loading of schema children. Each object with storage has the estimates
//! of its rows and size read with its schema's list (and again with its structure), which its
//! line shows. With a driver that reads a table's
//! structure (`Capabilities::structure`) an open object shows its groups
//! (Columns, Primary Key, Foreign Keys, Indexes, Unique and Check Constraints, Triggers: those
//! of its kind, an empty one dim and without a count; a key, an index or a check opens to the
//! `Columns` it covers), read once when it first opens and kept ([`Tree::structures`]); without
//! it, its columns from the profile's completion catalog. The
//! explorer ([`crate::app::explorer`]) shows it under the profile's node and owns the
//! selection; this is the model and the text of each node.

use crate::theme;
use datarig_core::driver::SchemaObjects;
use datarig_core::driver::structure::{RelationStats, StructureGroup, TableStructure};
use datarig_core::i18n::{I18n, Label};
use ratatui::style::{Modifier, Style};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Children {
    NotLoaded,
    Loading,
    /// Materialized views are among `views` and named again in `materialized`; `stats` has the
    /// estimates of the objects with storage, by name.
    Loaded {
        tables: Vec<String>,
        views: Vec<String>,
        materialized: BTreeSet<String>,
        stats: BTreeMap<String, RelationStats>,
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

/// A table's structure as the tree has it.
#[derive(Clone, Debug)]
pub enum Structure {
    Loading,
    Loaded(Box<TableStructure>),
    /// Why it could not be read.
    Failed(String),
}

/// An object whose structure was asked for: the structure, and which of its groups, items and
/// items' `Columns` are open (kept while the object closes, and while it is read again).
#[derive(Clone, Debug)]
pub struct ObjectView {
    pub structure: Structure,
    pub open_groups: BTreeSet<StructureGroup>,
    pub open_items: BTreeSet<(StructureGroup, usize)>,
    pub open_columns: BTreeSet<(StructureGroup, usize)>,
}

impl ObjectView {
    /// Loading, every group closed: the object first shows the lines of its groups.
    fn new() -> Self {
        Self {
            structure: Structure::Loading,
            open_groups: BTreeSet::new(),
            open_items: BTreeSet::new(),
            open_columns: BTreeSet::new(),
        }
    }

    /// The structure, once read.
    pub fn loaded(&self) -> Option<&TableStructure> {
        match &self.structure {
            Structure::Loaded(s) => Some(s),
            _ => None,
        }
    }
}

/// How many lines item `k` of group `g` opens to before its `Columns`: a foreign key the line
/// of what it references; the others none.
pub fn item_details(s: &TableStructure, g: StructureGroup, k: usize) -> usize {
    match g {
        StructureGroup::ForeignKeys => usize::from(k < s.foreign_keys.len()),
        _ => 0,
    }
}

/// Whether item `k` of group `g` opens at all: to its details, or to the columns it covers
/// (a key, an index, a check that reads columns).
fn item_opens(s: &TableStructure, g: StructureGroup, k: usize) -> bool {
    item_details(s, g, k) > 0 || !s.item_columns(g, k).is_empty()
}

/// Where a foreign key's table is (the explorer's jump).
pub enum Reveal {
    /// Its node.
    Found(Node),
    /// Its schema's objects are being read: the cursor goes there once they come
    /// ([`Tree::take_reveal`]); the action asks for them.
    Pending(TreeAction),
    /// The tree does not have it.
    Missing,
}

/// A node below the profile. Schema indices are stable until the schemas are listed again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Node {
    Schema(usize),
    Group(usize, Group),
    Object(usize, Group, usize),
    /// Column `k` of an open object, in the catalog's order (a driver without the table
    /// structure).
    Column(usize, Group, usize, usize),
    /// An open object whose columns the catalog does not have (still loading, or it could not
    /// be read).
    NoColumns(usize, Group, usize),
    /// A group of an open object's structure.
    StructGroup(usize, Group, usize, StructureGroup),
    /// Item `k` of a group (a column, a key, an index, …).
    StructItem(usize, Group, usize, StructureGroup, usize),
    /// Line `m` under an open item (a foreign key's reference).
    StructDetail(usize, Group, usize, StructureGroup, usize, usize),
    /// `Columns (n)` under an open item: the columns it covers
    /// ([`TableStructure::item_columns`]).
    StructColumns(usize, Group, usize, StructureGroup, usize),
    /// Column `m` of an open item's open `Columns`.
    StructColumn(usize, Group, usize, StructureGroup, usize, usize),
    /// An open object whose structure is being read, or why it could not be.
    StructNote(usize, Group, usize),
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
    Open {
        schema: String,
        name: String,
    },
    /// Read the structure of `schema.name` (only with a driver that has it: the explorer
    /// marks it loading when it asks, [`Tree::structure_loading`]).
    LoadStructure {
        schema: String,
        name: String,
    },
    /// Put the cursor on table `schema.name` (a foreign key's target).
    Reveal {
        schema: String,
        name: String,
    },
}

pub struct Tree {
    pub schemas: Vec<SchemaNode>,
    pub schemas_loading: bool,
    /// The structures asked for, by `(schema, object)`: read once when an object first opens,
    /// again with `r` on it.
    pub structures: HashMap<(String, String), ObjectView>,
    /// A table the cursor goes to once its schema's objects are listed.
    reveal: Option<(String, String)>,
}

impl Default for Tree {
    fn default() -> Self {
        Self::new()
    }
}

impl Tree {
    pub fn new() -> Self {
        Self { schemas: Vec::new(), schemas_loading: true, structures: HashMap::new(), reveal: None }
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
            | Node::StructGroup(i, _, _, _)
            | Node::StructItem(i, _, _, _, _)
            | Node::StructDetail(i, _, _, _, _, _)
            | Node::StructColumns(i, _, _, _, _)
            | Node::StructColumn(i, _, _, _, _, _)
            | Node::StructNote(i, _, _)
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
                Ok(SchemaObjects { tables, views, materialized, stats }) => {
                    Children::Loaded { tables, views, materialized, stats }
                }
                Err(e) => Children::Failed(e),
            };
        }
    }

    /// The visible nodes, in order (what open objects show left out).
    pub fn rows(&self) -> Vec<Row> {
        self.rows_with(|_, _| None, false)
    }

    /// The visible nodes, in order, with what open objects show: their structure (`structured`,
    /// the driver reads it) or their columns, `columns(schema, name)` being how many the
    /// catalog has (`None`: it does not have the object).
    pub fn rows_with(&self, columns: impl Fn(&str, &str) -> Option<usize>, structured: bool) -> Vec<Row> {
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
                                if structured {
                                    self.push_structure(&mut out, (i, g, j), &s.name, name);
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

    /// The rows of open object `(i, g, j)`, `schema.name`, below it: the groups of its kind,
    /// each open one with its items, an open item with its lines and its `Columns` (open: the
    /// columns).
    fn push_structure(&self, out: &mut Vec<Row>, (i, g, j): (usize, Group, usize), schema: &str, name: &str) {
        let view = self.structures.get(&(schema.to_string(), name.to_string()));
        let Some((view, st)) = view.and_then(|v| v.loaded().map(|s| (v, s))) else {
            return out.push(Row { depth: 3, node: Node::StructNote(i, g, j) });
        };
        for &sg in st.kind.groups() {
            out.push(Row { depth: 3, node: Node::StructGroup(i, g, j, sg) });
            if !view.open_groups.contains(&sg) {
                continue;
            }
            for k in 0..st.count(sg) {
                out.push(Row { depth: 4, node: Node::StructItem(i, g, j, sg, k) });
                if !view.open_items.contains(&(sg, k)) {
                    continue;
                }
                let lines = item_details(st, sg, k);
                out.extend((0..lines).map(|m| Row { depth: 5, node: Node::StructDetail(i, g, j, sg, k, m) }));
                let columns = st.item_columns(sg, k).len();
                if columns == 0 {
                    continue;
                }
                out.push(Row { depth: 5, node: Node::StructColumns(i, g, j, sg, k) });
                if view.open_columns.contains(&(sg, k)) {
                    out.extend((0..columns).map(|m| Row { depth: 6, node: Node::StructColumn(i, g, j, sg, k, m) }));
                }
            }
        }
    }

    /// The view of object `(i, g, j)` (its structure, what of it is open), once asked for.
    pub fn object_view(&self, i: usize, g: Group, j: usize) -> Option<&ObjectView> {
        self.structures.get(&self.object_name(i, g, j)?)
    }

    fn object_view_mut(&mut self, i: usize, g: Group, j: usize) -> Option<&mut ObjectView> {
        let key = self.object_name(i, g, j)?;
        self.structures.get_mut(&key)
    }

    /// The structure of `schema.name` is being read (asked now): what was open stays open.
    pub fn structure_loading(&mut self, schema: &str, name: &str) {
        let v = self.structures.entry((schema.to_string(), name.to_string())).or_insert_with(ObjectView::new);
        v.structure = Structure::Loading;
    }

    /// The structure of `schema.name` came, or why it could not be read. Its estimates, read
    /// with it, are the object's from now on (its line shows them).
    pub fn set_structure(&mut self, schema: &str, name: &str, result: Result<Box<TableStructure>, String>) {
        if let Some(st) = result.as_ref().ok().and_then(|s| s.stats())
            && let Some(Children::Loaded { stats, .. }) =
                self.schemas.iter_mut().find(|s| s.name == schema).map(|s| &mut s.children)
        {
            stats.insert(name.to_string(), st);
        }
        let v = self.structures.entry((schema.to_string(), name.to_string())).or_insert_with(ObjectView::new);
        v.structure = match result {
            Ok(s) => Structure::Loaded(s),
            Err(e) => Structure::Failed(e),
        };
    }

    /// Where table `schema.name` is, its schema and group opened (a foreign key's target).
    pub fn reveal(&mut self, schema: &str, name: &str) -> Reveal {
        let Some(i) = self.schemas.iter().position(|s| s.name == schema) else { return Reveal::Missing };
        let s = &mut self.schemas[i];
        s.expanded = true;
        match &s.children {
            Children::Loaded { .. } => match self.find(i, name) {
                Some(n) => Reveal::Found(n),
                None => Reveal::Missing,
            },
            Children::Loading => {
                self.reveal = Some((schema.to_string(), name.to_string()));
                Reveal::Pending(TreeAction::None)
            }
            Children::NotLoaded | Children::Failed(_) => {
                s.children = Children::Loading;
                self.reveal = Some((schema.to_string(), name.to_string()));
                Reveal::Pending(TreeAction::LoadObjects(schema.to_string()))
            }
        }
    }

    /// The objects of `schema` came: the table waiting to be revealed there, if one was, with
    /// its node (`None`: the schema does not have it).
    pub fn take_reveal(&mut self, schema: &str) -> Option<(String, Option<Node>)> {
        let (_, name) = self.reveal.take_if(|(s, _)| s == schema)?;
        let i = self.schemas.iter().position(|s| s.name == schema)?;
        let node = self.find(i, &name);
        Some((name, node))
    }

    /// The node of object `name` of loaded schema `i` (a table first), its group opened.
    fn find(&mut self, i: usize, name: &str) -> Option<Node> {
        let s = self.schemas.get_mut(i)?;
        let Children::Loaded { tables, views, .. } = &s.children else { return None };
        if let Some(j) = tables.iter().position(|t| t == name) {
            s.tables_open = true;
            return Some(Node::Object(i, Group::Tables, j));
        }
        let j = views.iter().position(|v| v == name)?;
        s.views_open = true;
        Some(Node::Object(i, Group::Views, j))
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

    /// The estimates of object `(i, g, j)`: `None` for one without storage (a view, a foreign
    /// table) or whose estimates were not read.
    pub fn object_stats(&self, i: usize, g: Group, j: usize) -> Option<RelationStats> {
        let (_, name) = self.object_name(i, g, j)?;
        match &self.schemas.get(i)?.children {
            Children::Loaded { stats, .. } => stats.get(&name).copied(),
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
                    // Its structure is read the first time (again after a failure).
                    let asked = self.object_view(i, g, j).is_some_and(|v| !matches!(v.structure, Structure::Failed(_)));
                    if !asked && let Some((schema, name)) = self.object_name(i, g, j) {
                        return TreeAction::LoadStructure { schema, name };
                    }
                } else {
                    s.open.remove(&(g, j));
                }
            }
            Node::StructGroup(i, g, j, sg) => {
                if self.is_expanded(node).is_none() {
                    return TreeAction::None;
                }
                let Some(v) = self.object_view_mut(i, g, j) else { return TreeAction::None };
                let open = v.open_groups.contains(&sg);
                if want.unwrap_or(!open) {
                    v.open_groups.insert(sg);
                } else {
                    v.open_groups.remove(&sg);
                }
            }
            Node::StructItem(i, g, j, sg, k) => {
                if self.is_expanded(node).is_none() {
                    return TreeAction::None;
                }
                let Some(v) = self.object_view_mut(i, g, j) else { return TreeAction::None };
                let open = v.open_items.contains(&(sg, k));
                if want.unwrap_or(!open) {
                    v.open_items.insert((sg, k));
                } else {
                    v.open_items.remove(&(sg, k));
                }
            }
            Node::StructColumns(i, g, j, sg, k) => {
                if self.is_expanded(node).is_none() {
                    return TreeAction::None;
                }
                let Some(v) = self.object_view_mut(i, g, j) else { return TreeAction::None };
                let open = v.open_columns.contains(&(sg, k));
                if want.unwrap_or(!open) {
                    v.open_columns.insert((sg, k));
                } else {
                    v.open_columns.remove(&(sg, k));
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
            // An empty group, and an item with nothing under it, have nothing to open.
            Node::StructGroup(i, g, j, sg) => {
                let v = self.object_view(i, g, j)?;
                (v.loaded()?.count(sg) > 0).then(|| v.open_groups.contains(&sg))
            }
            Node::StructItem(i, g, j, sg, k) => {
                let v = self.object_view(i, g, j)?;
                item_opens(v.loaded()?, sg, k).then(|| v.open_items.contains(&(sg, k)))
            }
            Node::StructColumns(i, g, j, sg, k) => {
                let v = self.object_view(i, g, j)?;
                (!v.loaded()?.item_columns(sg, k).is_empty()).then(|| v.open_columns.contains(&(sg, k)))
            }
            _ => None,
        }
    }

    /// Enter / double-click: an object opens, a foreign key (its line, or one of its columns)
    /// goes to the table it references, anything else toggles.
    pub fn activate(&mut self, node: Node) -> TreeAction {
        match node {
            Node::Object(i, g, j) => match self.object_name(i, g, j) {
                Some((schema, name)) => TreeAction::Open { schema, name },
                None => TreeAction::None,
            },
            Node::StructItem(i, g, j, StructureGroup::ForeignKeys, k)
            | Node::StructDetail(i, g, j, StructureGroup::ForeignKeys, k, _)
            | Node::StructColumn(i, g, j, StructureGroup::ForeignKeys, k, _) => {
                let fk = self.object_view(i, g, j).and_then(|v| v.loaded()).and_then(|s| s.foreign_keys.get(k));
                match fk {
                    Some(f) => TreeAction::Reveal { schema: f.ref_schema.clone(), name: f.ref_table.clone() },
                    None => TreeAction::None,
                }
            }
            n => self.set_expanded(n, None),
        }
    }

    /// Reload what `node` shows (`None`: the profile itself, i.e. the schemas). With the table
    /// structure (`structured`) an open object, or anything of its structure, reads its
    /// structure again; otherwise its schema's objects are listed again.
    pub fn refresh(&mut self, node: Option<Node>, structured: bool) -> TreeAction {
        let object = match node {
            Some(Node::Object(i, g, j)) if self.is_expanded(Node::Object(i, g, j)) == Some(true) => Some((i, g, j)),
            Some(
                Node::StructGroup(i, g, j, _)
                | Node::StructItem(i, g, j, _, _)
                | Node::StructDetail(i, g, j, _, _, _)
                | Node::StructColumns(i, g, j, _, _)
                | Node::StructColumn(i, g, j, _, _, _)
                | Node::StructNote(i, g, j),
            ) => Some((i, g, j)),
            _ => None,
        };
        if structured && let Some((schema, name)) = object.and_then(|(i, g, j)| self.object_name(i, g, j)) {
            return TreeAction::LoadStructure { schema, name };
        }
        let schema = match node {
            None | Some(Node::Loading(None)) => None,
            Some(
                Node::Schema(i)
                | Node::Group(i, _)
                | Node::Object(i, _, _)
                | Node::Column(i, _, _, _)
                | Node::NoColumns(i, _, _)
                | Node::StructGroup(i, _, _, _)
                | Node::StructItem(i, _, _, _, _)
                | Node::StructDetail(i, _, _, _, _, _)
                | Node::StructColumns(i, _, _, _, _)
                | Node::StructColumn(i, _, _, _, _, _)
                | Node::StructNote(i, _, _)
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
            // Drawn by the explorer from the catalog or the structure (see `widgets::explorer`).
            Node::Column(..)
            | Node::NoColumns(..)
            | Node::StructGroup(..)
            | Node::StructItem(..)
            | Node::StructDetail(..)
            | Node::StructColumns(..)
            | Node::StructColumn(..)
            | Node::StructNote(..) => (String::new(), Style::new()),
            Node::Loading(_) => (i18n.label(Label::TreeLoading).to_string(), dim),
            Node::Empty(_) => (i18n.label(Label::TreeEmpty).to_string(), dim),
            Node::Error(i) => match self.schemas.get(i).map(|s| &s.children) {
                Some(Children::Failed(e)) => (e.clone(), Style::new().fg(theme::ERROR)),
                _ => (String::new(), Style::new()),
            },
        }
    }
}
