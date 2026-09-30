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
fn actions_timings_and_events_read_as_sql() {
    assert_eq!(FkAction::SetNull.sql(), "SET NULL");
    assert_eq!(FkAction::default(), FkAction::NoAction);
    assert_eq!(TriggerTiming::InsteadOf.sql(), "INSTEAD OF");
    assert_eq!(TriggerEvent::Truncate.sql(), "TRUNCATE");
}
