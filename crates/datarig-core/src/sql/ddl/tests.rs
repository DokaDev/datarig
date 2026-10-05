use super::*;
use crate::driver::ddl::{Comment, ForeignTable, OwnedSequence, PartitionOf, TriggerExtra};
use crate::driver::structure::{
    CheckConstraint, ForeignKey, Index, KeyConstraint, StructureColumn, TableStructure, Trigger, TriggerEvent,
    TriggerTiming,
};

fn col(name: &str, type_name: &str, not_null: bool, default: Option<&str>) -> StructureColumn {
    StructureColumn {
        name: name.into(),
        type_name: type_name.into(),
        not_null,
        default: default.map(str::to_string),
        fill: ColumnFill::Default,
    }
}

fn seq(schema: &str, name: &str, type_name: &str) -> SequenceDdl {
    let (_, max) = type_range(type_name);
    SequenceDdl {
        schema: schema.into(),
        name: name.into(),
        type_name: type_name.into(),
        start: 1,
        increment: 1,
        min: 1,
        max,
        cache: 1,
        cycle: false,
    }
}

fn index(name: &str, definition: &str, constraint: bool) -> Index {
    Index {
        name: name.into(),
        columns: Vec::new(),
        options: Vec::new(),
        include: Vec::new(),
        key_columns: Vec::new(),
        include_columns: Vec::new(),
        unique: constraint,
        method: "btree".into(),
        predicate: None,
        primary: false,
        constraint,
        definition: definition.into(),
    }
}

fn trigger(name: &str, definition: &str) -> Trigger {
    Trigger {
        name: name.into(),
        timing: TriggerTiming::Before,
        events: vec![TriggerEvent::Update],
        for_each_row: true,
        function: "shop.touch".into(),
        enabled: true,
        update_columns: Vec::new(),
        condition: None,
        definition: definition.into(),
    }
}

fn grant(grantee: Option<&str>, privilege: &str, grantable: bool) -> Grant {
    Grant { grantee: grantee.map(str::to_string), privilege: privilege.into(), grantable }
}

/// `shop.users` as the dev database has it: a `bigserial` key with the sequence it owns, a
/// unique email, a check, a foreign key, an index, a trigger, a comment and a grant.
fn users() -> RelationDdl {
    let mut s = TableStructure::new(RelationKind::Table);
    s.columns = vec![
        col("id", "bigint", true, Some("nextval('shop.users_id_seq'::regclass)")),
        col("email", "text", true, None),
        col("team_id", "integer", false, None),
        col("created_at", "timestamp with time zone", true, Some("now()")),
    ];
    s.primary_key = Some(KeyConstraint {
        name: "users_pkey".into(),
        columns: vec!["id".into()],
        definition: "PRIMARY KEY (id)".into(),
    });
    s.unique_constraints.push(KeyConstraint {
        name: "users_email_key".into(),
        columns: vec!["email".into()],
        definition: "UNIQUE (email)".into(),
    });
    s.checks.push(CheckConstraint {
        name: "users_email_check".into(),
        expression: "email <> ''::text".into(),
        columns: vec!["email".into()],
        definition: "CHECK (email <> ''::text)".into(),
    });
    s.foreign_keys.push(ForeignKey {
        name: "users_team_id_fkey".into(),
        columns: vec!["team_id".into()],
        ref_schema: "shop".into(),
        ref_table: "teams".into(),
        ref_columns: vec!["id".into()],
        on_delete: Default::default(),
        on_update: Default::default(),
        definition: "FOREIGN KEY (team_id) REFERENCES shop.teams(id) ON DELETE SET NULL".into(),
    });
    s.indexes = vec![
        index(
            "users_created_at_idx",
            "CREATE INDEX users_created_at_idx ON shop.users USING btree (created_at)",
            false,
        ),
        index("users_email_key", "CREATE UNIQUE INDEX users_email_key ON shop.users USING btree (email)", true),
        index("users_pkey", "CREATE UNIQUE INDEX users_pkey ON shop.users USING btree (id)", true),
    ];
    s.triggers.push(trigger(
        "users_touch",
        "CREATE TRIGGER users_touch BEFORE UPDATE ON shop.users FOR EACH ROW EXECUTE FUNCTION shop.touch()",
    ));
    let mut r = RelationDdl::new("shop", "users", "datarig", s);
    r.sequences.push(OwnedSequence { column: "id".into(), sequence: seq("shop", "users_id_seq", "bigint") });
    r.comment = Some("People who can sign in.".into());
    r.columns[1].comment = Some("Lower case; it's unique.".into());
    r.grants = vec![grant(None, "SELECT", false), grant(Some("report"), "SELECT", false)];
    r
}

