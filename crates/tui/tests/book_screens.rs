//! The screens of the docs site (`docs/site/src/screens/*.txt`) are the text of snapshot tests,
//! never drawn by hand: each is the snapshot named in [`SCREENS`] without insta's header, the
//! quotes around each row and the notes after them. A snapshot that changes makes this test fail
//! until the screens are regenerated:
//!
//! ```sh
//! DATARIG_BLESS=1 cargo test -p datarig-tui --test book_screens
//! ```
//!
//! Pages show one in a text block: ```` ```text ```` + `{{#include ../screens/<name>.txt}}`.

use std::path::PathBuf;

/// `(screen, snapshot)`: `screens/<screen>.txt` from `tests/snapshots/<snapshot>.snap`. The
/// snapshots without Nerd Font icons, so the site needs no special font.
const SCREENS: &[(&str, &str)] = &[
    ("welcome-first-steps", "flows_tips__welcome_first_steps_en_80x24"),
    ("main-layout", "ui_snapshots__main_layout_160x45_en"),
    ("editor", "ui_snapshots__current_statement_is_marked_in_the_gutter"),
    ("grid-range", "ui_snapshots__grid_range_en_160x45"),
    ("grid-keys", "ui_snapshots__grid_key_marks_text_en_160x45"),
    ("inspector", "ui_snapshots__inspector_row_en_160x45"),
    ("grid-search", "ui_snapshots__grid_search_matches_terminal_120x40"),
    ("plan-tree-postgres", "flows_plan__plan_tree_analyze_en_160x45"),
    ("plan-summary-postgres", "flows_plan__plan_summary_en_160x45"),
    ("plan-tree-mysql", "flows_plan_mysql__plan_mysql_tree_en_120x40"),
    ("explorer-tree", "ui_snapshots__explorer_tree_text_en_160x45"),
    ("explorer-structure", "flows_structure__explorer_structure_text_en_120x40"),
    ("export-dialog", "flows_export_dialog__export_dialog_console_en_160x45"),
    ("chart-bars", "flows_chart__chart_bars_en_120x34"),
    ("profile-form", "ui_snapshots__profile_form_en_160x45"),
    ("settings", "ui_snapshots__settings_theme_en_80x24"),
    ("run-confirm", "flows_safety__run_confirm_en_160x45"),
    ("tx-uncommitted", "flows_tx__tx_uncommitted_en_160x45"),
    ("session-view", "flows_sessions__session_view_en_120x40"),
];

/// What ratatui's `TestBackend` writes after a row that has wide characters: the cells they
/// hide (the row's text already leaves them out).
const HIDDEN: &str = "\" Hidden by multi-width symbols: [";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The body of an insta snapshot: what follows the YAML header between its two `---` lines.
fn body(snap: &str) -> &str {
    let rest = snap.strip_prefix("---\n").expect("the header's first ---");
    let end = rest.find("\n---\n").expect("the header's last ---");
    &rest[end + "\n---\n".len()..]
}

/// A row as ratatui's `TestBackend` writes it: its cells between `"` and `"`, as they are (no
/// escapes: a `"` in the row stays one), maybe followed by [`HIDDEN`]. `None` for any other line.
fn quoted(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('"')?;
    match rest.find(HIDDEN) {
        Some(end) => Some(&rest[..end]),
        None => rest.strip_suffix('"'),
    }
}

/// The screen a snapshot shows, each row without trailing blanks. Every row quoted (a
/// `TestBackend`), or none (a snapshot of plain text).
fn screen(snap: &str) -> String {
    let body = body(snap).trim_end_matches('\n');
    let lines: Vec<&str> = body.lines().collect();
    let rows: Vec<&str> = match lines.iter().map(|l| quoted(l)).collect::<Option<Vec<&str>>>() {
        Some(rows) => rows,
        None => {
            assert!(lines.iter().all(|l| !l.starts_with('"')), "a snapshot with quoted and plain rows");
            lines
        }
    };
    let mut out = String::new();
    for row in rows {
        out.push_str(row.trim_end());
        out.push('\n');
    }
    out
}

/// A glyph from a Nerd Font (Unicode's private use areas).
fn icon(c: char) -> bool {
    matches!(c as u32, 0xE000..=0xF8FF | 0xF0000..=0x10FFFF)
}

#[test]
fn book_screens_are_up_to_date() {
    let bless = std::env::var("DATARIG_BLESS").is_ok_and(|v| v == "1");
    let dir = root().join("docs/site/src/screens");
    let mut stale = Vec::new();
    for (name, snapshot) in SCREENS {
        let snap_path = root().join("crates/tui/tests/snapshots").join(format!("{snapshot}.snap"));
        let snap = std::fs::read_to_string(&snap_path)
            .unwrap_or_else(|e| panic!("screen {name}: snapshot {}: {e}", snap_path.display()));
        let want = screen(&snap.replace("\r\n", "\n"));
        assert!(!want.chars().any(icon), "screen {name}: {snapshot} has Nerd Font icons; take its text variant");
        let path = dir.join(format!("{name}.txt"));
        if bless {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &want).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            continue;
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
        if have != want {
            let line = have
                .lines()
                .zip(want.lines())
                .position(|(a, b)| a != b)
                .unwrap_or(have.lines().count().min(want.lines().count()));
            stale.push(format!("screens/{name}.txt (first difference at line {})", line + 1));
        }
    }
    // A screen whose snapshot is no longer listed.
    for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.unwrap().path();
        let file = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        if !SCREENS.iter().any(|(n, _)| format!("{n}.txt") == file) {
            if bless {
                std::fs::remove_file(&path).unwrap();
            } else {
                stale.push(format!("screens/{file} (not in SCREENS)"));
            }
        }
    }
    assert!(
        stale.is_empty(),
        "out of date: {stale:?}; regenerate with DATARIG_BLESS=1 cargo test -p datarig-tui --test book_screens"
    );
}

#[test]
fn snapshot_rows_are_read_back_as_drawn() {
    let snap = "---\nsource: x.rs\nassertion_line: 9\nexpression: \"a.join(\\\"\\\\n\\\")\"\n---\n\
                \"│ a \"b\" \\ ＋ │  \"\n\
                \"│ {\"k\": [\"x\"]} テスト │\" Hidden by multi-width symbols: [(3, \" \"), (5, \" \")]\n";
    assert_eq!(screen(snap), "│ a \"b\" \\ ＋ │\n│ {\"k\": [\"x\"]} テスト │\n");
    assert_eq!(screen("---\nsource: x.rs\nexpression: s\n---\n plain  \nrow\n"), " plain\nrow\n");
    assert_eq!(quoted("\"open"), None);
}
