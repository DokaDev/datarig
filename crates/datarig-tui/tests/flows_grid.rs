//! The result grid through the real `App` event path: the inspector (a panel
//! that follows the selection, or a status bar preview), the cell viewer on a double click,
//! copying cells, rows, ranges and whole results in other formats, the grid's context menu and
//! the clipboard.

mod common;

use common::*;
use datarig_core::config::DetailView;
use datarig_core::i18n::Lang;
use datarig_tui::app::overlay::OverlayKind;
use datarig_tui::app::{DetailTab, Focus};
use ratatui::crossterm::event::{Event, KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

/// The edge-case users in the grid, the grid focused.
fn grid(lang: Lang) -> Harness {
    let mut h = with_edge_results(lang);
    h.key(KeyCode::Tab);
    assert_eq!(h.app.focus, Focus::Results);
    h
}

/// The inspector's lines as drawn at 160×45.
fn inspector(h: &mut Harness) -> String {
    let t = h.draw(160, 45);
    let d = h.app.layout.detail;
    let buf = t.backend().buffer();
    (d.y..d.y + d.height)
        .map(|y| {
            let mut s = String::new();
            let mut x = d.x;
            while x < d.x + d.width {
                let sym = buf[(x, y)].symbol();
                s.push_str(sym);
                x += datarig_tui::text::width(sym).max(1) as u16;
            }
            s
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn click(h: &mut Harness, x: u16, y: u16, button: MouseButton) {
    h.app.handle_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(button),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    }));
}

#[test]
fn the_inspector_follows_the_selection() {
    let mut h = grid(Lang::En);
    assert!(h.app.detail.visible, "shown by default");
    let text = inspector(&mut h);
    // The title names the tabs, the active one in brackets, and the key that switches them.
    assert!(text.lines().next().unwrap().contains(" [Cell] Row · I "), "{text}");
    assert!(text.contains("id") && text.contains("int8 · 1 character"), "{text}");
    // Another column, another row: it follows.
    h.keys("lj");
    let text = inspector(&mut h);
    assert!(text.contains("text · 3 characters · 9 bytes") && text.contains("林美玲"), "{text}");
    // JSON is pretty-printed, NULL says so.
    h.keys("$");
    let text = inspector(&mut h);
    assert!(text.contains("jsonb") && text.contains("{}"), "{text}");
    h.keys("k");
    let text = inspector(&mut h);
    assert!(text.contains("\"lang\": \"ko\",") && text.contains("\"tags\": ["), "{text}");
    h.keys("jjh");
    let text = inspector(&mut h);
    assert!(text.contains("address") && text.contains("text · NULL"), "{text}");
    // The Row tab: every column of the selected row, the selected one highlighted.
    h.keys("I");
    assert_eq!(h.app.detail.tab, DetailTab::Row);
    let text = inspector(&mut h);
    for want in ["id: 3", "name: 張志明", "nickname: 👨‍👩‍👧‍👦 family", "address: NULL", "profile: {\"vip\": true"]
    {
        assert!(text.contains(want), "{want}:\n{text}");
    }
    // `i` hides it (the grid takes the room back), and shows it again.
    let with = h.app.layout.results.width;
    h.keys("i");
    h.draw(160, 45);
    assert!(!h.app.detail.visible && h.app.layout.detail.width == 0 && h.app.layout.results.width > with);
    h.keys("i");
    h.draw(160, 45);
    assert!(h.app.layout.detail.width > 0);
    // A click on a tab's name in the title switches to it (and focuses the inspector).
    let d = h.app.layout.detail;
    assert!(inspector(&mut h).lines().next().unwrap().contains(" Cell [Row] · I "));
    click(&mut h, d.x + 3, d.y, MouseButton::Left);
    assert_eq!(h.app.detail.tab, DetailTab::Cell);
    assert_eq!(h.app.focus, Focus::Inspector);
}

/// The inspector says how to switch its tabs, and once clicked it has keys of
/// its own: `Tab`/`Shift+Tab` (and `I`) switch, `Esc` goes back to the grid.
#[test]
fn a_clicked_inspector_switches_tabs_with_tab() {
    use datarig_tui::keymap::Ctx;
    let mut h = grid(Lang::En);
    assert!(h.status(160, 45).contains("I cell/row"), "the grid's hint line: {}", h.status(160, 45));
    let d = h.app.layout.detail;
    click(&mut h, d.x + 5, d.y + 4, MouseButton::Left);
    assert_eq!((h.app.focus, h.app.key_context()), (Focus::Inspector, Ctx::Inspector));
    let status = h.status(160, 45);
    assert!(status.contains("Tab cell/row") && status.contains("Esc back"), "{status}");
    let t = h.draw(160, 45);
    assert_eq!(t.backend().buffer()[(d.x, d.y)].fg, datarig_tui::theme::ACCENT, "its border shows the focus");
    for (key, want) in
        [(KeyCode::Tab, DetailTab::Row), (KeyCode::BackTab, DetailTab::Cell), (KeyCode::Char('I'), DetailTab::Row)]
    {
        h.key(key);
        assert_eq!(h.app.detail.tab, want, "{key:?}");
    }
    assert_eq!(h.app.focus, Focus::Inspector, "Tab stays in the inspector");
    h.key(KeyCode::Esc);
    assert_eq!(h.app.focus, Focus::Results);
    // `Space r` lists the switch too, and it runs from the grid.
    h.keys(" rI");
    assert_eq!(h.app.detail.tab, DetailTab::Cell);
    // Hidden, the inspector gives the focus back to its grid.
    click(&mut h, d.x + 5, d.y + 4, MouseButton::Left);
    h.keys("i");
    assert_eq!(h.app.focus, Focus::Results);
    assert!(!h.app.detail.visible);
}

#[test]
fn the_status_bar_preview_replaces_the_panel() {
    let mut h = grid(Lang::En);
    h.command("set detail_view=statusbar");
    assert_eq!(h.app.prefs.detail_view, DetailView::Statusbar);
    h.app.transient = None;
    h.keys("l");
    h.draw(160, 45);
    assert_eq!(h.app.layout.detail.width, 0, "no panel");
    assert!(h.status(160, 45).contains("name: 陳大文"), "{}", h.status(160, 45));
    h.keys("j");
    assert!(h.status(160, 45).contains("name: 林美玲"));
    // Tabs and newlines stay on one line.
    h.keys("Gl");
    assert!(h.status(160, 45).contains("nickname: e\u{301} combining"), "{}", h.status(160, 45));
    h.keys("l");
    assert!(h.status(160, 45).contains("address: タブ→入り住所"), "{}", h.status(160, 45));
    // Hidden with `i`; only while the grid has the focus.
    h.keys("i");
    assert!(!h.status(160, 45).contains("address:"));
    h.keys("i");
    h.key(KeyCode::Esc);
    assert!(!h.status(160, 45).contains("address:"));
}

#[test]
fn a_double_click_on_a_cell_opens_the_viewer() {
    let mut h = grid(Lang::En);
    h.draw(160, 45);
    let r = h.app.layout.results;
    // The second data row, in the name column.
    let (x, y) = (h.app.tabs.active().grid.hit_cols[1].0 + 2, r.y + 1 + 2 + 1);
    click(&mut h, x, y, MouseButton::Left);
    assert!(h.viewer().is_none(), "one click selects");
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), (1, 1));
    click(&mut h, x, y, MouseButton::Left);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::CellViewer));
    assert_eq!(h.viewer().unwrap().text, "林美玲");
    h.key(KeyCode::Esc);
    // A click on another cell right after is not a double click.
    click(&mut h, x, y + 1, MouseButton::Left);
    assert!(h.viewer().is_none());
}

