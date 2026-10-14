use super::*;

#[test]
fn a_locked_object_is_locked_whatever_else_the_document_has() {
    assert_eq!(parse(r#"{"locked":true}"#), Err(DbError::Locked));
    assert_eq!(parse(r#"{"locked":true,"index":null}"#), Err(DbError::Locked));
    assert_eq!(parse("{}"), Err(DbError::NoResult));
}

#[test]
fn an_index_has_its_own_comment_or_its_constraints() {
    let json = r#"{"index":{"schema":"s","name":"i","table":"t","definition":"CREATE INDEX i ON s.t USING btree (a)",
        "constraint":null,"constraint_comment":null,"comment":"by a"}}"#;
    let Ok(DdlSource::Index(i)) = parse(json) else { panic!() };
    assert_eq!((i.comment.as_deref(), i.constraint), (Some("by a"), None));
    let json = r#"{"index":{"schema":"s","name":"t_pkey","table":"t","definition":"CREATE UNIQUE INDEX t_pkey …",
        "constraint":["t_pkey","PRIMARY KEY (a)"],"constraint_comment":"the key","comment":"ignored"}}"#;
    let Ok(DdlSource::Index(i)) = parse(json) else { panic!() };
    assert_eq!(i.comment.as_deref(), Some("the key"));
    assert_eq!(i.constraint, Some(("t_pkey".into(), "PRIMARY KEY (a)".into())));
}

#[test]
fn a_relation_reads_its_catalog_codes() {
    let json = r#"{"relation":{"kind":"r","columns":[
          {"name":"id","type":"integer","not_null":true,"default":null,"generated":"","identity":"a"},
          {"name":"note","type":"text","not_null":false,"default":null,"generated":"","identity":""}],
        "constraints":null,"indexes":[
          {"name":"t_email","unique":true,"primary":false,"constraint":false,"method":"btree","columns":["email"],
           "options":[""],"include":null,"key_columns":["email"],"include_columns":null,"predicate":null,
           "definition":"CREATE UNIQUE INDEX t_email ON s.t USING btree (email)"}],"triggers":null},
      "ddl":{"schema":"s","name":"t","owner":"o","persistence":"u","am":"heap","options":["fillfactor=70"],
        "toast_options":null,"tablespace":null,"partition_key":null,"partition_of":null,"inherits":null,"view":null,
        "server":null,"server_options":null,
        "columns":[
          {"name":"id","local":true,"collation":null,"storage":null,"compression":"","statistics":-1,"options":null,
           "comment":null,"grants":null,"identity":{"schema":"s","name":"t_id_seq","type":"integer","start":1,
           "increment":1,"min":1,"max":2147483647,"cache":1,"cycle":false}},
          {"name":"note","local":true,"collation":["pg_catalog","C"],"storage":"e","compression":"l","statistics":200,
           "options":["n_distinct=5"],"comment":"a note","grants":[[null,"SELECT",false],["r","UPDATE",true]],
           "identity":null}],
        "sequences":null,"constraints":null,
        "indexes":[{"name":"t_email","inherited":false,"replica":true,"comment":"by email"}],
        "triggers":[{"name":"x","enabled":"D","inherited":false,"comment":null},
                    {"name":"y","enabled":"A","inherited":true,"comment":null}],
        "replica_identity":"i","row_security":true,"force_row_security":false,
        "policies":[{"name":"p","permissive":false,"command":"w","roles":[null,"r"],"using":"(true)","check":null,
                     "comment":null}],
        "grants":[["r","SELECT",false]],"comment":"t"}}"#;
    let Ok(DdlSource::Relation(r)) = parse(json) else { panic!("{:?}", parse(json)) };
    assert!(r.unlogged);
    assert_eq!(r.access_method, None, "heap is the default");
    assert_eq!(r.columns[0].statistics, None, "-1: the default");
    assert_eq!(r.columns[0].compression, None, "empty: the default");
    assert_eq!(r.columns[0].identity.as_ref().map(|s| s.max), Some(2147483647));
    let note = &r.columns[1];
    assert_eq!(note.collation, Some(("pg_catalog".into(), "C".into())));
    assert_eq!(
        (note.storage.as_deref(), note.compression.as_deref(), note.statistics),
        (Some("EXTERNAL"), Some("lz4"), Some(200))
    );
    assert_eq!(
        note.grants,
        [
            Grant { grantee: None, privilege: "SELECT".into(), grantable: false },
            Grant { grantee: Some("r".into()), privilege: "UPDATE".into(), grantable: true },
        ]
    );
    assert_eq!(r.replica_identity, ReplicaIdentity::Index("t_email".into()));
    assert_eq!(r.index_comments, [Comment { name: "t_email".into(), text: "by email".into() }]);
    assert_eq!(
        r.triggers.iter().map(|t| (t.mode, t.inherited)).collect::<Vec<_>>(),
        [(TriggerMode::Disabled, false), (TriggerMode::Always, true)]
    );
    let p = &r.policies[0];
    assert_eq!(
        (p.permissive, p.command.as_deref(), p.roles.clone()),
        (false, Some("UPDATE"), vec![None, Some("r".into())])
    );
}
