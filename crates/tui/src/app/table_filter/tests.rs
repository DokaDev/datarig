use super::*;
use crate::app::filter_dialog::tests::{cursors, dialects, target, texts};

/// The completion of a condition and of an order, at every place of the cursor in any text, as
/// the filter line and the dialog's SQL fields ask for it, never panics.
#[test]
fn the_filter_completions_never_panic() {
    for text in texts() {
        for d in dialects() {
            let t = target(d);
            for cursor in cursors(&text) {
                for force in [false, true] {
                    let asked = Completion { items: Vec::new(), replace_start: cursor, trail: 0 };
                    let _ = condition_keywords(None, &text, cursor, d, force);
                    let _ = condition_keywords(Some(asked), &text, cursor, d, force);
                    let _ = order_completion(&text, cursor, &t, force);
                }
            }
        }
    }
}

/// The completion of a condition through the table tab's own entry, which completes the
/// statement the condition goes into and maps what it finds back into the condition, at every
/// place of the cursor in any text, on a table tab of a PostgreSQL, a MySQL and a MariaDB
/// profile whose session reads its text in each dialect of the family, never panics.
#[test]
fn the_where_completion_entry_never_panics() {
    use datarig_core::sql::dialect::Language;
    for driver in ["postgres", "mysql", "mariadb"] {
        let profile = datarig_core::profile::ConnectionConfig {
            name: "local".into(),
            driver: driver.into(),
            ..datarig_core::profile::ConnectionConfig::test_db()
        };
        let pid = profile.id;
        let cfg = datarig_core::config::Config { connections: vec![profile], ..Default::default() };
        let mut app = App::new(&cfg, None, datarig_i18n::Lang::En);
        app.open_table(pid, None, "shop", "users");
        let tab = app.tab().id;
        assert!(app.table_name(tab).is_some(), "{driver}: a table tab");
        let expected = app.profile(pid).map(|p| p.expected()).expect("the profile");
        for d in dialects().into_iter().filter(|d| expected.admits(d.engine())) {
            if let Some(t) = app.tabs.get_mut(tab) {
                t.exec.language = Some(Language::Sql(d));
            }
            app.sync_tab_language(tab);
            assert_eq!(app.tab_dialect(tab), d, "{driver}");
            // The entry completes there: a keyword of a condition.
            let and = app.filter_completion(tab, table_query::Field::Where, "id = 1 AN", 9, false).completion;
            let labels: Vec<String> = and.into_iter().flat_map(|c| c.items).map(|i| i.label).collect();
            assert!(labels.iter().any(|l| l == "AND"), "{driver} {d:?}: {labels:?}");
            for text in texts() {
                for cursor in cursors(&text) {
                    for force in [false, true] {
                        let _ = app.filter_completion(tab, table_query::Field::Where, &text, cursor, force);
                    }
                }
            }
        }
    }
}
