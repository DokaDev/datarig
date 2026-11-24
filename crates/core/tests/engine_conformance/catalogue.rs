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
