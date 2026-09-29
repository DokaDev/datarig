use super::*;

#[test]
fn an_estimate_of_minus_one_is_unknown_not_zero() {
    let s = parse(
        r#"{"kind":"r","rows":-1,"bytes":8192,"columns":null,"constraints":null,"indexes":null,"triggers":null}"#,
    )
    .unwrap();
    assert_eq!((s.kind, s.estimated_rows, s.total_bytes), (RelationKind::Table, None, Some(8192)));
    let s = parse(r#"{"kind":"m","rows":10999.6,"bytes":0}"#).unwrap();
    assert_eq!((s.kind, s.estimated_rows), (RelationKind::MaterializedView, Some(11000)));
    let s = parse(r#"{"kind":"v","rows":null,"bytes":null}"#).unwrap();
    assert_eq!((s.kind, s.estimated_rows, s.total_bytes), (RelationKind::View, None, None));
    assert_eq!(parse(r#"{"kind":"S"}"#), Err(DbError::NotSupported), "a sequence");
}

#[test]
fn columns_say_how_they_are_filled() {
    let json = r#"{"kind":"r","columns":[
        {"name":"id","type":"bigint","not_null":true,"default":null,"generated":"","identity":"a"},
        {"name":"n","type":"integer","not_null":false,"default":"0","generated":"","identity":""},
        {"name":"twice","type":"integer","not_null":false,"default":"(n * 2)","generated":"s","identity":""},
        {"name":"k","type":"integer","not_null":true,"default":null,"generated":"","identity":"d"}]}"#;
    let s = parse(json).unwrap();
    let fills: Vec<(&str, &ColumnFill, Option<&str>)> =
        s.columns.iter().map(|c| (c.name.as_str(), &c.fill, c.default.as_deref())).collect();
    assert_eq!(
        fills,
        [
            ("id", &ColumnFill::IdentityAlways, None),
            ("n", &ColumnFill::Default, Some("0")),
            ("twice", &ColumnFill::Stored("(n * 2)".into()), None),
            ("k", &ColumnFill::IdentityByDefault, None),
        ]
    );
}

#[test]
fn trigger_types_decode_from_their_bits() {
    let t = |tgtype, enabled: &str| {
        trigger(RawTrigger {
            name: "t".into(),
            tgtype,
            enabled: enabled.into(),
            function: "s.f".into(),
            definition: String::new(),
        })
    };
    // BEFORE INSERT OR UPDATE FOR EACH ROW.
    let a = t(1 | 2 | 4 | 16, "O");
    assert_eq!(
        (a.timing, a.events.as_slice(), a.for_each_row, a.enabled),
        (TriggerTiming::Before, [TriggerEvent::Insert, TriggerEvent::Update].as_slice(), true, true)
    );
    // AFTER DELETE OR TRUNCATE FOR EACH STATEMENT, disabled.
    let b = t(8 | 32, "D");
    assert_eq!(
        (b.timing, b.events.as_slice(), b.for_each_row, b.enabled),
        (TriggerTiming::After, [TriggerEvent::Delete, TriggerEvent::Truncate].as_slice(), false, false)
    );
    // INSTEAD OF UPDATE on a view; replica and always count as enabled.
    let c = t(1 | 16 | 64, "R");
    assert_eq!((c.timing, c.enabled), (TriggerTiming::InsteadOf, true));
    assert!(t(4, "A").enabled);
}

#[test]
fn foreign_key_actions_decode() {
    assert_eq!(
        ["a", "r", "c", "n", "d"].map(fk_action),
        [FkAction::NoAction, FkAction::Restrict, FkAction::Cascade, FkAction::SetNull, FkAction::SetDefault]
    );
}
