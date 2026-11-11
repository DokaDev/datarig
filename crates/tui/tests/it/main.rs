//! The TUI's integration tests as one test binary: every file of this folder is a module, so
//! the crate links once instead of once per file. Tests that need a process of their own stay
//! separate files in `tests/`: `update_quit.rs` runs its own executable again by test name, the
//! documentation checks are named by `--test` in CI and in the generated pages, and the tests
//! against real databases (`export_pg`, `integration_*`) keep their own process each, so their
//! connections never add up with another file's.
//!
//! A new file here needs its `mod` line below; [`every_file_is_a_module`] fails otherwise.

#[macro_use]
#[path = "../common/mod.rs"]
mod common;

mod app_flows;
mod flows_chart;
mod flows_context;
mod flows_ddl;
mod flows_dialog_mouse;
mod flows_edit_apply_ask;
mod flows_edit_cells;
mod flows_edit_changes;
mod flows_edit_format;
mod flows_edit_paste;
mod flows_edit_popup;
mod flows_edit_preview;
mod flows_edit_rows;
mod flows_editor;
mod flows_env_colors;
mod flows_environments;
mod flows_explorer;
mod flows_explorer_mouse;
mod flows_explorer_search;
mod flows_explorer_sections;
mod flows_export;
mod flows_export_dialog;
mod flows_export_parquet;
mod flows_export_path;
mod flows_export_xlsx;
mod flows_external;
mod flows_filter_dialog;
mod flows_filter_feedback;
mod flows_filter_history;
mod flows_filter_values;
mod flows_folder_pick;
mod flows_form_discard;
mod flows_grid;
mod flows_grid_hscroll;
mod flows_grid_search;
mod flows_grid_summary;
mod flows_hover;
mod flows_icons;
mod flows_idle;
mod flows_inspector;
mod flows_key_picker;
mod flows_keychain;
mod flows_keychain_offer;
mod flows_layout;
mod flows_manual_commit;
mod flows_menu;
mod flows_monitor;
mod flows_mouse;
mod flows_mysql;
mod flows_paging;
mod flows_password_sources;
mod flows_plan;
mod flows_plan_mysql;
mod flows_presets;
mod flows_queued;
mod flows_result_tabs;
mod flows_run_feedback;
mod flows_runs;
mod flows_safety;
mod flows_scripts;
mod flows_sessions;
mod flows_settings;
mod flows_spill;
mod flows_statement_cache;
mod flows_strip_keys;
mod flows_structure;
mod flows_tab_drag;
mod flows_tab_list;
mod flows_table_estimate;
mod flows_table_filter;
mod flows_table_filter_keep;
mod flows_table_filter_value;
mod flows_table_limit;
mod flows_table_locks;
mod flows_table_sort;
mod flows_tabs;
mod flows_themes;
mod flows_timers;
mod flows_tips;
mod flows_tls;
mod flows_tunnel;
mod flows_tx;
mod flows_tx_lifecycle;
mod flows_update;
mod flows_view_as_plan;
mod flows_whats_new;
mod flows_zoom;
mod no_emoji;
#[cfg(unix)]
mod signals;
mod ui_snapshots;

/// Every `.rs` file of this folder is declared above, so none is left out of the build.
#[test]
fn every_file_is_a_module() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it");
    let main = std::fs::read_to_string(dir.join("main.rs")).unwrap();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        if let Some(module) = name.strip_suffix(".rs").filter(|m| *m != "main") {
            assert!(
                main.lines().any(|l| l == format!("mod {module};")),
                "tests/it/{name} has no `mod {module};` in main.rs"
            );
        }
    }
}
