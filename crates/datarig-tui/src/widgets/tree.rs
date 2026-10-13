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
    /// estimates of the objects with storage, by name, and `heat` the size tier of the largest
    /// of them (see [`size_heat`]), kept with the estimates.
    Loaded {
        tables: Vec<String>,
        views: Vec<String>,
        materialized: BTreeSet<String>,
        stats: BTreeMap<String, RelationStats>,
        heat: BTreeMap<String, Heat>,
    },
    Failed(String),
}

/// How large an object is among those of its schema: its estimates are drawn in a warmer color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heat {
    /// Among the largest tenth of its schema (by size, at least one object), and 1 MiB or more.
    Warm,
    /// Warm, and 1 GiB or more.
    Hot,
}

/// The size an object must have at least to be warm, and hot.
const WARM_BYTES: u64 = 1 << 20;
const HOT_BYTES: u64 = 1 << 30;

/// The size tiers of a schema's objects, from their estimates: of the `n` with a known size, the
/// `max(1, round(n / 10))` largest are warm when they take 1 MiB or more, and hot when they take
/// 1 GiB or more. Equal sizes rank alike (an object's rank is how many are strictly larger). An
/// object whose size is not known, and one without storage, has no tier (left out).
pub fn size_heat(stats: &BTreeMap<String, RelationStats>) -> BTreeMap<String, Heat> {
    let mut sizes: Vec<u64> = stats.values().filter_map(|s| s.bytes).collect();
    if sizes.is_empty() {
        return BTreeMap::new();
    }
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    let top = ((sizes.len() as f64 * 0.10).round() as usize).max(1);
    stats
        .iter()
        .filter_map(|(name, s)| {
            let bytes = s.bytes.filter(|&b| b >= WARM_BYTES)?;
            let rank = sizes.partition_point(|&x| x > bytes);
            (rank < top).then(|| (name.clone(), if bytes >= HOT_BYTES { Heat::Hot } else { Heat::Warm }))
        })
        .collect()
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
/// of what it references, a trigger with a `WHEN` condition the condition; the others none.
pub fn item_details(s: &TableStructure, g: StructureGroup, k: usize) -> usize {
    match g {
        StructureGroup::ForeignKeys => usize::from(k < s.foreign_keys.len()),
        StructureGroup::Triggers => usize::from(s.triggers.get(k).is_some_and(|t| t.condition.is_some())),
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
    /// The explorer's `/` filter shows it open for a match below it, whatever its own
    /// expansion (which stays as it is).
    pub opened: bool,
    /// Its name matches the explorer's `/` filter.
    pub hit: bool,
}

impl Row {
    /// `node` at `depth`, as its own expansion shows it.
    fn at(depth: usize, node: Node) -> Self {
        Self { depth, node, opened: false, hit: false }
    }

    /// `node` at `depth`, shown open by the filter.
    fn opened(depth: usize, node: Node) -> Self {
        Self { opened: true, ..Self::at(depth, node) }
    }
}

/// The name of item `k` of group `sg` (a column, a key, an index, a check, a trigger).
fn item_name(st: &TableStructure, sg: StructureGroup, k: usize) -> Option<&str> {
    Some(match sg {
        StructureGroup::Columns => &st.columns.get(k)?.name,
        StructureGroup::PrimaryKey => &st.primary_key.as_ref().filter(|_| k == 0)?.name,
        StructureGroup::ForeignKeys => &st.foreign_keys.get(k)?.name,
        StructureGroup::Indexes => &st.indexes.get(k)?.name,
        StructureGroup::UniqueConstraints => &st.unique_constraints.get(k)?.name,
        StructureGroup::CheckConstraints => &st.checks.get(k)?.name,
        StructureGroup::Triggers => &st.triggers.get(k)?.name,
    })
}

/// Item `k` of group `sg` of object `(i, g, j)` and, open, its lines and its `Columns` (open:
/// the columns).
fn push_item(
    out: &mut Vec<Row>,
    view: &ObjectView,
    st: &TableStructure,
    (i, g, j): (usize, Group, usize),
    sg: StructureGroup,
    k: usize,
) {
    out.push(Row::at(4, Node::StructItem(i, g, j, sg, k)));
    if !view.open_items.contains(&(sg, k)) {
        return;
    }
    let lines = item_details(st, sg, k);
    out.extend((0..lines).map(|m| Row::at(5, Node::StructDetail(i, g, j, sg, k, m))));
    let columns = st.item_columns(sg, k).len();
    if columns == 0 {
        return;
    }
    out.push(Row::at(5, Node::StructColumns(i, g, j, sg, k)));
    if view.open_columns.contains(&(sg, k)) {
        out.extend((0..columns).map(|m| Row::at(6, Node::StructColumn(i, g, j, sg, k, m))));
    }
}

/// Object `(i, g, j)` opened by the filter with the items of its structure (`view`, `st`) whose
/// name matches, each in its group (opened) and as its own expansion shows it; nothing when
/// none does.
fn push_structure_matching(
    out: &mut Vec<Row>,
    (i, g, j): (usize, Group, usize),
    view: &ObjectView,
    st: &TableStructure,
    hit: &impl Fn(&str) -> bool,
) {
    let at = out.len();
    out.push(Row::opened(2, Node::Object(i, g, j)));
    for &sg in st.kind.groups() {
        let group = out.len();
        out.push(Row::opened(3, Node::StructGroup(i, g, j, sg)));
        for k in 0..st.count(sg) {
            if item_name(st, sg, k).is_some_and(hit) {
                let o = out.len();
                push_item(out, view, st, (i, g, j), sg, k);
                out[o].hit = true;
            }
        }
        if out.len() == group + 1 {
            out.truncate(group);
        }
    }
    if out.len() == at + 1 {
        out.truncate(at);
    }
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
                    let heat = size_heat(&stats);
                    Children::Loaded { tables, views, materialized, stats, heat }
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
            out.push(Row::at(0, Node::Loading(None)));
        }
        for i in 0..self.schemas.len() {
            self.push_schema(&mut out, i, &columns, structured);
        }
        out
    }

    /// The nodes the explorer's `/` filter shows, in order (`hit`: whether a name matches). A
    /// schema, an object or an item of an object's structure whose name matches shows as
    /// [`Tree::rows_with`] shows it (its own expansion, nothing else filtered); one with a match
    /// below it shows opened, whatever its own expansion, with only the branches that lead to
    /// matches; the others are left out. Only what was read is searched: the objects of a schema
    /// that were never listed, and the structure of an object that was never opened, are not.
    /// Nothing is asked for, and no expansion changes.
    pub fn rows_matching(
        &self,
        columns: impl Fn(&str, &str) -> Option<usize>,
        structured: bool,
        hit: impl Fn(&str) -> bool,
    ) -> Vec<Row> {
        let mut out = Vec::new();
        for (i, s) in self.schemas.iter().enumerate() {
            if hit(&s.name) {
                let at = out.len();
                self.push_schema(&mut out, i, &columns, structured);
                out[at].hit = true;
                continue;
            }
            let Children::Loaded { tables, views, .. } = &s.children else { continue };
            // The objects of this schema with a matching item in the structure read for them.
            let found: Vec<(Group, usize, &ObjectView, &TableStructure)> = if structured {
                self.structures
                    .iter()
                    .filter(|((schema, _), _)| *schema == s.name)
                    .filter_map(|((_, name), v)| {
                        let st = v.loaded()?;
                        let (g, j) = match tables.iter().position(|t| t == name) {
                            Some(j) => (Group::Tables, j),
                            None => (Group::Views, views.iter().position(|t| t == name)?),
                        };
                        Some((g, j, v, st))
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let at = out.len();
            out.push(Row::opened(0, Node::Schema(i)));
            for (g, items) in [(Group::Tables, tables), (Group::Views, views)] {
                let group = out.len();
                out.push(Row::opened(1, Node::Group(i, g)));
                for (j, name) in items.iter().enumerate() {
                    if hit(name) {
                        let o = out.len();
                        self.push_object(&mut out, (i, g, j), &s.name, name, &columns, structured);
                        out[o].hit = true;
                    } else if let Some((_, _, v, st)) = found.iter().find(|f| (f.0, f.1) == (g, j)) {
                        push_structure_matching(&mut out, (i, g, j), v, st, &hit);
                    }
                }
                if out.len() == group + 1 {
                    out.truncate(group);
                }
            }
            if out.len() == at + 1 {
                out.truncate(at);
            }
        }
        out
    }

    /// Schema `i` and, open, what it shows (see [`Tree::rows_with`]).
    fn push_schema(
        &self,
        out: &mut Vec<Row>,
        i: usize,
        columns: &impl Fn(&str, &str) -> Option<usize>,
        structured: bool,
    ) {
        let s = &self.schemas[i];
        out.push(Row::at(0, Node::Schema(i)));
        if !s.expanded {
            return;
        }
        match &s.children {
            Children::NotLoaded | Children::Loading => out.push(Row::at(1, Node::Loading(Some(i)))),
            Children::Failed(_) => out.push(Row::at(1, Node::Error(i))),
            Children::Loaded { tables, views, .. } => {
                if tables.is_empty() && views.is_empty() {
                    out.push(Row::at(1, Node::Empty(i)));
                }
                for (g, items, open) in [(Group::Tables, tables, s.tables_open), (Group::Views, views, s.views_open)] {
                    if items.is_empty() {
                        continue;
                    }
                    out.push(Row::at(1, Node::Group(i, g)));
                    if open {
                        for (j, name) in items.iter().enumerate() {
                            self.push_object(out, (i, g, j), &s.name, name, columns, structured);
                        }
                    }
                }
            }
        }
    }

    /// Object `(i, g, j)`, `schema.name`, and, open, what it shows: its structure
    /// (`structured`) or its columns.
    fn push_object(
        &self,
        out: &mut Vec<Row>,
        (i, g, j): (usize, Group, usize),
        schema: &str,
        name: &str,
        columns: &impl Fn(&str, &str) -> Option<usize>,
        structured: bool,
    ) {
        out.push(Row::at(2, Node::Object(i, g, j)));
        if !self.schemas[i].open.contains(&(g, j)) {
            return;
        }
        if structured {
            return self.push_structure(out, (i, g, j), schema, name);
        }
        match columns(schema, name) {
            Some(n) => out.extend((0..n).map(|k| Row::at(3, Node::Column(i, g, j, k)))),
            None => out.push(Row::at(3, Node::NoColumns(i, g, j))),
        }
    }

    /// The rows of open object `(i, g, j)`, `schema.name`, below it: the groups of its kind,
    /// each open one with its items, an open item with its lines and its `Columns` (open: the
    /// columns).
    fn push_structure(&self, out: &mut Vec<Row>, (i, g, j): (usize, Group, usize), schema: &str, name: &str) {
        let view = self.structures.get(&(schema.to_string(), name.to_string()));
        let Some((view, st)) = view.and_then(|v| v.loaded().map(|s| (v, s))) else {
            return out.push(Row::at(3, Node::StructNote(i, g, j)));
        };
        for &sg in st.kind.groups() {
            out.push(Row::at(3, Node::StructGroup(i, g, j, sg)));
            if !view.open_groups.contains(&sg) {
                continue;
            }
            for k in 0..st.count(sg) {
                push_item(out, view, st, (i, g, j), sg, k);
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

    /// The explorer asked for the structure of `schema.name` and waits for it.
    pub fn structure_awaited(&self, schema: &str, name: &str) -> bool {
        self.structures
            .get(&(schema.to_string(), name.to_string()))
            .is_some_and(|v| matches!(v.structure, Structure::Loading))
    }

    /// The structure of `schema.name` came, or why it could not be read. Its estimates, read
    /// with it, are the object's from now on (its line shows them), and the schema's size tiers
    /// follow them.
    pub fn set_structure(&mut self, schema: &str, name: &str, result: Result<Box<TableStructure>, String>) {
        if let Some(st) = result.as_ref().ok().and_then(|s| s.stats())
            && let Some(Children::Loaded { stats, heat, .. }) =
                self.schemas.iter_mut().find(|s| s.name == schema).map(|s| &mut s.children)
        {
            stats.insert(name.to_string(), st);
            *heat = size_heat(stats);
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

    /// The size tier of object `(i, g, j)` in its schema (`None`: not among the largest, or its
    /// size is not known).
    pub fn object_heat(&self, i: usize, g: Group, j: usize) -> Option<Heat> {
        let (_, name) = self.object_name(i, g, j)?;
        match &self.schemas.get(i)?.children {
            Children::Loaded { heat, .. } => heat.get(&name).copied(),
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
        let th = theme::cur();
        let dim = Style::new().fg(th.fg_dim).add_modifier(Modifier::ITALIC);
        match node {
            Node::Schema(i) => {
                (self.schemas.get(i).map(|s| s.name.clone()).unwrap_or_default(), Style::new().fg(th.fg))
            }
            Node::Group(_, g) => (
                i18n.label(if g == Group::Tables { Label::TreeGroupTables } else { Label::TreeGroupViews }).to_string(),
                Style::new().fg(th.fg_muted),
            ),
            Node::Object(i, g, j) => {
                (self.object_name(i, g, j).map(|(_, n)| n).unwrap_or_default(), Style::new().fg(th.fg))
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
                Some(Children::Failed(e)) => (e.clone(), Style::new().fg(th.error)),
                _ => (String::new(), Style::new()),
            },
        }
    }
}

#[cfg(test)]
mod tests;
