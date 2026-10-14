use super::*;
use datarig_core::driver::structure::{ColumnFill, KeyConstraint, RelationKind, StructureColumn};

const MIB: u64 = 1 << 20;
const GIB: u64 = 1 << 30;

/// Estimates by name: `Some(bytes)` or an unknown size.
fn stats(sizes: &[(&str, Option<u64>)]) -> BTreeMap<String, RelationStats> {
    sizes.iter().map(|(n, b)| (n.to_string(), RelationStats { rows: Some(1), bytes: *b })).collect()
}

fn heat(sizes: &[(&str, Option<u64>)]) -> Vec<(String, Heat)> {
    size_heat(&stats(sizes)).into_iter().collect()
}

#[test]
fn the_largest_tenth_of_a_schema_has_the_tier_of_its_size_from_one_mib() {
    // Small schemas: the largest one only (a tenth rounds to none, one at least).
    assert_eq!(heat(&[("a", Some(5 * MIB)), ("b", Some(2 * MIB))]), [("a".to_string(), Heat::Mib)]);
    assert_eq!(heat(&[("only", Some(3 * GIB))]), [("only".to_string(), Heat::Gib)]);
    // Twenty with a known size: the two largest.
    let mut sizes: Vec<(String, Option<u64>)> = (0..20).map(|i| (format!("t{i:02}"), Some(i * MIB))).collect();
    sizes[19].1 = Some(2 * GIB);
    let named: Vec<(&str, Option<u64>)> = sizes.iter().map(|(n, b)| (n.as_str(), *b)).collect();
    assert_eq!(heat(&named), [("t18".to_string(), Heat::Mib), ("t19".to_string(), Heat::Gib)]);
    // Fifteen: 1.5 rounds to two.
    let named: Vec<(String, Option<u64>)> = (0..15).map(|i| (format!("t{i:02}"), Some((i + 1) * MIB))).collect();
    let named: Vec<(&str, Option<u64>)> = named.iter().map(|(n, b)| (n.as_str(), *b)).collect();
    assert_eq!(heat(&named).len(), 2);
}

#[test]
fn size_tiers_need_a_known_size_of_one_mib_and_rank_ties_alike() {
    // Under 1 MiB nothing has a tier, however large it is among its schema's.
    assert!(heat(&[("a", Some(MIB - 1)), ("b", Some(10))]).is_empty());
    assert_eq!(heat(&[("a", Some(MIB)), ("b", Some(10))]), [("a".to_string(), Heat::Mib)], "1 MiB");
    assert_eq!(heat(&[("a", Some(GIB - 1))]), [("a".to_string(), Heat::Mib100)]);
    assert_eq!(heat(&[("a", Some(GIB))]), [("a".to_string(), Heat::Gib)], "1 GiB");
    assert_eq!(heat(&[("a", Some(100 * MIB - 1))]), [("a".to_string(), Heat::Mib)]);
    assert_eq!(heat(&[("a", Some(100 * MIB))]), [("a".to_string(), Heat::Mib100)], "100 MiB");
    assert_eq!(heat(&[("a", Some(10 * GIB - 1))]), [("a".to_string(), Heat::Gib)]);
    assert_eq!(heat(&[("a", Some(10 * GIB))]), [("a".to_string(), Heat::Gib10)], "10 GiB");
    // Unknown sizes are no tier and do not count: the one known size is the largest.
    assert_eq!(
        heat(&[("a", None), ("b", None), ("c", Some(4 * MIB))]),
        [("c".to_string(), Heat::Mib)],
        "one known of three"
    );
    assert!(heat(&[("a", None)]).is_empty());
    assert!(size_heat(&BTreeMap::new()).is_empty());
    // Equal sizes rank alike: both largest have a tier although the tenth is one.
    assert_eq!(
        heat(&[("a", Some(8 * MIB)), ("b", Some(8 * MIB)), ("c", Some(2 * MIB))]),
        [("a".to_string(), Heat::Mib), ("b".to_string(), Heat::Mib)]
    );
}

#[test]
fn size_tiers_are_kept_with_the_estimates_and_follow_a_structure_read() {
    let mut t = Tree::new();
    t.set_schemas(vec!["public".into()]);
    let objects = SchemaObjects {
        tables: vec!["big".into(), "small".into()],
        views: vec!["v".into()],
        materialized: BTreeSet::new(),
        stats: stats(&[("big", Some(50 * MIB)), ("small", Some(2 * MIB))]),
    };
    t.set_objects("public", Ok(objects));
    assert_eq!(t.object_heat(0, Group::Tables, 0), Some(Heat::Mib));
    assert_eq!(t.object_heat(0, Group::Tables, 1), None);
    assert_eq!(t.object_heat(0, Group::Views, 0), None, "a view without storage");
    // The small table's structure says it grew past the other: the tiers follow.
    let mut st = TableStructure::new(RelationKind::Table);
    st.total_bytes = Some(2 * GIB);
    t.set_structure("public", "small", Ok(Box::new(st)));
    assert_eq!(t.object_heat(0, Group::Tables, 1), Some(Heat::Gib));
    assert_eq!(t.object_heat(0, Group::Tables, 0), None);
}

