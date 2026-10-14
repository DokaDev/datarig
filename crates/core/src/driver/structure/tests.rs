use super::*;

fn col(name: &str) -> StructureColumn {
    StructureColumn {
        name: name.into(),
        type_name: "integer".into(),
        not_null: false,
        default: None,
        fill: ColumnFill::Default,
    }
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn index(name: &str, columns: &[&str], unique: bool, predicate: Option<&str>) -> Index {
    Index {
        name: name.into(),
        columns: names(columns),
        options: Vec::new(),
        include: Vec::new(),
        // An expression is no column.
        key_columns: columns.iter().map(|c| (!c.contains('(')).then(|| c.to_string())).collect(),
        include_columns: Vec::new(),
        unique,
        method: "btree".into(),
        predicate: predicate.map(str::to_string),
        primary: false,
        constraint: false,
        definition: String::new(),
    }
}

#[test]
fn groups_follow_the_kind() {
    use StructureGroup::*;
    assert_eq!(RelationKind::Table.groups().len(), 7);
    assert_eq!(RelationKind::PartitionedTable.groups(), RelationKind::Table.groups());
    assert_eq!(RelationKind::View.groups(), [Columns, Triggers]);
    assert_eq!(RelationKind::MaterializedView.groups(), [Columns, Indexes]);
    assert_eq!(RelationKind::ForeignTable.groups(), [Columns, CheckConstraints, Triggers]);
    assert!(RelationKind::MaterializedView.has_storage() && !RelationKind::View.has_storage());
    assert!(!RelationKind::ForeignTable.has_storage());
}

#[test]
fn marks_read_the_keys_as_the_key_catalog_does() {
    let mut s = TableStructure::new(RelationKind::Table);
    s.columns = ["id", "a", "b", "c", "d", "e"].map(col).to_vec();
    s.primary_key = Some(KeyConstraint { name: "t_pkey".into(), columns: names(&["id"]), definition: String::new() });
    s.foreign_keys.push(ForeignKey {
        name: "t_a_fkey".into(),
        columns: names(&["a", "id"]),
        ref_schema: "s".into(),
        ref_table: "o".into(),
        ref_columns: names(&["x", "y"]),
        on_delete: FkAction::Cascade,
        on_update: FkAction::NoAction,
        definition: String::new(),
    });
    s.unique_constraints.push(KeyConstraint {
        name: "t_b_key".into(),
        columns: names(&["b"]),
        definition: String::new(),
    });
    s.indexes = vec![
        index("t_c_key", &["c"], true, None),
        // Partial, or on an expression: no unique mark.
        index("t_d_partial", &["d"], true, Some("d > 0")),
        index("t_e_lower", &["lower(e)"], true, None),
        index("t_e_plain", &["e"], false, None),
    ];
    let m = |c| s.marks(c);
    assert_eq!(m("id"), KeyMarks { pk: true, fk: true, unique: false });
    assert_eq!(m("a"), KeyMarks { fk: true, ..KeyMarks::default() });
    assert_eq!(m("b"), KeyMarks { unique: true, ..KeyMarks::default() });
    assert_eq!(m("c"), KeyMarks { unique: true, ..KeyMarks::default() });
    assert_eq!(m("d"), KeyMarks::default());
    assert_eq!(m("e"), KeyMarks::default());
    assert_eq!(s.count(StructureGroup::PrimaryKey), 1);
    assert_eq!(s.count(StructureGroup::Indexes), 4);
    assert_eq!(s.count(StructureGroup::Triggers), 0);
}

#[test]
fn item_columns_follow_each_item() {
    use StructureGroup::*;
    let mut s = TableStructure::new(RelationKind::Table);
    s.columns = ["id", "a", "b"].map(col).to_vec();
    s.primary_key =
        Some(KeyConstraint { name: "t_pkey".into(), columns: names(&["b", "a"]), definition: String::new() });
    s.foreign_keys.push(ForeignKey {
        name: "t_fkey".into(),
        columns: names(&["a", "b"]),
        ref_schema: "s".into(),
        ref_table: "o".into(),
        ref_columns: names(&["x", "y"]),
        on_delete: FkAction::NoAction,
        on_update: FkAction::NoAction,
        definition: String::new(),
    });
    let mut x = index("t_idx", &["\"a\"", "lower(b)"], false, None);
    x.key_columns[0] = Some("a".into());
    x.options = names(&["DESC", ""]);
    x.include = names(&["id"]);
    x.include_columns = names(&["id"]);
    s.indexes.push(x);
    s.checks.push(CheckConstraint {
        name: "t_check".into(),
        expression: "a < b".into(),
        columns: names(&["a", "b"]),
        definition: String::new(),
    });
    let text = |g, k| s.item_columns(g, k).into_iter().map(|c| c.text).collect::<Vec<_>>();
    assert_eq!(text(PrimaryKey, 0), ["b", "a"]);
    assert_eq!(text(CheckConstraints, 0), ["a", "b"]);
    let fk = s.item_columns(ForeignKeys, 0);
    assert_eq!(fk.iter().map(|c| c.references.as_deref()).collect::<Vec<_>>(), [Some("x"), Some("y")]);
    let idx = s.item_columns(Indexes, 0);
    assert_eq!(text(Indexes, 0), ["a", "lower(b)", "id"], "a column by its name, unquoted");
    assert_eq!(idx.iter().map(|c| c.column.as_deref()).collect::<Vec<_>>(), [Some("a"), None, Some("id")]);
    assert_eq!(
        idx.iter().map(|c| (c.options.as_str(), c.include)).collect::<Vec<_>>(),
        [("DESC", false), ("", false), ("", true)]
    );
    for (g, k) in [(Columns, 0), (Triggers, 0), (Indexes, 1), (UniqueConstraints, 0)] {
        assert!(s.item_columns(g, k).is_empty(), "{g:?} {k}");
    }
    assert_eq!(s.column("b").map(|c| c.name.as_str()), Some("b"));
}

#[test]
fn actions_timings_and_events_read_as_sql() {
    assert_eq!(FkAction::SetNull.sql(), "SET NULL");
    assert_eq!(FkAction::default(), FkAction::NoAction);
    assert_eq!(TriggerTiming::InsteadOf.sql(), "INSTEAD OF");
    assert_eq!(TriggerEvent::Truncate.sql(), "TRUNCATE");
}