#[test]
fn a_table_with_its_sequence_keys_index_trigger_comments_and_grants() {
    let got = ddl_text(&DdlSource::Relation(Box::new(users())));
    let want = "\
-- Reconstructed by datarig from the catalog (not pg_dump)
-- Not included: rows, sequence values, rules, extended statistics, security labels

CREATE SEQUENCE shop.users_id_seq;

CREATE TABLE shop.users (
    id bigint DEFAULT nextval('shop.users_id_seq'::regclass) NOT NULL,
    email text NOT NULL,
    team_id integer,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT users_pkey PRIMARY KEY (id),
    CONSTRAINT users_email_key UNIQUE (email),
    CONSTRAINT users_email_check CHECK (email <> ''::text),
    CONSTRAINT users_team_id_fkey FOREIGN KEY (team_id) REFERENCES shop.teams(id) ON DELETE SET NULL
);

ALTER SEQUENCE shop.users_id_seq OWNED BY shop.users.id;

ALTER TABLE shop.users OWNER TO datarig;

CREATE INDEX users_created_at_idx ON shop.users USING btree (created_at);

CREATE TRIGGER users_touch BEFORE UPDATE ON shop.users FOR EACH ROW EXECUTE FUNCTION shop.touch();

COMMENT ON TABLE shop.users IS 'People who can sign in.';

COMMENT ON COLUMN shop.users.email IS 'Lower case; it''s unique.';

GRANT SELECT ON TABLE shop.users TO PUBLIC;

GRANT SELECT ON TABLE shop.users TO report;
";
    assert_eq!(got, want);
}

#[test]
fn names_that_need_quotes_are_quoted_and_others_are_not() {
    let mut s = TableStructure::new(RelationKind::Table);
    s.columns = vec![col("Mixed Col", "text", false, None), col("select", "integer", false, None)];
    let mut r = RelationDdl::new("My Schema", "order", "Owner \"x\"", s);
    r.columns[0].comment = Some("a\\b".into());
    r.columns[1].grants = vec![grant(Some("Report Role"), "UPDATE", true)];
    let got = ddl_text(&DdlSource::Relation(Box::new(r)));
    let want = "\
-- Reconstructed by datarig from the catalog (not pg_dump)
-- Not included: rows, sequence values, rules, extended statistics, security labels

CREATE TABLE \"My Schema\".\"order\" (
    \"Mixed Col\" text,
    \"select\" integer
);

ALTER TABLE \"My Schema\".\"order\" OWNER TO \"Owner \"\"x\"\"\";

COMMENT ON COLUMN \"My Schema\".\"order\".\"Mixed Col\" IS 'a\\b';

GRANT UPDATE (\"select\") ON TABLE \"My Schema\".\"order\" TO \"Report Role\" WITH GRANT OPTION;
";
    assert_eq!(got, want);
}

#[test]
fn hangul_names_are_quoted_and_comments_kept_as_they_are() {
    let name = "\u{c0ac}\u{c6a9}\u{c790}";
    let mut s = TableStructure::new(RelationKind::Table);
    s.columns = vec![col(name, "text", false, None)];
    let mut r = RelationDdl::new("public", name, "datarig", s);
    r.comment = Some(format!("{name} '\u{d14c}\u{c774}\u{be14}'"));
    let got = ddl_text(&DdlSource::Relation(Box::new(r)));
    assert!(got.contains(&format!("CREATE TABLE public.\"{name}\" (\n    \"{name}\" text\n);")), "{got}");
    assert!(
        got.contains(&format!("COMMENT ON TABLE public.\"{name}\" IS '{name} ''\u{d14c}\u{c774}\u{be14}''';")),
        "{got}"
    );
}

