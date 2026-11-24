//! How each engine's catalogue names what a result reads (`CatalogueRules`), how the DDL of its
//! objects is shown and opened in a console (`DdlRules`) and how its namespaces nest
//! (`EngineSpec::hierarchy`). No answer depends on the server's version.

use crate::Case;
use crate::Engine::{self, MariaDb, MySql, Pg};
use datarig_core::engine::Fact;

/// A question of the engine's DDL rules.
pub enum DdlQuestion {
    /// `DdlRules::index_ddl`.
    IndexDdl,
    /// `DdlRules::to_console`.
    ToConsole,
}

/// `CatalogueRules::result_tables` of engine `e`.
pub fn result_tables(e: Engine, _: &()) -> String {
    format!("{:?}", e.dialect().engine().spec().catalogue.result_tables())
}

/// The answer of engine `e`'s DDL rules to `q`, or why it has none.
pub fn ddl(e: Engine, q: &DdlQuestion) -> String {
    match &e.dialect().engine().spec().ddl {
        Fact::Known(rules) => match q {
            DdlQuestion::IndexDdl => format!("{:?}", rules.index_ddl()),
            DdlQuestion::ToConsole => format!("{:?}", rules.to_console()),
        },
        Fact::No(why) => format!("No({why:?})"),
        Fact::Unknown(why) => format!("Unknown({why:?})"),
    }
}

/// `EngineSpec::hierarchy` of engine `e`.
pub fn hierarchy(e: Engine, _: &()) -> String {
    format!("{:?}", e.dialect().engine().spec().hierarchy)
}

pub fn catalogue_cases() -> Vec<Case<()>> {
    vec![Case {
        name: "a result names its tables",
        input: (),
        expect: &[(Pg, "ByNumber"), (MySql, "ByName"), (MariaDb, "ByName")],
    }]
}

pub fn ddl_cases() -> Vec<Case<DdlQuestion>> {
    vec![
        Case {
            name: "the DDL of an index",
            input: DdlQuestion::IndexDdl,
            expect: &[(Pg, "Own"), (MySql, "Table"), (MariaDb, "Table")],
        },
        Case {
            name: "a DDL tab's text in a console",
            input: DdlQuestion::ToConsole,
            expect: &[(Pg, "AsRead"), (MySql, "Requoted"), (MariaDb, "Requoted")],
        },
    ]
}

pub fn hierarchy_cases() -> Vec<Case<()>> {
    vec![Case {
        name: "namespaces",
        input: (),
        expect: &[(Pg, "DatabaseSchema"), (MySql, "SchemaOnly"), (MariaDb, "SchemaOnly")],
    }]
}

/// What engine `e`'s filter rules answer of catalogue type `ty`: the result column it is
/// described as (`-`: none), whether it is shown in the session's zone, and the kind its
/// literals are written for (`FilterSql::catalogue_meta`, `catalogue_zoned`, `catalogue_kind`).
pub fn filter_types(e: Engine, ty: &&'static str) -> String {
    match &e.dialect().engine().spec().filter {
        Fact::Known(f) => {
            let meta = f.catalogue_meta("c", ty).map_or("-".to_string(), |m| format!("{} {:?}", m.type_name, m.kind));
            format!("{meta} | {} | {:?}", f.catalogue_zoned(ty), f.catalogue_kind(ty))
        }
        Fact::No(why) => format!("No({why:?})"),
        Fact::Unknown(why) => format!("Unknown({why:?})"),
    }
}

/// Catalogue types on each engine; the name of a case is its type.
pub fn filter_type_cases() -> Vec<Case<&'static str>> {
    vec![
        Case {
            name: "tinyint(1)",
            input: "tinyint(1)",
            expect: &[
                (Pg, "- | false | Other"),
                (MySql, "tinyint(1) Integer | false | Number"),
                (MariaDb, "tinyint(1) Integer | false | Number"),
            ],
        },
        Case {
            name: "int unsigned",
            input: "int unsigned",
            expect: &[
                (Pg, "- | false | Number"),
                (MySql, "int unsigned Integer | false | Number"),
                (MariaDb, "int unsigned Integer | false | Number"),
            ],
        },
        Case {
            name: "timestamp",
            input: "timestamp",
            expect: &[
                (Pg, "- | false | Other"),
                (MySql, "timestamp TimestampTz | true | Other"),
                (MariaDb, "timestamp TimestampTz | true | Other"),
            ],
        },
        Case {
            name: "datetime",
            input: "datetime",
            expect: &[
                (Pg, "- | false | Other"),
                (MySql, "datetime Timestamp | false | Other"),
                (MariaDb, "datetime Timestamp | false | Other"),
            ],
        },
        Case {
            name: "float",
            input: "float",
            expect: &[
                (Pg, "- | false | Other"),
                (MySql, "float Float | false | Real"),
                (MariaDb, "float Float | false | Real"),
            ],
        },
        Case {
            name: "double precision",
            input: "double precision",
            expect: &[
                (Pg, "- | false | Number"),
                (MySql, "double precision Other | false | Number"),
                (MariaDb, "double precision Other | false | Number"),
            ],
        },
        Case {
            name: "real",
            input: "real",
            expect: &[
                (Pg, "- | false | Real"),
                (MySql, "real Other | false | Number"),
                (MariaDb, "real Other | false | Number"),
            ],
        },
        Case {
            name: "bytea",
            input: "bytea",
            expect: &[
                (Pg, "- | false | Bytes"),
                (MySql, "bytea Other | false | Other"),
                (MariaDb, "bytea Other | false | Other"),
            ],
        },
        Case {
            name: "varchar(20)",
            input: "varchar(20)",
            expect: &[
                (Pg, "- | false | Text"),
                (MySql, "varchar Text | false | Text"),
                (MariaDb, "varchar Text | false | Text"),
            ],
        },
        Case {
            name: "character varying(20)",
            input: "character varying(20)",
            expect: &[
                (Pg, "- | false | Text"),
                (MySql, "character varying Other | false | Other"),
                (MariaDb, "character varying Other | false | Other"),
            ],
        },
        Case {
            name: "numeric",
            input: "numeric",
            expect: &[
                (Pg, "- | false | Number"),
                (MySql, "numeric Other | false | Number"),
                (MariaDb, "numeric Other | false | Number"),
            ],
        },
        Case {
            name: "point",
            input: "point",
            expect: &[
                (Pg, "- | false | Other"),
                (MySql, "geometry Bytes | false | Other"),
                (MariaDb, "geometry Bytes | false | Other"),
            ],
        },
        Case {
            name: "vector",
            input: "vector",
            expect: &[
                (Pg, "- | false | Other"),
                (MySql, "vector Bytes | false | Other"),
                (MariaDb, "vector Bytes | false | Other"),
            ],
        },
        Case {
            name: "text[]",
            input: "text[]",
            expect: &[
                (Pg, "- | false | Other"),
                (MySql, "text[] Other | false | Other"),
                (MariaDb, "text[] Other | false | Other"),
            ],
        },
        Case {
            name: "my_domain",
            input: "my_domain",
            expect: &[
                (Pg, "- | false | Other"),
                (MySql, "my_domain Other | false | Other"),
                (MariaDb, "my_domain Other | false | Other"),
            ],
        },
    ]
}
