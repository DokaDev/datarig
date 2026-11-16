//! Every catalogue statement names its operators, functions and types with their schema
//! (see [`super::BEGIN_READ`]): none is left to the session's search path.

use super::ddl::{EMPTY_PATH, FIND_NAMED, FIND_RELATION, FIND_TRIGGER, FIND_TRIGGER_FUNCTION, SET_PATH};
use super::{
    BEGIN_READ, CATALOG, DATABASES, ENUM_CTES, KEY_COLUMNS, KEY_FOREIGN, KEY_INDEXES, SCHEMA_OBJECTS, SCHEMAS,
};
use datarig_core::sql::risk::path_names::path_names;

fn statements() -> Vec<(&'static str, String)> {
    vec![
        ("BEGIN_READ", BEGIN_READ.to_string()),
        ("SCHEMAS", SCHEMAS.to_string()),
        ("DATABASES", DATABASES.to_string()),
        ("SCHEMA_OBJECTS", SCHEMA_OBJECTS.to_string()),
        ("CATALOG", format!("{ENUM_CTES} {CATALOG}")),
        ("KEY_COLUMNS", KEY_COLUMNS.to_string()),
        ("KEY_INDEXES", KEY_INDEXES.to_string()),
        ("KEY_FOREIGN", KEY_FOREIGN.to_string()),
        ("TABLE_STRUCTURE", super::structure::SQL.to_string()),
        ("ORDER_KEYS", super::order_keys::ORDER_KEYS.to_string()),
        ("TYPE_KINDS", super::type_kinds::TYPE_KINDS.to_string()),
        ("FIND_RELATION", FIND_RELATION.to_string()),
        ("FIND_TRIGGER", FIND_TRIGGER.to_string()),
        ("FIND_TRIGGER_FUNCTION", FIND_TRIGGER_FUNCTION.to_string()),
        ("FIND_NAMED", FIND_NAMED.to_string()),
        ("SET_PATH", SET_PATH.to_string()),
        ("EMPTY_PATH", EMPTY_PATH.to_string()),
        ("DDL RELATION", super::ddl::RELATION.to_string()),
        ("DDL TRIGGER", super::ddl::TRIGGER.to_string()),
        ("DDL FUNCTION", super::ddl::FUNCTION.to_string()),
    ]
}

#[test]
fn no_catalogue_statement_leaves_a_name_to_the_search_path() {
    let mut left = Vec::new();
    for (name, sql) in statements() {
        match path_names(&sql) {
            Some(names) if names.is_empty() => {}
            Some(names) => left.push(format!("{name}: {names:?}")),
            None => left.push(format!("{name}: the parser rejects it")),
        }
    }
    assert!(left.is_empty(), "left to the search path:\n{}", left.join("\n"));
}