#[test]
fn columns_identity_generated_collation_and_settings() {
    let mut s = TableStructure::new(RelationKind::Table);
    s.columns = vec![
        StructureColumn { fill: ColumnFill::IdentityAlways, ..col("id", "integer", true, None) },
        StructureColumn { fill: ColumnFill::IdentityByDefault, ..col("n", "bigint", true, None) },
        StructureColumn { fill: ColumnFill::Stored("(n * 2)".into()), ..col("twice", "bigint", false, None) },
        StructureColumn { fill: ColumnFill::Virtual("lower(name)".into()), ..col("low", "text", false, None) },
        col("name", "text", false, None),
    ];
    let mut r = RelationDdl::new("s", "t", "o", s);
    r.unlogged = true;
    r.columns[0].identity = Some(seq("s", "t_id_seq", "integer"));
    r.columns[1].identity =
        Some(SequenceDdl { start: 100, increment: 10, cache: 20, cycle: true, ..seq("s", "custom_seq", "bigint") });
    r.columns[4].collation = Some(("pg_catalog".into(), "C".into()));
    r.columns[4].storage = Some("EXTERNAL".into());
    r.columns[4].compression = Some("lz4".into());
    r.columns[4].statistics = Some(500);
    r.columns[4].options = vec!["n_distinct=-1".into()];
    r.options = vec!["fillfactor=70".into(), "autovacuum_enabled=false".into()];
    r.toast_options = vec!["autovacuum_enabled=false".into()];
    r.tablespace = Some("fast".into());
    r.replica_identity = ReplicaIdentity::Full;
    let got = ddl_text(&DdlSource::Relation(Box::new(r)));
    let want = "\
-- Reconstructed by datarig from the catalog (not pg_dump)
-- Not included: rows, sequence values, rules, extended statistics, security labels

CREATE UNLOGGED TABLE s.t (
    id integer GENERATED ALWAYS AS IDENTITY NOT NULL,
    n bigint GENERATED BY DEFAULT AS IDENTITY (SEQUENCE NAME s.custom_seq INCREMENT BY 10 START WITH 100 CACHE 20 CYCLE) NOT NULL,
    twice bigint GENERATED ALWAYS AS ((n * 2)) STORED,
    low text GENERATED ALWAYS AS (lower(name)) VIRTUAL,
    name text COLLATE pg_catalog.\"C\"
)
WITH (fillfactor=70, autovacuum_enabled=false, toast.autovacuum_enabled=false)
TABLESPACE fast;

ALTER TABLE ONLY s.t ALTER COLUMN name SET STORAGE EXTERNAL;

ALTER TABLE ONLY s.t ALTER COLUMN name SET COMPRESSION lz4;

ALTER TABLE ONLY s.t ALTER COLUMN name SET STATISTICS 500;

ALTER TABLE ONLY s.t ALTER COLUMN name SET (n_distinct=-1);

ALTER TABLE s.t OWNER TO o;

ALTER TABLE ONLY s.t REPLICA IDENTITY FULL;
";
    assert_eq!(got, want);
}

#[test]
fn sequences_write_only_what_differs_from_their_defaults() {
    assert_eq!(sequence_options(&seq("s", "q", "bigint"), true), Vec::<String>::new());
    assert_eq!(sequence_options(&seq("s", "q", "integer"), true), ["AS integer"]);
    assert_eq!(sequence_options(&seq("s", "q", "integer"), false), Vec::<String>::new());
    let down = SequenceDdl { increment: -1, min: i64::MIN, max: -1, start: -1, ..seq("s", "q", "bigint") };
    assert_eq!(sequence_options(&down, true), ["INCREMENT BY -1"]);
    let odd = SequenceDdl { min: 5, max: 50, start: 7, ..seq("s", "q", "smallint") };
    assert_eq!(sequence_options(&odd, true), ["AS smallint", "MINVALUE 5", "MAXVALUE 50", "START WITH 7"]);
}