// ── copying ──────────────────────────────────────────────────────────────────

/// The status line's message (between the first two `│`).
fn message(h: &mut Harness) -> String {
    h.app.transient = None;
    h.status(160, 45).split('│').nth(1).unwrap_or("").trim().to_string()
}

#[test]
fn y_copies_the_cell_and_capital_y_the_row() {
    let mut h = grid(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("jl");
    h.keys("y");
    assert_eq!(clip.last().as_deref(), Some("林美玲"), "the value as it is");
    assert_eq!(message(&mut h), "Copied the cell (system clipboard)");
    // A NULL cell copies as nothing.
    h.keys("ly");
    assert_eq!(clip.last().as_deref(), Some(""));
    // The row: every column, TSV, no header for one row (copy_header = auto).
    h.keys("G");
    h.keys("Y");
    assert_eq!(
        clip.last().as_deref(),
        Some("8\t全角 ＡＢＣ 半角 ｱｲｳ\te\u{301} combining\t\"タブ\t入り住所 (tab inside)\"\t{\"multiline\": true}")
    );
    assert_eq!(message(&mut h), "Copied 1 row as TSV (system clipboard)");
}

#[test]
fn v_selects_a_range_that_y_copies_as_tsv_with_a_header() {
    let mut h = grid(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("jl");
    h.keys("v");
    assert_eq!(h.app.tab().grid.anchor, Some((1, 1)));
    assert!(h.status(160, 45).contains("Selecting"), "{}", h.status(160, 45));
    h.keys("jjl");
    h.keys("y");
    assert_eq!(
        clip.last().as_deref(),
        Some("name\tnickname\n林美玲\t\n張志明\t👨\u{200d}👩\u{200d}👧\u{200d}👦 family\nEmoji テスト\t🔥🚀✨🎉")
    );
    assert_eq!(message(&mut h), "Copied 3 rows as TSV (system clipboard)");
    assert_eq!(h.app.tab().grid.anchor, None, "the range is done");
    // Esc drops a range before it leaves the grid; `v` again drops it too.
    h.keys("v");
    h.key(KeyCode::Esc);
    assert_eq!((h.app.tab().grid.anchor, h.app.focus), (None, Focus::Results));
    h.keys("vv");
    assert_eq!(h.app.tab().grid.anchor, None);
    // `Y` with a range: its rows, every column.
    h.keys("gg0vjY");
    assert_eq!(clip.last().unwrap().lines().count(), 3, "header and two rows");
    // copy_header = off: never a header.
    h.command("set copy_header=off");
    h.keys("gg0vjY");
    assert_eq!(clip.last().unwrap().lines().count(), 2);
}

#[test]
fn copy_as_every_format_through_keys_commands_and_the_menu() {
    let mut h = grid(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    // Space r a c: CSV of every fetched row, with its header.
    h.keys(" rac");
    let csv = clip.last().unwrap();
    assert!(csv.starts_with("id,name,nickname,address,profile\n1,陳大文,大文🐘,"), "{csv}");
    assert!(csv.contains("\n2,林美玲,,"), "NULL is an empty field: {csv}");
    assert_eq!(message(&mut h), "Copied 8 rows as CSV (system clipboard)");
    h.keys(" raj");
    let json: serde_json::Value = serde_json::from_str(&clip.last().unwrap()).unwrap();
    assert_eq!(json[0]["id"], 1);
    assert_eq!(json[0]["profile"]["tags"][1], "db", "jsonb is embedded");
    assert_eq!(json[1]["nickname"], serde_json::Value::Null);
    h.keys(" raJ");
    let pretty = clip.last().unwrap();
    assert!(pretty.starts_with("[\n  {\n    \"id\": 1,\n"), "{pretty}");
    assert_eq!(serde_json::from_str::<serde_json::Value>(&pretty).unwrap(), json, "the same JSON, indented");
    h.keys(" ram");
    assert!(clip.last().unwrap().starts_with("| id | name | nickname | address | profile |\n| ---: | --- |"));
    h.keys(" rat");
    assert!(clip.last().unwrap().starts_with("1\t陳大文\t"), "without headers");
    h.keys(" raT");
    assert!(clip.last().unwrap().starts_with("id\tname\t"), "with headers");
    h.keys(" rah");
    assert!(clip.last().unwrap().starts_with("<table>\n  <thead>\n    <tr><th>id</th>"));
    h.keys(" rax");
    assert!(clip.last().unwrap().contains("<column name=\"nickname\" null=\"true\"/>"));
    // SQL INSERT: these columns name no table, so nothing is copied and the notice says why and
    // how to name one.
    let before = clip.last();
    h.keys(" rai");
    assert_eq!(clip.last(), before, "nothing new copied");
    let status = h.status(160, 45);
    assert!(status.contains("Not copied as INSERT: a column is computed, not a column of the table"), "{status}");
    let refused = h.app.transient.as_ref().map(|(n, _)| n.render(&h.app.i18n).to_string()).unwrap_or_default();
    assert!(refused.ends_with("name the table: :copy insert <schema.table>"), "{refused}");
    // UPDATE: refused for the same reason, typed.
    h.keys(" rau");
    let status = h.status(160, 45);
    assert!(status.contains("Not copied as UPDATE: a column is computed"), "{status}");
    // IN: one column only.
    h.keys(" ran");
    assert!(h.status(160, 45).contains("Not copied as an IN list: it takes one column"));
    // The selection scope: a range, else the cell. A comma list and an IN list of one column.
    h.keys("gg0vjj");
    h.keys(" ryl");
    assert_eq!(clip.last().as_deref(), Some("1, 2, 3"));
    h.keys("gg0vjj");
    h.keys(" ryn");
    assert_eq!(clip.last().as_deref(), Some("(1, 2, 3)"));
    h.keys("gg0l");
    h.keys(" ryt");
    assert_eq!(clip.last().as_deref(), Some("陳大文"), "the cell, as it is");
    // The command line: a format, then a scope.
    h.command("copy markdown");
    assert!(clip.last().unwrap().starts_with("| id |"));
    h.command("copy list selection");
    assert_eq!(clip.last().as_deref(), Some("陳大文"));
    h.command("copy html all");
    assert!(clip.last().unwrap().contains("<td>張志明</td>"));
    h.keys(":copy js");
    assert!(h.screen(160, 45).contains("JSON (pretty)"));
    h.key(KeyCode::Enter);
    assert!(clip.last().unwrap().starts_with("[\n  {\"id\": 1"));
    h.keys(":copy xml s");
    assert!(h.screen(160, 45).contains("xml selection"), "the scopes complete: {}", h.screen(160, 45));
    h.key(KeyCode::Esc);
    h.command("copy yaml");
    assert!(h.screen(160, 45).contains("Unknown format “yaml”"));
    h.key(KeyCode::Esc);
    h.command("copy csv everything");
    assert!(h.screen(160, 45).contains("Usage: :copy <format> [selection|all]"));
    h.key(KeyCode::Esc);
    // The grid's context menu: right click on a cell selects it; the copies are two levels,
    // scope then format, with their keys.
    h.draw(160, 45);
    let (x, y) = (h.app.tabs.active().grid.hit_cols[1].0 + 2, h.app.layout.results.y + 3 + 2);
    click(&mut h, x, y, MouseButton::Right);
    assert_eq!(h.overlay_kind(), Some(OverlayKind::ContextMenu));
    assert_eq!((h.app.tab().grid.row, h.app.tab().grid.col), (2, 1));
    let screen = h.screen(160, 45);
    for want in ["view cell value", "Copy the cell", "Copy selection ▸", "Copy all fetched rows ▸"] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
    assert!(!screen.contains("Fetch every row"), "every row is fetched");
    h.keys("jjjjj");
    h.key(KeyCode::Enter);
    let screen = h.screen(160, 45);
    for want in ["Without headers (TSV)", "JSON (pretty)", "SQL IN clause", "SQL UPDATE"] {
        assert!(screen.contains(want), "{want}:\n{screen}");
    }
    // Each format with the key that runs it here (they were not shown).
    for (label, key) in [("Without headers (TSV)", 't'), ("JSON (pretty)", 'J'), ("SQL UPDATE", 'u')] {
        let line = screen.lines().find(|l| l.contains(label)).unwrap();
        let rest = line[line.find(label).unwrap() + label.len()..].trim_start();
        assert!(rest.starts_with(&format!("{key} │")), "{label}: {line}");
    }
    insta::assert_snapshot!("grid_copy_submenu_en_160x45", h.draw(160, 45).backend());
    // Esc closes the formats first; a format's letter runs it (`l`: the comma list).
    h.key(KeyCode::Esc);
    assert!(h.app.overlays.menu().is_some_and(|m| m.sub.is_none()));
    h.key(KeyCode::Right);
    h.keys("l");
    assert!(h.overlay_kind().is_none());
    assert_eq!(clip.last().unwrap().lines().count(), 1, "a comma list of every fetched row");
    // The key shown next to an item of the first level runs it.
    click(&mut h, x, y, MouseButton::Right);
    h.keys("y");
    assert_eq!(clip.last().as_deref(), Some("張志明"), "the key shown next to an item runs it");
    assert!(h.overlay_kind().is_none());
}

#[test]
fn the_clipboard_setting_picks_the_method() {
    let osc = |h: &mut Harness| h.app.take_terminal_output();
    // osc52: through the terminal only.
    let mut h = grid(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.command("set clipboard=osc52");
    h.keys("y");
    assert_eq!(osc(&mut h), ["\x1b]52;c;MQ==\x07"], "the cell `1`");
    assert!(clip.last().is_none());
    assert_eq!(message(&mut h), "Copied the cell (OSC 52)");
    // auto over SSH: OSC 52 too.
    let mut h = grid(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[("SSH_CONNECTION", "10.0.0.1 1 10.0.0.2 22")]);
    h.keys("y");
    assert_eq!(osc(&mut h).len(), 1);
    assert!(clip.last().is_none());
    // auto without a system clipboard: OSC 52; in tmux the notice says what tmux needs.
    let mut h = grid(Lang::En);
    FakeClipboard::attach(&mut h, true, &[("TMUX", "/tmp/tmux-1/default,1,0")]);
    h.keys("y");
    assert_eq!(osc(&mut h).len(), 1);
    assert_eq!(message(&mut h), "Copied the cell (OSC 52; in tmux this needs set -g set-clipboard on)");
    // system without one: an error, nothing copied.
    let mut h = grid(Lang::En);
    FakeClipboard::attach(&mut h, true, &[]);
    h.command("set clipboard=system");
    h.keys("y");
    assert!(osc(&mut h).is_empty());
    let status = h.status(160, 45);
    assert!(status.contains("Could not copy: the system clipboard is not available"), "{status}");
}

#[test]
fn a_large_copy_asks_first_and_says_only_fetched_rows_are_copied() {
    use datarig_core::driver::DbEvent;
    let mut h = Harness::connected(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.ctrl('e');
    let rows: Vec<Vec<Option<String>>> = (0..10_001).map(|i| vec![Some(i.to_string())]).collect();
    let columns = Some(vec![meta("n", "int4", true, false)]);
    h.db(DbEvent::Page { id: 1, columns, rows, more: true, elapsed: std::time::Duration::ZERO });
    h.key(KeyCode::Tab);
    h.keys(" raT");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    assert!(h.screen(160, 45).contains("Copy 10,001 rows?"));
    h.keys("n");
    assert!(clip.last().is_none(), "no: nothing copied");
    h.keys(" raT");
    h.keys("y");
    assert_eq!(clip.last().unwrap().lines().count(), 10_002, "the header and every fetched row");
    h.app.transient = None;
    let m = h.status(200, 45);
    assert!(
        m.contains(
            "Copied the 10,001 rows fetched so far as TSV with headers (system clipboard); more rows are on the server"
        ),
        "{m}"
    );
}

#[test]
fn sql_insert_names_the_source_table_when_the_keys_know_it() {
    use datarig_core::driver::DbEvent;
    let mut h = Harness::connected(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    h.app.run(vec!["SELECT user_id, status AS state FROM shop.orders WHERE id < 3".into()]);
    let columns =
        Some(vec![meta_from("user_id", "int8", true, ORDERS, 2), meta_from("state", "text", false, ORDERS, 3)]);
    let rows = vec![vec![Some("7".into()), Some("paid".into())], vec![None, Some("it's".into())]];
    h.db(DbEvent::Page { id: 1, columns, rows, more: false, elapsed: std::time::Duration::ZERO });
    h.key(KeyCode::Tab);
    h.command("copy sql");
    assert_eq!(
        clip.last().as_deref(),
        Some(
            "INSERT INTO \"shop\".\"orders\" (\"user_id\", \"status\") VALUES (7, 'paid');\n\
             INSERT INTO \"shop\".\"orders\" (\"user_id\", \"status\") VALUES (NULL, 'it''s');"
        ),
        "the table's own column name, not the alias"
    );
    assert!(!h.status(160, 45).contains("<table>"));
}

/// Run `sql` in the harness's tab and let `columns` (one row of `1`s) come back as its result,
/// the grid focused.
fn result_of(h: &mut Harness, qid: &mut u64, sql: &str, columns: Vec<datarig_core::driver::ColumnMeta>) {
    use datarig_core::driver::DbEvent;
    *qid += 1;
    h.app.run(vec![sql.to_string()]);
    let rows = vec![columns.iter().map(|_| Some("1".to_string())).collect()];
    h.db(DbEvent::Page { id: *qid, columns: Some(columns), rows, more: false, elapsed: std::time::Duration::ZERO });
    h.app.focus = Focus::Results;
}

#[test]
fn sql_insert_refuses_what_is_not_one_tables_rows() {
    use datarig_core::driver::DbEvent;
    let mut h = Harness::connected(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    let mut qid = 0;
    let mut refused = |h: &mut Harness, sql: &str, columns: Vec<datarig_core::driver::ColumnMeta>| {
        result_of(h, &mut qid, sql, columns);
        h.command("copy sql");
        h.status(200, 45)
    };
    let status = refused(
        &mut h,
        "SELECT o.id, u.id FROM shop.orders o JOIN shop.users u ON u.id = o.user_id",
        vec![meta_from("id", "int8", true, ORDERS, 1), meta_from("id", "int8", true, USERS, 1)],
    );
    assert!(
        status.contains("Not copied as INSERT: it reads more than one table (a join or a comma in FROM)"),
        "{status}"
    );
    let status = refused(
        &mut h,
        "SELECT a.id, b.status FROM shop.orders a JOIN shop.orders b ON b.user_id = a.user_id",
        vec![meta_from("id", "int8", true, ORDERS, 1), meta_from("status", "text", false, ORDERS, 3)],
    );
    assert!(status.contains("it reads more than one table"), "{status}");
    // The CTE self-join: every column names shop.orders, yet no row is a row of it.
    let status = refused(
        &mut h,
        "WITH x AS (SELECT * FROM shop.orders) SELECT a.id, b.status FROM x a JOIN x b ON a.user_id = b.id",
        vec![meta_from("id", "int8", true, ORDERS, 1), meta_from("status", "text", false, ORDERS, 3)],
    );
    assert!(status.contains("Not copied as INSERT: it uses WITH"), "{status}");
    let status = refused(
        &mut h,
        "SELECT id, id AS again FROM shop.orders",
        vec![meta_from("id", "int8", true, ORDERS, 1), meta_from("again", "int8", true, ORDERS, 1)],
    );
    assert!(status.contains("a column of the table is selected twice"), "{status}");
    let status =
        refused(&mut h, "SELECT 1 AS a, 2 AS a", vec![meta("a", "int4", true, false), meta("a", "int4", true, false)]);
    assert!(status.contains("FROM names no plain table"), "{status}");
    assert!(clip.last().is_none(), "nothing was copied");
}

#[test]
fn copy_insert_names_the_table_and_warns_when_the_rows_are_not_its_own() {
    use datarig_core::driver::DbEvent;
    let mut h = Harness::connected(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    let mut qid = 0;
    let cte = "WITH x AS (SELECT * FROM shop.orders) SELECT a.id, b.status FROM x a JOIN x b ON a.user_id = b.id";
    let columns = || vec![meta_from("id", "int8", true, ORDERS, 1), meta_from("status", "text", false, ORDERS, 3)];
    result_of(&mut h, &mut qid, cte, columns());
    // Named explicitly: copied by column name, with a warning that the rows are copied as-is.
    h.command("copy insert shop.orders");
    assert_eq!(clip.last().as_deref(), Some("INSERT INTO \"shop\".\"orders\" (\"id\", \"status\") VALUES (1, '1');"));
    let status = h.status(200, 45);
    assert!(status.contains("The rows are copied as-is; they may not exist in \"shop\".\"orders\""), "{status}");
    // A single-table SELECT of that table: no warning.
    result_of(&mut h, &mut qid, "SELECT id, status FROM shop.orders", columns());
    h.command("copy insert orders");
    assert_eq!(clip.texts.lock().unwrap().len(), 2);
    assert_eq!(message(&mut h), "Copied 1 row as SQL INSERT (system clipboard)");
    // Errors: nothing copied, the command line says why.
    for (target, want) in [
        ("shop.nope", "Not copied: no table “shop.nope” is known"),
        ("shop.users", "Not copied: shop.users has no column “status”"),
        ("a.b.c", "Not copied: no table “a.b.c” is known"),
    ] {
        h.command(&format!("copy insert {target}"));
        let status = h.status(200, 45);
        assert!(status.contains(want), "{target}: {status}");
    }
    h.command("copy insert shop.orders somewhere");
    assert!(h.status(200, 45).contains("Not copied: no table “shop.orders somewhere” is known"));
    assert_eq!(clip.texts.lock().unwrap().len(), 2, "no error copied anything");
}

#[test]
fn copy_insert_without_the_key_cache_says_the_columns_are_not_known() {
    let mut h = grid(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.command("copy insert shop.users");
    let status = h.status(200, 45);
    assert!(status.contains("Not copied: the tables' columns are not known yet, or could not be read"), "{status}");
    assert!(clip.last().is_none());
}

#[test]
fn a_copy_too_long_for_osc52_is_not_sent_and_says_so() {
    use datarig_core::driver::DbEvent;
    let osc = |h: &mut Harness| h.app.take_terminal_output();
    // About 150 KB of text: 200 KB of base64, over the default limit of 100 KB.
    let big = |h: &mut Harness| {
        h.ctrl('e');
        let rows: Vec<Vec<Option<String>>> = (0..2_000).map(|i| vec![Some(format!("{i:075}"))]).collect();
        let columns = Some(vec![meta("n", "text", false, false)]);
        h.db(DbEvent::Page { id: 1, columns, rows, more: false, elapsed: std::time::Duration::ZERO });
        h.key(KeyCode::Tab);
    };
    // clipboard = osc52: not sent, an error in the status bar, never "Copied".
    let mut h = Harness::connected(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    big(&mut h);
    h.command("set clipboard=osc52");
    h.keys(" raT");
    assert!(osc(&mut h).is_empty(), "nothing goes to the terminal");
    assert!(clip.last().is_none());
    let status = h.status(200, 45);
    assert!(status.contains("Not copied: 203 KB of base64 is more than OSC 52 may carry (100 KB"), "{status}");
    assert!(!status.contains("Copied"), "{status}");
    // A short copy still goes through OSC 52.
    h.keys("gg0y");
    assert_eq!(osc(&mut h).len(), 1);
    // auto over SSH: the system clipboard takes the long copy, and the status says so.
    let mut h = Harness::connected(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[("SSH_TTY", "/dev/pts/1")]);
    big(&mut h);
    h.keys(" raT");
    assert!(osc(&mut h).is_empty());
    assert_eq!(clip.last().map(|t| t.lines().count()), Some(2_001));
    assert_eq!(message(&mut h), "Copied 2,000 rows as TSV with headers (system clipboard)");
    // auto without a system clipboard: not sent, and said so.
    let mut h = Harness::connected(Lang::En);
    FakeClipboard::attach(&mut h, true, &[]);
    big(&mut h);
    h.keys(" raT");
    assert!(osc(&mut h).is_empty());
    assert!(h.status(200, 45).contains("Not copied:"), "{}", h.status(200, 45));
    // The limit is a setting.
    h.app.prefs.osc52_max_bytes = 4;
    h.keys("gg0y");
    assert!(osc(&mut h).is_empty(), "`0…0` is 76 characters: over 4 bytes");
    h.app.prefs.osc52_max_bytes = 1_000;
    h.keys("gg0y");
    assert_eq!(osc(&mut h).len(), 1);
}

/// When the server has more rows, the menu says the copy takes the rows
/// fetched so far and offers to fetch the rest first (asked first, with the count); the copy
/// runs once every row is there. A failed or cancelled fetch copies nothing.
#[test]
fn fetch_every_row_then_copy() {
    use datarig_core::driver::{DbCommand, DbError, DbEvent};
    let mut h = Harness::connected(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    let page = |from: usize, n: usize| (from..from + n).map(|i| vec![Some(i.to_string())]).collect::<Vec<_>>();
    let columns = Some(vec![meta("n", "int4", true, false)]);
    let ms = std::time::Duration::from_millis(1);
    h.db(DbEvent::Page { id, columns, rows: page(0, 600), more: true, elapsed: ms });
    h.key(KeyCode::Tab);
    h.keys("gg");
    h.draw(160, 45);
    let open_menu = |h: &mut Harness| {
        let (x, y) = (h.app.tab().grid.hit_cols[0].0 + 2, h.app.tab().grid.data_y);
        click(h, x, y, MouseButton::Right);
        h.screen(160, 45)
    };
    let screen = open_menu(&mut h);
    assert!(screen.contains("Copy the 600 fetched rows (the server has more) ▸"), "{screen}");
    assert!(screen.contains("Fetch every row, then copy ▸"), "{screen}");
    // The last scope: its formats, then CSV.
    h.keys("jjjjjj");
    h.key(KeyCode::Enter);
    h.keys("c");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    let screen = h.screen(160, 45);
    assert!(screen.contains("600 rows are fetched so far"), "{screen}");
    h.sent();
    h.keys("y");
    assert!(matches!(&h.sent()[..], [DbCommand::FetchMore { .. }]), "the next page");
    h.db(DbEvent::Page { id, columns: None, rows: page(600, 500), more: true, elapsed: ms });
    assert!(matches!(&h.sent()[..], [DbCommand::FetchMore { .. }]), "page after page");
    assert!(clip.last().is_none(), "not before every row is there");
    h.db(DbEvent::Page { id, columns: None, rows: page(1100, 50), more: false, elapsed: ms });
    let csv = clip.last().expect("copied");
    assert_eq!(csv.lines().count(), 1 + 1150, "the header and every row");
    assert!(csv.ends_with("\n1149"));
    assert!(message(&mut h).starts_with("Copied 1,150 rows as CSV"));
    // Complete now: no offer to fetch.
    let screen = open_menu(&mut h);
    assert!(!screen.contains("Fetch every row") && screen.contains("Copy all fetched rows ▸"), "{screen}");
    h.key(KeyCode::Esc);

    // A fetch that fails copies nothing, and says so.
    h.ctrl('e');
    let id = h.app.tab().exec.query_id;
    h.db(DbEvent::Page {
        id,
        columns: Some(vec![meta("n", "int4", true, false)]),
        rows: page(0, 10),
        more: true,
        elapsed: ms,
    });
    h.key(KeyCode::Tab);
    h.draw(160, 45);
    let copies = clip.texts.lock().unwrap().len();
    open_menu(&mut h);
    h.keys("jjjjjj");
    h.key(KeyCode::Enter);
    h.keys("t");
    h.keys("y");
    h.db(DbEvent::Failed {
        id,
        error: DbError::from("ERROR: canceling statement due to user request"),
        cancelled: true,
    });
    assert_eq!(clip.texts.lock().unwrap().len(), copies, "nothing copied");
    assert!(
        h.status(160, 45).contains("Not copied: fetching the rest of the rows did not finish"),
        "{}",
        h.status(160, 45)
    );
}

/// Repros for a copy that fetches every row first: Ctrl+C stops it (a page that
/// lands after the cancel asks for nothing more and nothing is copied), and one left from
/// before a connection switch never takes the next result.
#[test]
fn a_fetch_then_copy_stops_on_cancel_and_never_outlives_its_result() {
    use datarig_core::driver::{DbCommand, DbEvent};
    let mut h = Harness::connected(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    let page = |from: usize, n: usize| (from..from + n).map(|i| vec![Some(i.to_string())]).collect::<Vec<_>>();
    let ms = std::time::Duration::from_millis(1);
    let start = |h: &mut Harness| {
        h.key(KeyCode::Esc);
        h.app.focus = datarig_tui::app::Focus::Editor;
        h.ctrl('e');
        let id = h.app.tab().exec.query_id;
        let columns = Some(vec![meta("n", "int4", true, false)]);
        h.db(DbEvent::Page { id, columns, rows: page(0, 600), more: true, elapsed: ms });
        h.key(KeyCode::Tab);
        h.keys("gg");
        h.draw(160, 45);
        let (x, y) = (h.app.tab().grid.hit_cols[0].0 + 2, h.app.tab().grid.data_y);
        click(h, x, y, MouseButton::Right);
        h.keys("jjjjjj");
        h.key(KeyCode::Enter);
        h.keys("c");
        h.sent();
        h.keys("y");
        assert!(matches!(&h.sent()[..], [DbCommand::FetchMore { .. }]), "fetching the rest");
        id
    };
    // Ctrl+C: the page in flight lands anyway, nothing more is fetched, nothing copied.
    let id = start(&mut h);
    h.ctrl('c');
    assert!(h.session_cancelled(1), "the fetch is cancelled on the server");
    h.db(DbEvent::Page { id, columns: None, rows: page(600, 500), more: true, elapsed: ms });
    assert!(h.sent().is_empty(), "no next page");
    assert!(clip.last().is_none() && h.overlay_kind().is_none(), "nothing copied or asked");
    assert!(h.app.tab().exec.running.is_none());

    // A connection switch while it fetches: the next result is not taken.
    let _ = start(&mut h);
    h.keys(" cs");
    assert_eq!(h.overlay_kind(), Some(datarig_tui::app::overlay::OverlayKind::Confirm), "it runs: asked first");
    h.keys("y");
    h.type_text("local");
    h.key(KeyCode::Enter);
    h.key(KeyCode::Esc);
    h.app.focus = datarig_tui::app::Focus::Editor;
    h.ctrl('e');
    h.db(DbEvent::Connected);
    let id = h.app.tab().exec.query_id;
    let columns = Some(vec![meta("n", "int4", true, false)]);
    h.sent();
    h.db(DbEvent::Page { id, columns, rows: page(0, 600), more: true, elapsed: ms });
    h.draw(160, 45);
    assert!(!h.sent().iter().any(|c| matches!(c, DbCommand::FetchMore { .. })), "nothing fetched for the old copy");
    assert!(clip.last().is_none() && h.overlay_kind().is_none(), "nothing copied or asked");
}

// ── whole rows and columns ─────────────────────────────────────────

fn left_at(h: &mut Harness, kind: MouseEventKind, x: u16, y: u16, modifiers: KeyModifiers) {
    h.app.handle_event(Event::Mouse(MouseEvent { kind, column: x, row: y, modifiers }));
}

/// The first field (the id) of each line of a TSV copy, header left out.
fn ids(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| !l.starts_with("id\t") && *l != "id")
        .map(|l| l.split('\t').next().unwrap().to_string())
        .collect()
}

/// A click on a row number selects that whole row, a drag down the numbers the rows it
/// passes, Shift+click extends; `y` copies them whole as TSV, as `Y` does.
#[test]
fn the_row_number_gutter_selects_whole_rows() {
    let mut h = grid(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.draw(160, 45);
    let g = &h.app.tab().grid;
    let (gx, dy) = (g.gutter_x.0, g.data_y);
    let none = KeyModifiers::NONE;
    left_at(&mut h, MouseEventKind::Down(MouseButton::Left), gx, dy + 1, none);
    left_at(&mut h, MouseEventKind::Up(MouseButton::Left), gx, dy + 1, none);
    let g = &h.app.tab().grid;
    assert_eq!((g.anchor, g.shape, g.row), (Some((1, 0)), datarig_tui::widgets::grid::Shape::Rows, 1));
    assert_eq!(g.selection(8, 5), Some((1..2, 0..5)), "the whole row");
    h.keys("y");
    assert_eq!(ids(&clip.last().unwrap()), ["2"]);
    assert_eq!(clip.last().unwrap().split('\t').count(), 5, "every column");
    // A drag down the gutter: the rows it passes.
    left_at(&mut h, MouseEventKind::Down(MouseButton::Left), gx, dy, none);
    left_at(&mut h, MouseEventKind::Drag(MouseButton::Left), gx, dy + 2, none);
    left_at(&mut h, MouseEventKind::Up(MouseButton::Left), gx, dy + 2, none);
    h.keys("y");
    assert_eq!(ids(&clip.last().unwrap()), ["1", "2", "3"]);
    // Shift+click extends from the row clicked first.
    left_at(&mut h, MouseEventKind::Down(MouseButton::Left), gx, dy + 4, none);
    left_at(&mut h, MouseEventKind::Down(MouseButton::Left), gx, dy + 6, KeyModifiers::SHIFT);
    h.keys("Y");
    assert_eq!(ids(&clip.last().unwrap()), ["5", "6", "7"]);
    // A click on a cell drops it.
    let cx = h.app.tab().grid.hit_cols[1].0 + 2;
    left_at(&mut h, MouseEventKind::Down(MouseButton::Left), cx, dy, none);
    assert_eq!(h.app.tab().grid.anchor, None);
}

/// A click on a header selects that whole column (every fetched row), a drag across headers
/// the columns it passes. Headers had no click of their own before (no sorting).
#[test]
fn a_header_selects_whole_columns() {
    let mut h = grid(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.draw(160, 45);
    let (g, hy) = (h.app.tab().grid.hit_cols.clone(), h.app.tab().grid.header_y);
    let none = KeyModifiers::NONE;
    left_at(&mut h, MouseEventKind::Down(MouseButton::Left), g[0].0 + 2, hy, none);
    left_at(&mut h, MouseEventKind::Up(MouseButton::Left), g[0].0 + 2, hy, none);
    h.keys("y");
    let text = clip.last().unwrap();
    assert_eq!(text.lines().next(), Some("id"), "{text}");
    assert_eq!(ids(&text), ["1", "2", "3", "4", "5", "6", "7", "8"]);
    // A drag across the headers of id and name: both columns, every row.
    left_at(&mut h, MouseEventKind::Down(MouseButton::Left), g[0].0 + 2, hy + 1, none);
    left_at(&mut h, MouseEventKind::Drag(MouseButton::Left), g[1].0 + 2, hy, none);
    left_at(&mut h, MouseEventKind::Up(MouseButton::Left), g[1].0 + 2, hy, none);
    assert_eq!(h.app.tab().grid.selection(8, 5), Some((0..8, 0..2)));
    h.keys("y");
    let text = clip.last().unwrap();
    assert_eq!(text.lines().count(), 9, "header and eight rows");
    assert!(text.lines().all(|l| l.split('\t').count() == 2), "{text}");
}

/// `V` selects whole rows as vim's linewise Visual does, `j`/`k` extend it, `y` copies them
/// whole; `v` on it turns it into a range of cells (and `V` again drops it).
#[test]
fn capital_v_selects_whole_rows_that_y_copies() {
    let mut h = grid(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.keys("jll");
    h.keys("Vj");
    assert!(h.status(160, 45).contains("Selecting"));
    h.keys("y");
    let text = clip.last().unwrap();
    assert_eq!(ids(&text), ["2", "3"]);
    assert!(text.lines().skip(1).all(|l| l.split('\t').count() == 5), "whole rows: {text}");
    h.keys("Vjv");
    assert_eq!(h.app.tab().grid.shape, datarig_tui::widgets::grid::Shape::Cells);
    h.keys("V");
    assert_eq!(h.app.tab().grid.shape, datarig_tui::widgets::grid::Shape::Rows, "back to rows");
    h.keys("V");
    assert_eq!(h.app.tab().grid.anchor, None);
}

/// The wheel during a drag in the gutter goes on extending: the rows that came under the
/// pointer join the selection.
#[test]
fn the_wheel_during_a_gutter_drag_keeps_extending() {
    let mut h = Harness::connected(Lang::En);
    let cols = vec![meta("n", "int4", true, false)];
    let rows: Vec<Vec<Option<String>>> = (1..=200).map(|i| vec![Some(i.to_string())]).collect();
    h.ctrl('e');
    h.db(datarig_core::driver::DbEvent::Page {
        id: 1,
        columns: Some(cols),
        rows,
        more: false,
        elapsed: Default::default(),
    });
    h.key(KeyCode::Tab);
    h.draw(80, 24);
    let g = &h.app.tab().grid;
    let (gx, dy) = (g.gutter_x.0, g.data_y);
    let none = KeyModifiers::NONE;
    left_at(&mut h, MouseEventKind::Down(MouseButton::Left), gx, dy, none);
    left_at(&mut h, MouseEventKind::Drag(MouseButton::Left), gx, dy + 2, none);
    assert_eq!(h.app.tab().grid.selection(200, 1), Some((0..3, 0..1)));
    left_at(&mut h, MouseEventKind::ScrollDown, gx, dy + 2, none);
    let (rows, _) = h.app.tab().grid.selection(200, 1).unwrap();
    assert!(rows.start == 0 && rows.end > 3, "extended: {rows:?}");
}

/// A copy loses nothing, so `Enter` in its question copies (as `y` does); the
/// questions that would lose something keep on `Enter` (see `flows_explorer`).
#[test]
fn enter_answers_yes_to_a_large_copy() {
    use datarig_core::driver::DbEvent;
    let mut h = Harness::connected(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.ctrl('e');
    let rows: Vec<Vec<Option<String>>> = (0..10_001).map(|i| vec![Some(i.to_string())]).collect();
    let columns = Some(vec![meta("n", "int4", true, false)]);
    h.db(DbEvent::Page { id: 1, columns, rows, more: false, elapsed: std::time::Duration::ZERO });
    h.key(KeyCode::Tab);
    h.keys(" raT");
    assert_eq!(h.overlay_kind(), Some(OverlayKind::Confirm));
    assert!(h.screen(160, 45).contains("y/Enter copy · n cancel"), "{}", h.screen(160, 45));
    h.key(KeyCode::Enter);
    assert!(h.overlay_kind().is_none());
    assert_eq!(clip.last().unwrap().lines().count(), 10_002, "the header and every row");
}

/// A whole column is every fetched row; when the server has more, the copy's
/// notice says the rows not fetched yet are not in it (as the "all rows" copy does).
#[test]
fn a_whole_column_copy_says_when_rows_were_not_fetched() {
    use datarig_core::driver::DbEvent;
    let mut h = Harness::connected(Lang::En);
    let clip = FakeClipboard::attach(&mut h, false, &[]);
    h.ctrl('e');
    let rows: Vec<Vec<Option<String>>> = (1..=40).map(|i| vec![Some(i.to_string()), Some("x".into())]).collect();
    let columns = Some(vec![meta("id", "int4", true, false), meta("t", "text", false, false)]);
    h.db(DbEvent::Page { id: 1, columns, rows, more: true, elapsed: std::time::Duration::ZERO });
    h.key(KeyCode::Tab);
    h.draw(160, 45);
    let (g, hy) = (h.app.tab().grid.hit_cols.clone(), h.app.tab().grid.header_y);
    let none = KeyModifiers::NONE;
    left_at(&mut h, MouseEventKind::Down(MouseButton::Left), g[0].0 + 2, hy, none);
    left_at(&mut h, MouseEventKind::Up(MouseButton::Left), g[0].0 + 2, hy, none);
    h.keys("y");
    assert_eq!(clip.last().unwrap().lines().count(), 41, "the header and the 40 fetched rows");
    assert_eq!(
        message(&mut h),
        "Copied the 40 rows fetched so far as TSV (system clipboard); more rows are on the server"
    );
    // A range of cells is what it shows: no such note.
    h.keys("vjy");
    assert_eq!(message(&mut h), "Copied 2 rows as TSV (system clipboard)");
}

/// `V` (or `v`) again drops the selection and its "Selecting…" notice; so does
/// `Esc`.
#[test]
fn dropping_a_selection_drops_its_notice() {
    let mut h = grid(Lang::En);
    for keys in ["VV", "vv"] {
        h.keys(&keys[..1]);
        assert!(h.status(160, 45).contains("Selecting"), "{keys}");
        h.keys(&keys[1..]);
        assert_eq!(h.app.tab().grid.anchor, None);
        assert!(!h.status(160, 45).contains("Selecting"), "{keys}: {}", h.status(160, 45));
    }
    h.keys("V");
    h.key(KeyCode::Esc);
    assert_eq!((h.app.tab().grid.anchor, h.app.focus), (None, Focus::Results));
    assert!(!h.status(160, 45).contains("Selecting"), "{}", h.status(160, 45));
}