/// A tree with schemas `app` (loaded: tables `orders`, `users`, view `order_totals`; the
/// structure of `users` read) and `audit` (never listed).
fn tree() -> Tree {
    let mut t = Tree::new();
    t.set_schemas(vec!["app".into(), "audit".into()]);
    t.set_objects(
        "app",
        Ok(SchemaObjects {
            tables: vec!["orders".into(), "users".into()],
            views: vec!["order_totals".into()],
            materialized: BTreeSet::new(),
            stats: BTreeMap::new(),
        }),
    );
    let mut st = TableStructure::new(RelationKind::Table);
    let col = |name: &str| StructureColumn {
        name: name.into(),
        type_name: "text".into(),
        not_null: false,
        default: None,
        fill: ColumnFill::Default,
    };
    st.columns = vec![col("id"), col("email")];
    st.primary_key = Some(KeyConstraint {
        name: "users_pkey".into(),
        columns: vec!["id".into()],
        definition: "PRIMARY KEY (id)".into(),
    });
    t.set_structure("app", "users", Ok(Box::new(st)));
    t
}

/// The rows the filter `q` shows: `node depth [open] [hit]`.
fn matching(t: &Tree, q: &str) -> Vec<String> {
    t.rows_matching(|_, _| None, true, |n| n.contains(q))
        .into_iter()
        .map(|r| {
            let name = match r.node {
                Node::Schema(i) => t.schemas[i].name.clone(),
                Node::Group(_, g) => format!("{g:?}"),
                Node::Object(i, g, j) => t.object_name(i, g, j).unwrap().1,
                Node::StructGroup(.., sg) => format!("{sg:?}"),
                Node::StructItem(i, g, j, sg, k) => {
                    item_name(t.object_view(i, g, j).unwrap().loaded().unwrap(), sg, k).unwrap().to_string()
                }
                n => format!("{n:?}"),
            };
            let mut s = format!("{}{name}", "  ".repeat(r.depth));
            if r.opened {
                s.push_str(" +");
            }
            if r.hit {
                s.push_str(" *");
            }
            s
        })
        .collect()
}

#[test]
fn the_filter_shows_matching_nodes_with_the_way_to_them_opened() {
    let t = tree();
    // A table: its schema and group opened (both closed in the tree).
    assert_eq!(matching(&t, "orders"), ["app +", "  Tables +", "    orders *"]);
    // A table and a view.
    assert_eq!(matching(&t, "order"), ["app +", "  Tables +", "    orders *", "  Views +", "    order_totals *"]);
    // A structure item: the object and its group opened, the other groups left out.
    assert_eq!(matching(&t, "email"), ["app +", "  Tables +", "    users +", "      Columns +", "        email *"]);
    assert_eq!(
        matching(&t, "pkey"),
        ["app +", "  Tables +", "    users +", "      PrimaryKey +", "        users_pkey *"]
    );
    // A schema matches by its name: shown as its expansion says (closed), unloaded or not.
    assert_eq!(matching(&t, "audit"), ["audit *"]);
    assert_eq!(matching(&t, "app"), ["app *"]);
    assert!(matching(&t, "zzz").is_empty());
    // The filter changed nothing.
    assert!(!t.schemas[0].expanded && t.schemas[0].open.is_empty());
}

#[test]
fn a_matching_node_shows_its_own_expansion_and_unread_parts_are_not_searched() {
    let mut t = tree();
    // `users` open with its Columns group: shown as the tree shows it, all of it.
    let users = Node::Object(0, Group::Tables, 1);
    let _ = t.set_expanded(users, Some(true));
    let _ = t.set_expanded(Node::StructGroup(0, Group::Tables, 1, StructureGroup::Columns), Some(true));
    let rows = matching(&t, "users");
    assert_eq!(
        rows[..5],
        ["app +", "  Tables +", "    users *", "      Columns", "        id"].map(String::from),
        "{rows:?}"
    );
    // Without the structure (another driver), its items are not searched.
    assert!(t.rows_matching(|_, _| None, false, |n| n.contains("email")).is_empty());
    // A schema never listed has nothing searched in it, and stays unloaded.
    let mut t = tree();
    t.schemas[1].expanded = false;
    assert!(matching(&t, "log").is_empty());
    assert!(matches!(t.schemas[1].children, Children::NotLoaded));
}