#[test]
fn a_partition_and_a_partitioned_table() {
    let mut s = TableStructure::new(RelationKind::PartitionedTable);
    s.columns = vec![col("id", "integer", true, None), col("at", "date", true, None)];
    s.primary_key = Some(KeyConstraint {
        name: "events_pkey".into(),
        columns: vec!["id".into(), "at".into()],
        definition: "PRIMARY KEY (id, at)".into(),
    });
    let mut r = RelationDdl::new("s", "events", "o", s);
    r.partition_key = Some("RANGE (at)".into());
    let got = ddl_text(&DdlSource::Relation(Box::new(r)));
    assert!(
        got.contains(
            "CREATE TABLE s.events (\n    id integer NOT NULL,\n    at date NOT NULL,\n    \
             CONSTRAINT events_pkey PRIMARY KEY (id, at)\n)\nPARTITION BY RANGE (at);"
        ),
        "{got}"
    );

    // Its partition: the parent's columns and key are not repeated; its own check is.
    let mut s = TableStructure::new(RelationKind::Table);
    s.columns = vec![col("id", "integer", true, None), col("at", "date", true, None)];
    s.primary_key = Some(KeyConstraint {
        name: "events_2024_pkey".into(),
        columns: vec!["id".into(), "at".into()],
        definition: "PRIMARY KEY (id, at)".into(),
    });
    s.checks.push(CheckConstraint {
        name: "events_2024_id_check".into(),
        expression: "id > 0".into(),
        columns: vec!["id".into()],
        definition: "CHECK (id > 0)".into(),
    });
    s.indexes = vec![index("events_2024_pkey", "CREATE UNIQUE INDEX events_2024_pkey ON s.events_2024", true)];
    s.triggers.push(trigger("t", "CREATE TRIGGER t …"));
    let mut r = RelationDdl::new("s", "events_2024", "o", s);
    r.partition_of = Some(PartitionOf {
        schema: "s".into(),
        name: "events".into(),
        bound: "FOR VALUES FROM ('2024-01-01') TO ('2025-01-01')".into(),
    });
    r.inherited_constraints = vec!["events_2024_pkey".into()];
    r.inherited_indexes = vec!["events_2024_pkey".into()];
    r.triggers.push(TriggerExtra { name: "t".into(), mode: TriggerMode::Origin, inherited: true, comment: None });
    let got = ddl_text(&DdlSource::Relation(Box::new(r)));
    let want = "\
-- Reconstructed by datarig from the catalog (not pg_dump)
-- Not included: rows, sequence values, rules, extended statistics, security labels

CREATE TABLE s.events_2024 PARTITION OF s.events (
    CONSTRAINT events_2024_id_check CHECK (id > 0)
) FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');

ALTER TABLE s.events_2024 OWNER TO o;
";
    assert_eq!(got, want);
}

#[test]
fn an_inheriting_table_lists_only_its_own_columns() {
    let mut s = TableStructure::new(RelationKind::Table);
    s.columns = vec![col("id", "integer", false, None), col("extra", "text", false, None)];
    let mut r = RelationDdl::new("s", "child", "o", s);
    r.columns[0].local = false;
    r.inherits = vec![("s".into(), "parent".into()), ("t".into(), "Other".into())];
    let got = ddl_text(&DdlSource::Relation(Box::new(r)));
    assert!(got.contains("CREATE TABLE s.child (\n    extra text\n)\nINHERITS (s.parent, t.\"Other\");"), "{got}");
}

#[test]
fn a_view_with_options_defaults_triggers_and_comments() {
    let mut s = TableStructure::new(RelationKind::View);
    s.columns = vec![col("id", "bigint", false, None), col("state", "text", false, Some("'new'::text"))];
    s.triggers.push(trigger(
        "summary_insert",
        "CREATE TRIGGER summary_insert INSTEAD OF INSERT ON shop.summary FOR EACH ROW EXECUTE FUNCTION shop.ins()",
    ));
    let mut r = RelationDdl::new("shop", "summary", "datarig", s);
    r.view_definition = Some(" SELECT o.id,\n    o.state\n   FROM shop.orders o;".into());
    r.options = vec!["check_option=local".into(), "security_barrier=true".into()];
    r.comment = Some("Orders in short.".into());
    r.grants = vec![grant(Some("report"), "SELECT", false), grant(Some("report"), "INSERT", false)];
    let got = ddl_text(&DdlSource::Relation(Box::new(r)));
    let want = "\
-- Reconstructed by datarig from the catalog (not pg_dump)
-- Not included: rows, sequence values, rules, extended statistics, security labels

CREATE OR REPLACE VIEW shop.summary WITH (check_option=local, security_barrier=true) AS
 SELECT o.id,
    o.state
   FROM shop.orders o;

ALTER VIEW shop.summary ALTER COLUMN state SET DEFAULT 'new'::text;

ALTER VIEW shop.summary OWNER TO datarig;

CREATE TRIGGER summary_insert INSTEAD OF INSERT ON shop.summary FOR EACH ROW EXECUTE FUNCTION shop.ins();

COMMENT ON VIEW shop.summary IS 'Orders in short.';

GRANT SELECT, INSERT ON TABLE shop.summary TO report;
";
    assert_eq!(got, want);
}

#[test]
fn a_materialized_view_with_no_data_and_its_index() {
    let mut s = TableStructure::new(RelationKind::MaterializedView);
    s.columns = vec![col("day", "date", false, None)];
    s.indexes = vec![index("daily_day", "CREATE INDEX daily_day ON a.daily USING btree (day)", false)];
    let mut r = RelationDdl::new("a", "daily", "o", s);
    r.view_definition = Some(" SELECT d.day\n   FROM a.days d;".into());
    r.index_comments = vec![Comment { name: "daily_day".into(), text: "By day.".into() }];
    let got = ddl_text(&DdlSource::Relation(Box::new(r)));
    let want = "\
-- Reconstructed by datarig from the catalog (not pg_dump)
-- Not included: rows, sequence values, rules, extended statistics, security labels

CREATE MATERIALIZED VIEW a.daily AS
 SELECT d.day
   FROM a.days d
WITH NO DATA;

ALTER MATERIALIZED VIEW a.daily OWNER TO o;

CREATE INDEX daily_day ON a.daily USING btree (day);

COMMENT ON INDEX a.daily_day IS 'By day.';
";
    assert_eq!(got, want);
}

#[test]
fn row_level_security_and_policies() {
    let mut s = TableStructure::new(RelationKind::Table);
    s.columns = vec![col("owner", "text", false, None)];
    let mut r = RelationDdl::new("s", "docs", "o", s);
    r.row_security = true;
    r.force_row_security = true;
    r.policies = vec![
        Policy {
            name: "mine".into(),
            permissive: true,
            command: None,
            roles: vec![None],
            using: Some("(owner = CURRENT_USER)".into()),
            check: None,
            comment: Some("Own rows only.".into()),
        },
        Policy {
            name: "no deletes".into(),
            permissive: false,
            command: Some("DELETE".into()),
            roles: vec![Some("app".into()), Some("Admin Role".into())],
            using: Some("false".into()),
            check: None,
            comment: None,
        },
        Policy {
            name: "insert".into(),
            permissive: true,
            command: Some("INSERT".into()),
            roles: vec![None],
            using: None,
            check: Some("(owner = CURRENT_USER)".into()),
            comment: None,
        },
    ];
    let got = ddl_text(&DdlSource::Relation(Box::new(r)));
    for line in [
        "ALTER TABLE s.docs ENABLE ROW LEVEL SECURITY;",
        "ALTER TABLE s.docs FORCE ROW LEVEL SECURITY;",
        "CREATE POLICY mine ON s.docs USING ((owner = CURRENT_USER));",
        "CREATE POLICY \"no deletes\" ON s.docs AS RESTRICTIVE FOR DELETE TO app, \"Admin Role\" USING (false);",
        "CREATE POLICY insert ON s.docs FOR INSERT WITH CHECK ((owner = CURRENT_USER));",
        "COMMENT ON POLICY mine ON s.docs IS 'Own rows only.';",
    ] {
        assert!(got.contains(&format!("\n{line}\n")), "{line}\n{got}");
    }
}

#[test]
fn a_foreign_table() {
    let mut s = TableStructure::new(RelationKind::ForeignTable);
    s.columns = vec![col("id", "integer", false, None)];
    let mut r = RelationDdl::new("s", "remote", "o", s);
    r.foreign = Some(ForeignTable { server: "files".into(), options: vec!["filename=/tmp/x.csv".into()] });
    let got = ddl_text(&DdlSource::Relation(Box::new(r)));
    assert!(
        got.contains(
            "CREATE FOREIGN TABLE s.remote (\n    id integer\n)\nSERVER files\nOPTIONS (filename '/tmp/x.csv');"
        ),
        "{got}"
    );
    assert!(got.contains("ALTER FOREIGN TABLE s.remote OWNER TO o;"), "{got}");
}

#[test]
fn disabled_and_replica_triggers_say_so() {
    let mut s = TableStructure::new(RelationKind::Table);
    s.columns = vec![col("id", "integer", false, None)];
    s.triggers = vec![trigger("a", "CREATE TRIGGER a …"), trigger("b", "CREATE TRIGGER b …")];
    let mut r = RelationDdl::new("s", "t", "o", s);
    r.triggers = vec![
        TriggerExtra { name: "a".into(), mode: TriggerMode::Disabled, inherited: false, comment: Some("off".into()) },
        TriggerExtra { name: "b".into(), mode: TriggerMode::Replica, inherited: false, comment: None },
    ];
    let got = ddl_text(&DdlSource::Relation(Box::new(r)));
    assert!(got.contains("CREATE TRIGGER a …;\n\nALTER TABLE s.t DISABLE TRIGGER a;\n"), "{got}");
    assert!(got.contains("CREATE TRIGGER b …;\n\nALTER TABLE s.t ENABLE REPLICA TRIGGER b;\n"), "{got}");
    assert!(got.contains("COMMENT ON TRIGGER a ON s.t IS 'off';"), "{got}");
}

#[test]
fn an_index_or_the_constraint_it_backs() {
    let i = IndexDdl {
        schema: "shop".into(),
        table: "users".into(),
        name: "users_created_at_idx".into(),
        definition: "CREATE INDEX users_created_at_idx ON shop.users USING btree (created_at)".into(),
        constraint: None,
        comment: Some("Recent first.".into()),
    };
    let want = "\
-- Reconstructed by datarig from the catalog (not pg_dump)

CREATE INDEX users_created_at_idx ON shop.users USING btree (created_at);

COMMENT ON INDEX shop.users_created_at_idx IS 'Recent first.';
";
    assert_eq!(ddl_text(&DdlSource::Index(i.clone())), want);
    let pk = IndexDdl {
        name: "users_pkey".into(),
        constraint: Some(("users_pkey".into(), "PRIMARY KEY (id)".into())),
        comment: None,
        ..i
    };
    let want = "\
-- Reconstructed by datarig from the catalog (not pg_dump)

ALTER TABLE ONLY shop.users ADD CONSTRAINT users_pkey PRIMARY KEY (id);
";
    assert_eq!(ddl_text(&DdlSource::Index(pk)), want);
}

#[test]
fn a_trigger_and_a_function() {
    let t = TriggerDdl {
        schema: "shop".into(),
        table: "users".into(),
        name: "users_touch".into(),
        definition: "CREATE TRIGGER users_touch BEFORE UPDATE ON shop.users FOR EACH ROW EXECUTE FUNCTION shop.touch()"
            .into(),
        mode: TriggerMode::Always,
        comment: None,
    };
    let want = "\
-- Reconstructed by datarig from the catalog (not pg_dump)

CREATE TRIGGER users_touch BEFORE UPDATE ON shop.users FOR EACH ROW EXECUTE FUNCTION shop.touch();

ALTER TABLE shop.users ENABLE ALWAYS TRIGGER users_touch;
";
    assert_eq!(ddl_text(&DdlSource::Trigger(t)), want);
    let f = FunctionDdl {
        schema: "shop".into(),
        name: "touch".into(),
        arguments: String::new(),
        procedure: false,
        owner: "datarig".into(),
        definition: "CREATE OR REPLACE FUNCTION shop.touch()\n RETURNS trigger\n LANGUAGE plpgsql\nAS $function$\
                     BEGIN RETURN NEW; END$function$\n"
            .into(),
        comment: Some("Sets updated_at.".into()),
    };
    let want = "\
-- Reconstructed by datarig from the catalog (not pg_dump)

CREATE OR REPLACE FUNCTION shop.touch()
 RETURNS trigger
 LANGUAGE plpgsql
AS $function$BEGIN RETURN NEW; END$function$;

ALTER FUNCTION shop.touch() OWNER TO datarig;

COMMENT ON FUNCTION shop.touch() IS 'Sets updated_at.';
";
    assert_eq!(ddl_text(&DdlSource::Function(f)), want);
}

#[test]
fn storage_parameters_quote_what_is_not_a_plain_value() {
    assert_eq!(option("fillfactor=70", ""), "fillfactor=70");
    assert_eq!(option("autovacuum_vacuum_scale_factor=0.05", "toast."), "toast.autovacuum_vacuum_scale_factor=0.05");
    assert_eq!(option("x=a b", ""), "x='a b'");
    assert_eq!(option("x=it's", ""), "x='it''s'");
    assert_eq!(option("x=", ""), "x=''");
}
