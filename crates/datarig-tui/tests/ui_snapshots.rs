//! Headless TUI tests: the real `App` update path (`handle_event` / `on_db_event`) rendered
//! with ratatui's `TestBackend` and compared with insta snapshots. No database needed: a
//! channel-backed fake session records the commands the UI sends.

mod common;

use common::*;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_core::i18n::Lang;
use datarig_tui::app::CommandItem;
use datarig_tui::{screens, theme};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[test]
fn main_layout_160x45_en() {
    let mut h = Harness::connected(Lang::En);
    let t = h.draw(160, 45);
    insta::assert_snapshot!(t.backend());
    let buf = t.backend().buffer();
    // Whole screen painted with `bg`; focused (editor) border uses `accent` (row 1: the tab
    // bar is above it).
    assert_eq!(buf[(100, 30)].bg, theme::DARK.bg);
    assert_eq!(buf[(40, 1)].fg, theme::DARK.accent);
    assert_eq!(buf[(0, 0)].symbol(), "╭");
}

#[test]
fn main_layout_80x24() {
    for lang in [Lang::En, Lang::Ko] {
        let mut h = with_edge_results(lang);
        assert_screen!(format!("main_layout_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
    }
}

#[test]
fn too_small_terminal() {
    let mut h = Harness::connected(Lang::En);
    insta::assert_snapshot!(h.draw(79, 24).backend());
    assert!(h.screen(79, 24).contains("80×24 or larger required"));
    // The message comes from the catalog in the UI language.
    let mut h = Harness::connected(Lang::Ko);
    let too_small = datarig_core::i18n::Msg::ScreenTooSmall { width: "80".into(), height: "24".into() };
    assert!(h.screen(79, 24).contains(&ko_msg(&too_small)));
}

#[test]
fn tree_lazy_load_and_open_table() {
    let mut h = Harness::connected(Lang::En);
    h.key(KeyCode::BackTab); // editor -> explorer
    h.keys("jjjj"); // profile -> its database -> analytics -> public -> shop
    h.key(KeyCode::Char('l'));
    assert!(matches!(&h.sent()[..], [DbCommand::LoadObjects { schema }] if schema == "shop"));
    let loading = h.draw(120, 30);
    // Row 1 is "＋ New connection", row 2 the profile, row 3 its database, rows 4-6 the schemas.
    assert!(row_text(loading.backend().buffer(), 7).contains("Loading…"));
    h.db(DbEvent::Objects {
        schema: "shop".into(),
        result: Ok((
            vec!["audit_log", "order_items", "orders", "products", "reviews", "users"]
                .into_iter()
                .map(String::from)
                .collect(),
            vec!["order_summary".to_string()],
        )
            .into()),
    });
    insta::assert_snapshot!(h.draw(120, 30).backend());
    h.keys("jjjjjjj"); // Tables, audit_log .. users
    h.key(KeyCode::Enter);
    let sent = h.sent();
    assert!(
        matches!(&sent[..], [DbCommand::Execute { statements, .. }] if statements == &[r#"SELECT * FROM "shop"."users""#]),
        "{sent:?}"
    );
    // The table opens in a tab of its own: the console is left as it was.
    assert!(h.app.tab().is_table());
    assert_eq!(h.app.tab().doc.table.as_ref().map(|t| t.label()).as_deref(), Some("shop.users"));
    let console = h.app.tabs.iter().next().unwrap().editor.text();
    assert_eq!(console, SAMPLE_SQL, "opening a table must not modify the editor");
}

/// The explorer's tree with every kind of node open: a schema, both groups, tables, a view, a
/// materialized view, key columns and columns of every type category (a driver without the
/// table structure: the columns come from the catalog; `flows_structure` has the structure).
fn tree_all_kinds(icons: bool) -> Harness {
    use datarig_core::config::IconsSetting;
    use datarig_core::driver::SchemaObjects;
    use datarig_core::driver::structure::RelationStats;
    use datarig_core::sql::complete::{ColumnInfo, Relation};
    let mut h = Harness::connected(Lang::En);
    h.driver.no_structure.store(true, std::sync::atomic::Ordering::SeqCst);
    h.app.icons = if icons { IconsSetting::On } else { IconsSetting::Off };
    let mut cat = catalog();
    let col = |n: &str, t: &str| ColumnInfo { name: n.into(), type_name: t.into() };
    cat.relations.push(Relation {
        schema: "shop".into(),
        name: "zz_types".into(),
        is_view: false,
        columns: vec![
            col("t", "text"),
            col("n", "numeric(10,2)"),
            col("at", "timestamp with time zone"),
            col("ok", "boolean"),
            col("doc", "jsonb"),
            col("uid", "uuid"),
            col("raw", "bytea"),
            col("tags", "text[]"),
            col("mood", "order_status"),
        ],
    });
    h.db(DbEvent::Catalog(Ok(cat)));
    h.db(DbEvent::Keys(Ok(shop_keys())));
    h.key(KeyCode::BackTab); // editor -> explorer
    h.keys("jjjj"); // profile -> its database -> analytics -> public -> shop
    h.key(KeyCode::Char('l'));
    let objects = SchemaObjects {
        tables: vec!["users".into(), "zz_types".into()],
        views: vec!["order_summary".into(), "zz_mv".into()],
        materialized: ["zz_mv".to_string()].into(),
        // Estimates on the lines of a table and a materialized view; none yet for zz_types.
        stats: [
            ("users".to_string(), RelationStats { rows: Some(11_000), bytes: Some(4_404_019) }),
            ("zz_types".to_string(), RelationStats::default()),
            ("zz_mv".to_string(), RelationStats { rows: Some(950), bytes: Some(16_384) }),
        ]
        .into(),
    };
    h.db(DbEvent::Objects { schema: "shop".into(), result: Ok(objects) });
    h.keys("jj"); // Tables, users
    h.key(KeyCode::Char('l'));
    h.keys("jjjjjj"); // its five columns, zz_types
    h.key(KeyCode::Char('l'));
    while h.rows()[h.selected()].trim() != "zz_mv" {
        h.keys("j");
    }
    h
}

/// With icons on every node of the tree has the icon of its kind (a materialized
/// view its own), and a column that is no key the icon of its type; with icons off the tree is
/// text as before. Names stay aligned: each icon is one glyph and a space.
#[test]
fn explorer_tree_icons_on_and_off() {
    for icons in [false, true] {
        for (w, hh) in SIZES {
            let mut h = tree_all_kinds(icons);
            let name = format!("explorer_tree_{}_en_{w}x{hh}", if icons { "icons" } else { "text" });
            insta::assert_snapshot!(name, h.draw(w, hh).backend());
        }
    }
    // The screen line that shows `want` after the border and the indentation.
    let line = |h: &mut Harness, want: &str| -> String {
        let text = want.trim_start_matches(['│', ' ', '▾', '▸']);
        h.screen(160, 45).lines().find(|l| l.contains(text)).unwrap_or_default().to_string()
    };
    let mut h = tree_all_kinds(false);
    for want in [
        "│        ▸ zz_mv",
        "│        ▸ order_summary",
        "│      ▾ Views",
        "│            nickname  text",
        "│            UQ email  text",
        "│            tags  text[]",
    ] {
        assert!(line(&mut h, want).starts_with(want), "icons off: {want:?} in {:?}", line(&mut h, want));
    }
    let mut h = tree_all_kinds(true);
    for want in [
        "│    ▾ \u{f12e3} shop",
        "│      ▾ \u{f13c8} Tables",
        "│        ▾ \u{f04eb} zz_types",
        "│      ▾ \u{f06d0} Views",
        "│        ▸ \u{f1094} order_summary",
        "│        ▸ \u{f13a0} zz_mv",
        "│            \u{f084} id  bigint",
        "│            \u{ee40} email  text",
        "│            \u{f0284} nickname  text",
        "│            \u{f03a0} n  numeric(10,2)",
        "│            \u{f00f0} at  timestamp",
        "│            \u{f0a1a} ok  boolean",
        "│            \u{f0626} doc  jsonb",
        "│            \u{f0efe} uid  uuid",
        "│            \u{f12a7} raw  bytea",
        "│            \u{f016a} tags  text[]",
        "│            \u{f0832} mood  order_status",
    ] {
        assert!(line(&mut h, want).starts_with(want), "icons on: {want:?} in {:?}", line(&mut h, want));
    }
    // The icons take the tree's colors: objects in the accent, groups muted, types dim.
    let t = h.draw(160, 45);
    let buf = t.backend().buffer();
    let at = |g: &str| {
        (0..45u16)
            .flat_map(|y| (0..40u16).map(move |x| (x, y)))
            .find(|&(x, y)| buf[(x, y)].symbol() == g)
            .unwrap_or_else(|| panic!("{g:?} not drawn"))
    };
    assert_eq!(buf[at("\u{f13a0}")].fg, theme::DARK.accent);
    assert_eq!(buf[at("\u{f06d0}")].fg, theme::DARK.fg_muted);
    assert_eq!(buf[at("\u{f0284}")].fg, theme::DARK.fg_dim);
    assert_eq!(buf[at("\u{f084}")].fg, theme::DARK.key_pk);
}

#[test]
fn grid_edge_rows_align_and_truncate() {
    let mut h = with_edge_results(Lang::En);
    // The whole width for the grid (the inspector has its own tests).
    h.app.detail.visible = false;
    let t = h.draw(160, 45);
    insta::assert_snapshot!(t.backend());
    let buf = t.backend().buffer();
    let res = h.app.layout.results;
    let (header_y, first_row) = (res.y + 1, res.y + 3);
    let sep_cols = |y: u16| -> Vec<u16> {
        (res.x + 1..res.x + res.width - 1).filter(|&x| buf[(x, y)].symbol() == "│").collect()
    };
    let header = sep_cols(header_y);
    assert!(header.len() >= 5, "{header:?}");
    for y in first_row..first_row + 8 {
        assert_eq!(sep_cols(y), header, "separators misaligned on screen row {y}: {}", row_text(buf, y));
    }
    let row1 = row_text(buf, first_row);
    assert!(row1.contains('…'), "long CJK address must end with …: {row1}");
    let row2 = row_text(buf, first_row + 1);
    assert!(row2.contains("NULL"));
    let row8 = row_text(buf, first_row + 7);
    assert!(row8.contains("タブ→入り"), "tab shown as →: {row8}");
    // NULL uses null_fg + italic.
    let x = (res.x..res.x + res.width).find(|&x| buf[(x, first_row + 1)].symbol() == "N").unwrap();
    assert_eq!(buf[(x, first_row + 1)].fg, theme::DARK.null_fg);
    assert!(buf[(x, first_row + 1)].modifier.contains(ratatui::style::Modifier::ITALIC));
    assert!(h.status(160, 45).trim_end().ends_with("EN"));
}

/// Runtime language switch goes through the command line (F2 is gone).
#[test]
fn en_and_ko_ui_switch_via_commands() {
    let mut h = with_edge_results(Lang::En);
    let en = h.status(160, 45);
    assert!(en.contains("8 rows · 12ms"), "{en}");
    h.key(KeyCode::F(2));
    assert_eq!(h.app.i18n.lang, Lang::En, "F2 no longer toggles the language");
    h.command("korean");
    assert!(h.cmdline().is_none());
    assert_screen!("after_commands_language_ko", Lang::Ko, h.draw(160, 45));
    let ko = h.status(160, 45);
    assert!(ko.contains(common::ko(datarig_core::i18n::Label::LangSwitched)), "{ko}");
    // English words still find the action while the UI is Korean.
    h.command("english");
    let back = h.draw(160, 45);
    assert!(row_text(back.backend().buffer(), 0).contains("Explorer"));
}

#[test]
fn autocomplete_popup_alias_columns() {
    let mut h = Harness::connected(Lang::En);
    h.keys("Go");
    h.keys("SELECT * FROM shop.users u WHERE u.");
    assert!(h.popup().is_none(), "auto-popup is debounced");
    h.settle();
    let popup = h.popup().expect("popup opened after '.'");
    let labels: Vec<_> = popup.items.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["id", "email", "name", "nickname", "profile"]);
    insta::assert_snapshot!(h.draw(160, 45).backend());
    h.key(KeyCode::Down);
    h.key(KeyCode::Tab);
    assert!(h.popup().is_none());
    assert!(h.app.tab().editor.text().ends_with("WHERE u.email"));
}

#[test]
fn autocomplete_schema_then_tables() {
    for lang in [Lang::En, Lang::Ko] {
        autocomplete_schema_then_tables_in(lang);
    }
}

fn autocomplete_schema_then_tables_in(lang: Lang) {
    let mut h = Harness::connected(lang);
    h.keys("Go");
    h.keys("SELECT * FROM sh");
    h.settle();
    let labels: Vec<_> = h.popup().unwrap().items.iter().map(|c| c.label.clone()).collect();
    assert_eq!(labels, ["shop", "shop.orders", "shop.users"]);
    h.key(KeyCode::Esc); // closes only the popup
    assert!(h.popup().is_none());
    assert_eq!(h.app.tab().editor.mode, datarig_tui::widgets::editor::Mode::Insert);
    h.keys("op.");
    h.settle();
    assert_screen!(format!("autocomplete_schema_then_tables_{}", lang_tag(lang)), lang, h.draw(120, 30));
}

#[test]
fn cell_viewer_pretty_prints_jsonb() {
    let mut h = with_edge_results(Lang::En);
    h.key(KeyCode::Tab); // editor -> results
    h.keys("$");
    h.key(KeyCode::Enter);
    let v = h.viewer().expect("viewer open");
    assert!(v.text.contains("\n  \"lang\": \"ko\""), "{}", v.text);
    insta::assert_snapshot!(h.draw(120, 30).backend());
    h.key(KeyCode::Esc);
    assert!(h.viewer().is_none());
}

/// Marker stamped on every cell of the screen before the overlays are drawn.
const MARK: &str = "▓";

/// Draw `h` twice: once as the user sees it, and once with every cell of the screen below the
/// overlays replaced by [`MARK`] (styles kept). Returns the screen below the overlays, the
/// marked frame and the real frame.
fn draw_layers(h: &mut Harness, w: u16, hh: u16) -> (Buffer, Buffer, Buffer) {
    let mut screen = Buffer::empty(Rect::new(0, 0, w, hh));
    let mut t = Terminal::new(TestBackend::new(w, hh)).unwrap();
    t.draw(|f| {
        let area = f.area();
        f.buffer_mut().set_style(area, theme::DARK.base());
        let cursor = screens::draw_screen(f, &mut h.app);
        screen = f.buffer_mut().clone();
        for cell in &mut f.buffer_mut().content {
            cell.set_symbol(MARK);
        }
        screens::draw_overlays(f, &mut h.app, cursor);
    })
    .unwrap();
    let marked = t.backend().buffer().clone();
    let real = h.draw(w, hh).backend().buffer().clone();
    (screen, marked, real)
}

/// Guard for overlay drawing: an overlay clears only its own box (no glyph of the screen below
/// shows inside it) and the screen stays visible around it, dimmed. (An earlier fix blanked
/// the whole content area behind the cell viewer and the keyboard help instead.)
#[test]
fn overlays_clear_only_their_box_and_dim_the_screen() {
    type Open = fn() -> Harness;
    let cases: [(&str, Open); 10] = [
        ("cell viewer", || {
            let mut h = with_edge_results(Lang::En);
            h.key(KeyCode::Tab);
            h.keys("$");
            h.key(KeyCode::Enter);
            h
        }),
        ("keyboard help", || {
            let mut h = with_edge_results(Lang::En);
            h.key(KeyCode::F(1));
            h
        }),
        ("which-key", || {
            let mut h = with_edge_results(Lang::En);
            h.keys(" ");
            h.settle();
            h
        }),
        ("command line", || {
            let mut h = with_edge_results(Lang::En);
            h.ctrl('k');
            h
        }),
        ("profile form", || {
            let mut h = with_edge_results(Lang::En);
            h.keys(" cn");
            h
        }),
        ("profile list", || {
            let mut h = with_profiles(Lang::En);
            h.command("conn.manage");
            h
        }),
        ("password prompt", || {
            let mut h = launched(Lang::En, &sample_config(None));
            h.key(KeyCode::Enter);
            h.db(DbEvent::ConnectFailed { error: "password missing".into(), auth: true });
            h
        }),
        ("delete confirmation", || {
            let mut h = launched(Lang::En, &sample_config(None));
            h.keys("d");
            h
        }),
        ("quit confirmation", || {
            let mut h = with_edge_results(Lang::En);
            h.db(DbEvent::Block(true));
            h.db(DbEvent::TxOpen(true));
            h.ctrl('q');
            h
        }),
        ("keychain notice", || {
            use datarig_core::i18n::Label;
            use datarig_tui::app::overlay::{Busy, Overlay};
            let mut h = with_edge_results(Lang::En);
            h.app.overlays.push(Overlay::Busy(Busy { title: Label::MigrateTitle, text: Label::MigrateRunning }));
            h
        }),
    ];
    for (name, open) in cases {
        for (w, hh) in SIZES {
            let mut h = open();
            assert!(h.overlay_kind().is_some(), "{name}: open");
            let (screen, marked, real) = draw_layers(&mut h, w, hh);
            // The COMMAND badge of the command line stands out of the dimmed status bar, apart
            // from the command line's box.
            let badge = |p: ratatui::layout::Position| p.y + 1 == hh && real[p].bg == theme::DARK.mode_command;
            // The overlay's box: every cell the overlays drew over the marked screen.
            let drawn: Vec<(u16, u16)> = marked
                .area
                .positions()
                .filter(|&p| marked[p].symbol() != MARK && !badge(p))
                .map(|p| (p.x, p.y))
                .collect();
            assert!(!drawn.is_empty(), "{name} {w}x{hh}: nothing drawn");
            let (x0, x1) = (drawn.iter().map(|p| p.0).min().unwrap(), drawn.iter().map(|p| p.0).max().unwrap());
            let (y0, y1) = (drawn.iter().map(|p| p.1).min().unwrap(), drawn.iter().map(|p| p.1).max().unwrap());
            let rect = Rect::new(x0, y0, x1 - x0 + 1, y1 - y0 + 1);
            let visible = (w as usize * hh as usize) - rect.area() as usize;
            assert!(
                visible * 10 >= w as usize * hh as usize,
                "{name} {w}x{hh}: the box {rect:?} hides (almost) the whole screen"
            );
            assert_eq!(
                marked.area.positions().any(badge),
                h.app.overlays.is_open(datarig_tui::app::overlay::OverlayKind::Commands),
                "{name} {w}x{hh}: the COMMAND badge"
            );
            for p in marked.area.positions() {
                let (m, s, r) = (&marked[p], &screen[p], &real[p]);
                if badge(p) {
                    continue;
                }
                if rect.contains(p) {
                    // (1) Nothing of the screen below shows inside the box.
                    assert_ne!(m.symbol(), MARK, "{name} {w}x{hh}: screen glyph inside the box at {p:?}");
                    continue;
                }
                // (2) Outside the box the screen keeps its symbols, dimmed.
                assert_eq!(
                    (m.fg, m.bg),
                    (theme::DARK.dim_fg(s.fg), theme::DARK.dim_bg(s.bg)),
                    "{name} {w}x{hh}: not dimmed at {p:?}"
                );
                // A wide glyph straddling the box's left border is blanked on purpose.
                let straddles = p.x + 1 == rect.x && datarig_tui::text::width(s.symbol()) > 1;
                assert!(
                    r.symbol() == s.symbol() || straddles,
                    "{name} {w}x{hh}: {:?} became {:?} at {p:?}",
                    s.symbol(),
                    r.symbol()
                );
            }
        }
    }
}

#[test]
fn paging_request_on_the_next_page_key() {
    let mut h = Harness::connected(Lang::En);
    h.ctrl('e');
    h.sent();
    let cols = vec![meta("id", "int8", true, false)];
    let page = |from: usize| (from..from + 500).map(|i| vec![Some(i.to_string())]).collect::<Vec<_>>();
    h.db(DbEvent::TxOpen(true));
    h.db(DbEvent::Page { id: 1, columns: Some(cols), rows: page(0), more: true, elapsed: Duration::from_millis(5) });
    // The transaction that holds the portal is the app's, not the user's.
    assert!(!h.status(160, 45).contains("TX open"));
    h.key(KeyCode::Tab);
    h.keys("jjj");
    // Pages are explicit, the end of one fetches nothing.
    h.keys("G");
    assert!(h.sent().is_empty(), "no fetch by scrolling");
    assert!(h.status(160, 45).contains("End of the page: n shows the next one"));
    h.app.transient = None;
    h.keys("n");
    assert!(matches!(&h.sent()[..], [DbCommand::FetchMore { id: 1 }]));
    assert!(h.status(160, 45).contains("Fetching more rows"));
    h.keys("k"); // scrolling is not blocked while fetching
    h.db(DbEvent::Page { id: 1, columns: None, rows: page(500), more: false, elapsed: Duration::from_millis(3) });
    assert_eq!(h.app.tab().grid.window(1000), 500..1000, "the page asked for is shown");
    h.db(DbEvent::TxOpen(false));
    let st = h.status(160, 45);
    assert!(st.contains("1,000 rows") && !st.contains("TX open"), "{st}");
}

#[test]
fn cancel_and_busy_through_key_path() {
    let mut h = Harness::connected(Lang::Ko);
    h.ctrl('c'); // idle: ignored, app keeps running
    assert!(!h.app.quit && !h.cancelled.load(Ordering::SeqCst));
    h.ctrl('e');
    h.ctrl('e');
    assert!(h.status(160, 45).contains(&ko_msg(&datarig_core::i18n::Msg::QueryBusy { key: "Ctrl+C".into() })));
    h.ctrl('c');
    assert!(h.cancelled.load(Ordering::SeqCst), "CancelRequest sent through the session");
    h.db(DbEvent::Failed { id: 1, error: "canceling statement due to user request".into(), cancelled: true });
    assert!(h.status(160, 45).contains(ko(datarig_core::i18n::Label::QueryCancelled)));
    assert_eq!(h.sent().len(), 1, "second Ctrl+E while running must not send");
}

#[test]
fn visual_selection_runs_both_statements() {
    let mut h = Harness::connected(Lang::En);
    h.keys("vjjjjj$");
    h.ctrl('e');
    let sent = h.sent();
    let [DbCommand::Execute { statements, .. }] = &sent[..] else { panic!("{sent:?}") };
    assert_eq!(statements.len(), 2);
    assert!(statements[1].starts_with("SELECT u.name"));
    assert_eq!(h.app.tab().editor.mode, datarig_tui::widgets::editor::Mode::Normal);
}

// ── iteration 2: command line, profiles, test connection ─────────────────────────

const SIZES: [(u16, u16); 2] = [(80, 24), (160, 45)];

/// Split a list of `en …` / `ko …` lines: the English ones are returned, to be pinned; each
/// Korean line must be a translation of the English line at the same place. The Korean text
/// itself is the catalog's (`locales/ko.toml`) and is not kept in the snapshots.
fn english_pinned(out: &str) -> String {
    let (en, ko): (Vec<&str>, Vec<&str>) = out.lines().partition(|l| l.starts_with("en "));
    assert_eq!(en.len(), ko.len(), "{out}");
    for (e, k) in en.iter().zip(&ko) {
        let (e, k) = (&e[3..], k.strip_prefix("ko ").expect("a Korean line"));
        assert!(e != k && !k.is_ascii(), "not translated: {k}");
    }
    en.iter().map(|l| format!("{l}\n")).collect()
}

fn lang_tag(l: Lang) -> &'static str {
    if l == Lang::En { "en" } else { "ko" }
}

/// Test DB profile plus two more (CJK name, IPv6 host).
fn with_profiles(lang: Lang) -> Harness {
    let mut h = Harness::connected(lang);
    let base = datarig_core::profile::ConnectionConfig::test_db();
    h.app.profiles.push(datarig_core::profile::ConnectionConfig {
        name: "分析-replica".into(),
        host: "analytics.internal".into(),
        port: 6432,
        user: "report".into(),
        password: String::new(),
        database: "warehouse".into(),
        sslmode: "require".into(),
        ..base.clone()
    });
    h.app.profiles.push(datarig_core::profile::ConnectionConfig {
        name: "v6".into(),
        host: "::1".into(),
        port: 5432,
        user: "postgres".into(),
        password: String::new(),
        database: "postgres".into(),
        ..base
    });
    h
}

#[test]
fn command_line_en_ko_sizes() {
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            // The default: a popup near the top, the input on its first line, the entries below.
            let mut h = Harness::connected(lang);
            h.keys(":");
            assert_screen!(format!("commands_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
            let t = h.draw(w, hh);
            let input = row_text(t.backend().buffer(), hh / 6 + 1);
            assert!(input.contains("│ : "), "{input}");
            assert!(!row_text(t.backend().buffer(), hh - 1).starts_with(':'), "the status bar stays");
            // `commands.position = bottom`: Neovim style, the `:` prompt on the last row after
            // the COMMAND badge.
            h.app.prefs.commands_position = datarig_core::config::CommandsPosition::Bottom;
            assert_screen!(format!("commands_bottom_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
            let t = h.draw(w, hh);
            let last = row_text(t.backend().buffer(), hh - 1);
            let badge = format!(
                " {}  : ",
                datarig_core::i18n::I18n::new(lang).label(datarig_core::i18n::Label::StatusModeCommand)
            );
            assert!(last.starts_with(&badge), "{last}");
            assert_eq!(t.backend().buffer()[(0, hh - 1)].bg, theme::DARK.mode_command);
        }
    }
    // Filtering: fuzzy query narrows and ranks the list.
    let mut h = Harness::connected(Lang::En);
    h.ctrl('k');
    h.type_text("tst con");
    let p = h.cmdline().unwrap();
    assert_eq!(p.items[0], CommandItem::Action(action_index("conn.test")));
    insta::assert_snapshot!("commands_filtered_en_80x24", h.draw(80, 24).backend());
    h.type_text("zzz");
    assert!(h.cmdline().unwrap().items.is_empty());
    assert!(h.screen(80, 24).contains("No matching commands"));
    h.key(KeyCode::Esc);
    assert!(h.cmdline().is_none());
}

/// Argument completion (profiles, settings) and the inline error of a command that cannot run.
#[test]
fn command_line_arguments_and_errors() {
    for lang in [Lang::En, Lang::Ko] {
        let mut h = with_profiles(lang);
        h.keys(":conn ");
        assert_screen!(format!("commands_conn_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
        let mut h = with_profiles(lang);
        h.keys(":set language=");
        assert_screen!(format!("commands_set_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
        // The `icons` setting: its values, its description and a preview of the glyphs.
        let mut h = with_profiles(lang);
        h.keys(":set icons=");
        assert_screen!(format!("commands_set_icons_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
        let mut h = with_profiles(lang);
        h.keys(":frobz");
        h.key(KeyCode::Enter);
        assert!(h.cmdline().is_some_and(|c| c.error.is_some()), "the command line stays, with the error");
        for (w, hh) in SIZES {
            assert_screen!(format!("commands_error_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
    }
}

/// Profiles in nested folders with every node state: connected with its schema tree open,
/// connecting, failed (with its error line) and not connected; icons on and off.
fn explorer_states(lang: Lang, icons: bool) -> Harness {
    use datarig_core::config::IconsSetting;
    use datarig_core::profile::ConnectionConfig;
    let mut h = Harness::connected(lang);
    h.db(DbEvent::Schemas(Ok(vec!["analytics".into(), "public".into(), "shop".into()])));
    let base = ConnectionConfig::test_db();
    let add = |h: &mut Harness, name: &str, folder: Option<&str>, color: Option<&str>, icon: Option<&str>| {
        let p = ConnectionConfig {
            name: name.into(),
            folder: folder.map(String::from),
            color: color.map(String::from),
            icon: icon.map(String::from),
            ..ConnectionConfig { id: datarig_core::profile::ProfileId::new(), ..base.clone() }
        };
        if let Some(f) = p.folder_path() {
            h.app.folders.insert(&f);
        }
        h.app.profiles.push(p);
    };
    add(&mut h, "orders-prod", Some("work/prod"), Some("red"), Some("fire"));
    add(&mut h, "orders-stage", Some("work"), Some("yellow"), None);
    add(&mut h, "分析-replica", Some("work/prod"), None, Some("chart"));
    add(&mut h, "scratch", Some("local"), Some("#8c8cf0"), None);
    for f in ["work", "work/prod", "local"] {
        let f = datarig_core::profile::folder::FolderPath::parse(f).unwrap();
        h.app.folders.toggle(&f);
    }
    // orders-prod connecting, 分析-replica failed.
    h.explore("orders-prod");
    h.key(KeyCode::Enter);
    h.explore("分析-replica");
    h.key(KeyCode::Enter);
    h.meta_db(
        "分析-replica",
        DbEvent::ConnectFailed { error: "password authentication failed for user \"report\"".into(), auth: false },
    );
    h.explore("orders-prod");
    h.app.icons = if icons { IconsSetting::On } else { IconsSetting::Off };
    h
}

#[test]
fn explorer_folders_states_and_icons() {
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            let mut h = explorer_states(lang, false);
            assert_screen!(format!("explorer_states_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
    }
    let mut h = explorer_states(Lang::En, true);
    insta::assert_snapshot!("explorer_states_icons_en_160x45", h.draw(160, 45).backend());
    let rows = h.rows();
    assert_eq!(
        rows,
        [
            "+",
            "local/",
            "  scratch",
            "work/",
            "  prod/",
            "    orders-prod",
            "    分析-replica",
            "      !",
            "  orders-stage",
            "local-pg",
            "  db:datarig*",
            "    analytics",
            "    public",
            "    shop",
        ],
        "folders first, then profiles, each by name; a connected one's own database first"
    );
}

/// Quick connect (`Ctrl+O`): every profile with its state and folder; typing narrows the list.
#[test]
fn quick_connect_en_ko() {
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            let mut h = explorer_states(lang, false);
            h.ctrl('o');
            assert_screen!(format!("quick_connect_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
    }
    let mut h = explorer_states(Lang::En, true);
    h.ctrl('o');
    h.type_text("ord");
    let mut t = h.draw(80, 24);
    insta::assert_snapshot!("quick_connect_filtered_en_80x24", t.backend());
    let c = t.get_cursor_position().unwrap();
    assert!(row_text(t.backend().buffer(), c.y).contains("ord"), "the cursor is in the input");
}

#[test]
fn profile_form_en_ko_sizes() {
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            let mut h = with_profiles(lang);
            h.explore("分析-replica");
            h.keys("e");
            let t = h.draw(w, hh);
            assert_screen!(format!("profile_form_{}_{w}x{hh}", lang_tag(lang)), lang, &t);
            // Hardware cursor sits in the focused (name) input, after the text.
            let (cx, cy) = {
                let mut t = t;
                let p = t.get_cursor_position().unwrap();
                (p.x, p.y)
            };
            let row = row_text(h.draw(w, hh).backend().buffer(), cy);
            assert!(row.contains("分析-replica"), "{row}");
            assert!(cx > 0);
        }
    }
}

/// The form's advanced section (SSL mode, policy, color, icon, folder), the icon list and the
/// name input of a new folder.
/// The SSH section: a tunnel with a PEM key, its field hints, at both sizes.
#[test]
fn profile_form_ssh_section() {
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            let mut h = with_profiles(lang);
            h.explore("local-pg");
            h.keys("e");
            h.ctrl('n');
            h.key(KeyCode::Char(' '));
            h.key(KeyCode::Tab);
            h.type_text("bastion.example.com");
            h.key(KeyCode::Tab);
            h.key(KeyCode::Tab);
            h.type_text("ec2-user");
            h.key(KeyCode::Tab);
            h.key(KeyCode::Tab);
            h.type_text("~/.ssh/prod.pem");
            assert_screen!(format!("profile_form_ssh_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
    }
}

#[test]
fn profile_form_advanced_section_and_pickers() {
    use datarig_core::config::IconsSetting;
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            let mut h = with_profiles(lang);
            h.app.folders.insert(&datarig_core::profile::folder::FolderPath::parse("work/prod").unwrap());
            h.explore("分析-replica");
            h.keys("e");
            h.ctrl('p');
            h.key(KeyCode::Tab);
            h.key(KeyCode::Right);
            h.key(KeyCode::Tab);
            h.type_text("prod-rules");
            h.key(KeyCode::Tab);
            h.key(KeyCode::Right);
            h.key(KeyCode::Tab);
            h.key(KeyCode::Tab);
            h.key(KeyCode::Right);
            h.key(KeyCode::Right);
            assert_screen!(format!("profile_form_advanced_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
    }
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            let mut h = with_profiles(lang);
            h.app.icons = IconsSetting::On;
            h.explore("local-pg");
            h.keys("e");
            h.ctrl('p');
            for _ in 0..4 {
                h.key(KeyCode::Tab);
            }
            h.key(KeyCode::Enter);
            assert_screen!(format!("chooser_icon_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
    }
    for lang in [Lang::En, Lang::Ko] {
        let mut h = with_profiles(lang);
        h.key(KeyCode::BackTab);
        h.keys("N");
        h.type_text("本番");
        assert_screen!(format!("name_input_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
    }
}

#[test]
fn profile_form_validation_and_dsn_error_snapshot() {
    let mut h = with_profiles(Lang::En);
    h.command("new connection");
    h.ctrl('s'); // attempt save with empty required fields
    for _ in 0..7 {
        h.key(KeyCode::Tab); // to DSN (past the password storage selector and the password)
    }
    h.ctrl('u');
    h.type_text("postgres://h/db?application_name=x");
    let t = h.draw(80, 24);
    insta::assert_snapshot!("profile_form_errors_en_80x24", t.backend());
    let screen: String = (0..24).map(|y| row_text(t.backend().buffer(), y)).collect::<Vec<_>>().join("\n");
    assert!(screen.contains("required"), "{screen}");
    assert!(screen.contains("unsupported parameter"), "{screen}");
}

#[test]
fn test_connection_success_and_error_states() {
    use datarig_core::driver::{PingError, PingInfo};
    for lang in [Lang::En, Lang::Ko] {
        let mut h = with_profiles(lang);
        h.explore("local-pg");
        h.keys("e");
        h.ctrl('t');
        let running = h.draw(80, 24);
        let screen: String = (0..24).map(|y| row_text(running.backend().buffer(), y)).collect::<Vec<_>>().join("\n");
        let testing = datarig_core::i18n::I18n::new(lang)
            .msg(&datarig_core::i18n::Msg::TestRunning { elapsed: Duration::ZERO })
            .to_string();
        let testing = testing.split('…').next().unwrap();
        assert!(screen.contains(testing), "{testing:?} in {screen}");
        h.ping(Ok(PingInfo { server_version: "17.2".into(), latency: Duration::from_millis(12) }));
        assert_screen!(format!("test_connection_ok_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
        h.ctrl('t');
        h.ping(Err(PingError::Failed("error connecting to server: Connection refused (os error 61)".into())));
        assert_screen!(format!("test_connection_error_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
    }
}

// ── startup: the explorer, the welcome panel ────────────────────────

fn launched(lang: Lang, cfg: &datarig_core::config::Config) -> Harness {
    use std::sync::Arc;
    Harness::launched(cfg, lang, Arc::new(datarig_core::secret::MemoryStore::new()), datarig_tui::app::Startup::Normal)
}

#[test]
fn explorer_at_launch_en_ko_sizes() {
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            let mut h = launched(lang, &sample_config(Some("分析-replica")));
            let t = h.draw(w, hh);
            assert_screen!(format!("explorer_launch_{}_{w}x{hh}", lang_tag(lang)), lang, &t);
            // The cursor is on the last used profile (row 3: "＋", local-pg, v6, 分析-replica).
            let area = h.app.explorer.area;
            let buf = t.backend().buffer();
            assert_eq!(buf[(area.x + 3, area.y + 3)].bg, theme::DARK.selection.bg.unwrap());
            assert!(row_text(buf, area.y + 3).contains("分析-replica"));
        }
    }
    // `/` filters by name and puts the hardware cursor in the filter input.
    let mut h = launched(Lang::En, &sample_config(None));
    h.keys("/");
    h.type_text("v6");
    let mut t = h.draw(80, 24);
    insta::assert_snapshot!("explorer_filtered_en_80x24", t.backend());
    let c = t.get_cursor_position().unwrap();
    assert_eq!((c.x, c.y), (5, 1));
}

#[test]
fn welcome_panel_en_ko() {
    for lang in [Lang::En, Lang::Ko] {
        let cfg = datarig_core::config::Config { connections: Vec::new(), ..Default::default() };
        let mut h = launched(lang, &cfg);
        assert_screen!(format!("welcome_form_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
        h.key(KeyCode::Esc);
        for (w, hh) in SIZES {
            assert_screen!(format!("welcome_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
        // The name as text, never an emoji or a glyph, icons on or off.
        assert!(h.screen(80, 24).contains("  datarig  "));
        h.app.icons = datarig_core::config::IconsSetting::On;
        assert!(h.screen(80, 24).contains("  datarig  "));
    }
}

#[test]
fn explorer_connect_error_on_the_node() {
    for lang in [Lang::En, Lang::Ko] {
        let mut h = launched(lang, &sample_config(None));
        h.key(KeyCode::Enter);
        h.db(DbEvent::ConnectFailed { error: refused(), auth: false });
        for (w, hh) in SIZES {
            assert_screen!(format!("explorer_error_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
    }
}

/// `datarig <profile>` with an unknown name: the workspace opens with the error in the results
/// pane.
#[test]
fn launch_with_an_unknown_profile() {
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            use std::sync::Arc;
            let mut h = Harness::launched(
                &sample_config(None),
                lang,
                Arc::new(datarig_core::secret::MemoryStore::new()),
                datarig_tui::app::Startup::Profile("nope".into()),
            );
            assert_screen!(format!("launch_unknown_profile_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
    }
}

#[test]
fn password_prompt_en_ko() {
    for lang in [Lang::En, Lang::Ko] {
        let mut h = launched(lang, &sample_config(None));
        h.key(KeyCode::Enter);
        h.db(DbEvent::ConnectFailed { error: "password missing".into(), auth: true });
        h.type_text("秘密");
        let mut t = h.draw(80, 24);
        assert_screen!(format!("password_prompt_{}_80x24", lang_tag(lang)), lang, &t);
        // Hardware cursor after the two masked graphemes (IME preedit shows there).
        let c = t.get_cursor_position().unwrap();
        let row = row_text(t.backend().buffer(), c.y);
        assert!(row.contains("••"), "{row}");
        h.key(KeyCode::Tab);
        let mut t = h.draw(80, 24);
        let c2 = t.get_cursor_position().unwrap();
        assert_eq!(c2.y, c.y + 2, "Tab moves the focus to the keychain checkbox");
    }
}

#[test]
fn quit_confirmation_en_ko() {
    for lang in [Lang::En, Lang::Ko] {
        // (No running query here: its elapsed time in the status bar would change the snapshot.)
        let mut h = Harness::connected(lang);
        h.db(DbEvent::Block(true));
        h.db(DbEvent::TxOpen(true));
        h.ctrl('q');
        assert_screen!(format!("quit_confirmation_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
    }
}

#[test]
fn keychain_move_notice_en_ko() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _in_runtime = rt.enter();
    let body = "[[connections]]\nname = \"local-pg\"\nhost = \"127.0.0.1\"\nport = 55432\nuser = \"datarig\"\n\
                password = \"datarig\"\ndatabase = \"datarig\"\n";
    for lang in [Lang::En, Lang::Ko] {
        // A file per run, each dropped after its store opens and its move rewrote it.
        let scratch = MovingConfig::new(&format!("snap-move-{}", lang_tag(lang)), body);
        let (cfg, _) = datarig_core::config::load(Some(scratch.path.clone()));
        let store = std::sync::Arc::new(GatedStore::default());
        let _open_at_end = store.open_on_drop();
        let (mut h, _rx) = Harness::started(
            &cfg,
            lang,
            store.clone() as std::sync::Arc<dyn datarig_core::secret::SecretStore>,
            datarig_tui::app::Startup::Normal,
        );
        assert_screen!(format!("keychain_move_notice_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
    }
}

// ── key guide: which-key, keyboard help, hint line ────────────────

#[test]
fn which_key_en_ko_sizes() {
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            let mut h = Harness::connected(lang);
            h.keys(" ");
            h.settle();
            assert_screen!(format!("which_key_root_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
            // A group: the results' keys (the `Space , l` group became the settings screen).
            h.keys("r");
            assert_screen!(format!("which_key_results_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
    }
}

#[test]
fn keyboard_help_en_ko_sizes() {
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            let mut h = Harness::connected(lang);
            h.key(KeyCode::BackTab);
            h.key(KeyCode::F(1));
            assert_screen!(format!("help_explorer_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
        // vim Normal: Space ? (the editor keeps `?`); the same list, other sections closed below.
        let mut h = Harness::connected(lang);
        h.keys(" ?");
        // Its section starts with where typing starts and stops.
        assert_screen!(format!("help_editor_normal_top_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
        h.key(KeyCode::PageDown);
        h.key(KeyCode::PageDown);
        assert_screen!(format!("help_editor_normal_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
    }
    // The search covers every context.
    let mut h = Harness::connected(Lang::En);
    h.keys(" ?/results");
    insta::assert_snapshot!("help_search_en_80x24", h.draw(80, 24).backend());
}

#[test]
fn hint_line_per_context() {
    let mut out = String::new();
    for lang in [Lang::En, Lang::Ko] {
        for kitty in [false, true] {
            let mut h = with_edge_results(lang);
            h.app.set_keyboard_enhanced(kitty);
            let mut line = |h: &mut Harness, name: &str| {
                for w in [160, 80] {
                    let s = h.status(w, if w == 160 { 45 } else { 24 });
                    out.push_str(&format!("{} kitty={kitty} {name:<14} {w:>3}: {}\n", lang_tag(lang), s.trim_end()));
                }
            };
            line(&mut h, "editor.normal");
            h.keys("v");
            line(&mut h, "editor.visual");
            h.key(KeyCode::Esc);
            h.keys("i");
            line(&mut h, "editor.insert");
            h.key(KeyCode::Esc);
            h.key(KeyCode::Tab);
            line(&mut h, "grid");
            h.key(KeyCode::Enter);
            line(&mut h, "cell_viewer");
            h.key(KeyCode::Esc);
            h.key(KeyCode::Tab);
            line(&mut h, "explorer");
        }
    }
    insta::assert_snapshot!("hint_lines", english_pinned(&out));
}

fn action_index(id: &str) -> usize {
    datarig_tui::app::action::REGISTRY.iter().position(|s| s.id == id).expect("registered")
}

// ── password sources ────────────────────────────────────────────

/// The new-profile form with the storage selector on `kind` (and its field filled in).
fn form_with_source(lang: Lang, kind: datarig_core::secret::SourceKind) -> Harness {
    use datarig_core::secret::SourceKind;
    let mut h = with_profiles(lang);
    h.command("conn.new");
    h.type_text("prod");
    for _ in 0..4 {
        h.key(KeyCode::Tab); // to the storage selector
    }
    while h.form().source != kind {
        h.key(KeyCode::Right);
    }
    match kind {
        SourceKind::Command => {
            h.key(KeyCode::Tab);
            h.type_text("op read op://work/prod-db/password");
        }
        SourceKind::Env => {
            h.key(KeyCode::Tab);
            h.type_text("PROD_DB_PASSWORD");
        }
        SourceKind::Keychain | SourceKind::File => {
            h.key(KeyCode::Tab);
            h.type_text("s3cret");
        }
        SourceKind::Prompt => {}
    }
    h
}

#[test]
fn profile_form_per_password_source() {
    use datarig_core::secret::SourceKind;
    for lang in [Lang::En, Lang::Ko] {
        for kind in SourceKind::ALL {
            for (w, hh) in SIZES {
                let mut h = form_with_source(lang, kind);
                let name = format!("profile_form_source_{}_{}_{w}x{hh}", kind.as_str(), lang_tag(lang));
                assert_screen!(name, lang, h.draw(w, hh));
            }
        }
    }
    // The password never shows; the command and the variable name do.
    let mut h = form_with_source(Lang::En, SourceKind::File);
    let screen = h.screen(80, 24);
    assert!(!screen.contains("s3cret") && screen.contains("kept in secrets.toml (0600)"), "{screen}");
    let mut h = form_with_source(Lang::En, SourceKind::Command);
    assert!(h.screen(160, 45).contains("op://work/prod-db/password"));
}

#[test]
fn profile_form_without_a_keychain() {
    for lang in [Lang::En, Lang::Ko] {
        let mut h = with_profiles(lang);
        h.app.secrets.unavailable = Some(datarig_core::fault::Fault::new(
            datarig_core::fault::FaultKind::Keychain(datarig_core::fault::KeychainFault::NoStore),
            "no secret service",
        ));
        h.command("conn.new");
        assert_screen!(format!("profile_form_no_keychain_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
    }
}

#[test]
fn source_change_confirmation_and_prompt_source() {
    use datarig_core::secret::SourceKind;
    for lang in [Lang::En, Lang::Ko] {
        let mut h = with_profiles(lang);
        h.explore("local-pg");
        h.keys("e");
        for _ in 0..4 {
            h.key(KeyCode::Tab);
        }
        h.key(KeyCode::Right); // keychain -> file
        assert_eq!(h.form().source, SourceKind::File);
        h.ctrl('s');
        assert_screen!(format!("source_change_move_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
        h.key(KeyCode::Char('n'));
        h.key(KeyCode::Left);
        h.key(KeyCode::Left); // keychain -> prompt (wraps)
        assert_eq!(h.form().source, SourceKind::Prompt);
        h.ctrl('s');
        assert_screen!(format!("source_change_remove_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
    }
    // A `prompt` profile asks before connecting and offers no checkbox.
    for lang in [Lang::En, Lang::Ko] {
        let mut cfg = sample_config(None);
        cfg.connections[0].set_source(datarig_core::secret::PasswordSource::Prompt);
        let mut h = launched(lang, &cfg);
        h.key(KeyCode::Enter);
        assert!(h.app.conns.attempt().is_none() && h.prompt().is_some());
        assert_screen!(format!("password_prompt_every_time_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
    }
}

#[test]
fn source_errors_on_the_node() {
    for lang in [Lang::En, Lang::Ko] {
        let mut cfg = sample_config(None);
        cfg.connections[0].set_source(datarig_core::secret::PasswordSource::Env("PROD_DB_PASSWORD".into()));
        let mut h = launched(lang, &cfg);
        h.app.set_env_lookup(std::sync::Arc::new(|_| None));
        h.key(KeyCode::Enter);
        assert_screen!(format!("source_error_env_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
    }
}

// ── workspace tabs ─────────────────────────────────────────────────

/// Three console tabs: the first runs a statement inside an open transaction, the second is
/// idle, the third lost its connection (the active one).
fn three_tabs(lang: Lang) -> Harness {
    let mut h = Harness::connected(lang);
    h.ctrl('e');
    h.tab_db(0, DbEvent::Block(true));
    h.tab_db(0, DbEvent::TxOpen(true));
    h.ctrl('t');
    h.ctrl('t');
    h.keys("i");
    h.type_text("select 3");
    h.key(KeyCode::Esc);
    h.ctrl('e');
    h.tab_db(2, DbEvent::Lost { error: "server closed the connection unexpectedly".into() });
    h
}

#[test]
fn tab_bar_en_ko_sizes() {
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            let mut h = three_tabs(lang);
            assert_screen!(format!("tab_bar_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
    }
}

#[test]
fn tab_bar_with_icons_and_many_tabs() {
    use datarig_core::config::IconsSetting;
    let mut h = three_tabs(Lang::En);
    h.app.icons = IconsSetting::On;
    let first_row = |h: &mut Harness, w, hh| h.screen(w, hh).lines().next().unwrap().to_string();
    let mut out = format!("icons 80: {}\n", first_row(&mut h, 80, 24));
    // Fourteen tabs: the bar scrolls to keep the active one in view and marks hidden tabs.
    for _ in 0..11 {
        h.ctrl('t');
    }
    for (label, keys) in [("last", ""), ("middle", " 7"), ("first", " 1")] {
        h.keys(keys);
        out.push_str(&format!("{label} 80: {}\n", first_row(&mut h, 80, 24)));
        out.push_str(&format!("{label} 160: {}\n", first_row(&mut h, 160, 45)));
    }
    insta::assert_snapshot!(out);
}

/// Idle paging: the results panel's title counts down on its right while the portal
/// is open (`(paging · 23s)`); once closed it says `(paging closed)` and the grid keeps its
/// rows with a footer saying why paging stopped (fake clock, en and ko, both sizes).
#[test]
fn idle_paging_countdown_and_closed_footer() {
    for (lang, (w, hh)) in [Lang::En, Lang::Ko].into_iter().flat_map(|l| SIZES.map(|s| (l, s))) {
        let name = format!("{}_{w}x{hh}", lang_tag(lang));
        let mut h = Harness::connected(lang);
        h.ctrl('e');
        let cols = vec![meta("id", "int8", true, false)];
        let rows = (0..500).map(|i| vec![Some(i.to_string())]).collect::<Vec<_>>();
        h.db(DbEvent::TxOpen(true));
        h.db(DbEvent::Page { id: 1, columns: Some(cols), rows, more: true, elapsed: Duration::from_millis(5) });
        h.advance(Duration::from_secs(7));
        assert_screen!(format!("idle_paging_open_{name}"), lang, h.draw(w, hh));
        h.advance(Duration::from_secs(23));
        assert!(matches!(&h.sent()[..], [.., DbCommand::ClosePortal { id: 1 }]));
        h.db(DbEvent::TxOpen(false));
        assert_screen!(format!("idle_paging_closed_{name}"), lang, h.draw(w, hh));
    }
}

/// The explorer's context menu (right click on a connected profile), en and ko.
#[test]
fn context_menu_en_ko() {
    use ratatui::crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    for (lang, w, hh) in [(Lang::En, 80, 24), (Lang::Ko, 80, 24), (Lang::En, 160, 45), (Lang::Ko, 160, 45)] {
        let mut h = explorer_states(lang, false);
        h.draw(w, hh);
        let area = h.app.explorer.area;
        let row = h.rows().iter().position(|r| r == "local-pg").unwrap() as u16;
        h.app.handle_event(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: area.x + 6,
            row: area.y + row,
            modifiers: KeyModifiers::NONE,
        }));
        assert_screen!(format!("context_menu_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
    }
}

// ── saved queries, the save dialog and the second instance ─────────────────

/// Temporary data and state directories for one snapshot.
struct P6Dirs(std::path::PathBuf);

impl P6Dirs {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!("datarig-snap-p6-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // A few saved queries, one folder open, one closed.
        let store = datarig_core::scripts::ScriptStore::open(&root.join("data"));
        for (p, t) in [
            ("売上/日別 集計.sql", "select 1"),
            ("reports/daily.sql", "select * from shop.orders where status = 'paid';"),
            ("reports/old/2024.sql", "select 2024"),
            ("scratch.sql", "select now()"),
        ] {
            store.create(p, t).unwrap();
        }
        P6Dirs(root)
    }

    fn launch(&self, lang: Lang) -> Harness {
        let cfg = test_db_config();
        let (mut app, clock) = new_app_with_clock(&cfg, lang);
        let store = std::sync::Arc::new(datarig_core::secret::MemoryStore::new());
        app.set_secret_store(store.clone() as std::sync::Arc<dyn datarig_core::secret::SecretStore>);
        app.set_paths(datarig_core::paths::Paths {
            data: Some(self.0.join("data")),
            state: Some(self.0.join("state")),
        });
        app.launch(datarig_tui::app::Startup::Normal);
        let driver = FakeDriver::default();
        Harness { app, cancelled: driver.any_cancel.clone(), driver, store, clock }
    }
}

impl Drop for P6Dirs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Put the explorer's cursor on the row shown as `text`.
fn select_row(h: &mut Harness, text: &str) {
    h.app.focus = datarig_tui::app::Focus::Tree;
    let rows = h.app.explorer_rows();
    let i = h.rows().iter().position(|r| r.trim() == text).unwrap_or_else(|| panic!("no {text} in {:?}", h.rows()));
    h.app.explorer.select(&rows, i);
}

/// The explorer's "Saved queries" section: folders (one open), a script open in a tab (its
/// name in the tab label and the editor title), the cursor on it.
#[test]
fn saved_queries_section_en_ko_sizes() {
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            let dirs = P6Dirs::new(&format!("section-{}-{w}", lang_tag(lang)));
            let mut h = dirs.launch(lang);
            select_row(&mut h, "reports/");
            h.key(KeyCode::Enter);
            select_row(&mut h, "daily");
            h.key(KeyCode::Enter);
            select_row(&mut h, "daily");
            assert_screen!(format!("saved_queries_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
    }
}

/// `Ctrl+S` on a console: the name dialog with a name that is taken.
#[test]
fn save_name_dialog_en_ko_sizes() {
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            let dirs = P6Dirs::new(&format!("dialog-{}-{w}", lang_tag(lang)));
            let mut h = dirs.launch(lang);
            h.key(KeyCode::Tab);
            h.ctrl('s');
            h.type_text("REPORTS/Daily");
            h.key(KeyCode::Enter);
            assert_screen!(format!("save_name_dialog_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
    }
}

/// A second instance: the banner on top.
#[test]
fn read_only_banner_en_ko_sizes() {
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            let dirs = P6Dirs::new(&format!("banner-{}-{w}", lang_tag(lang)));
            let _first = dirs.launch(lang);
            let mut h = dirs.launch(lang);
            assert!(h.app.read_only);
            assert_screen!(format!("read_only_banner_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
    }
}

/// The messages of the workspace recovery, the trash and the typed failures, rendered in both
/// languages (one line each).
#[test]
fn recovery_trash_and_failure_messages_en_ko() {
    use datarig_core::fault::{Fault, FaultKind, KeychainFault};
    use datarig_core::i18n::{I18n, Label, Msg};
    use std::io::ErrorKind;
    let faults = [
        Fault::new(FaultKind::Io(ErrorKind::PermissionDenied), "x"),
        Fault::new(FaultKind::Io(ErrorKind::Other), "x"),
        Fault::new(FaultKind::Toml { line: Some(3) }, "x"),
        Fault::new(FaultKind::Toml { line: None }, "x"),
        Fault::new(FaultKind::Shape { key: "editor".into() }, "x"),
        Fault::new(FaultKind::Keychain(KeychainFault::Locked), "x"),
        Fault::new(FaultKind::Keychain(KeychainFault::NoStore), "x"),
        Fault::new(FaultKind::Keychain(KeychainFault::Damaged), "x"),
        Fault::new(FaultKind::Keychain(KeychainFault::Refused), "x"),
        Fault::new(FaultKind::Keychain(KeychainFault::Ambiguous), "x"),
        Fault::new(FaultKind::Keychain(KeychainFault::Mismatch), "x"),
        Fault::new(FaultKind::Keychain(KeychainFault::Failure), "x"),
        Fault::new(FaultKind::Other, "x"),
    ];
    let mut out = String::new();
    for lang in [Lang::En, Lang::Ko] {
        let i = I18n::new(lang);
        let reason = |f: &Fault| i.msg(&datarig_tui::app::fault_reason(f)).to_string();
        let path = "/state/consoles".to_string();
        let msgs = vec![
            Msg::WorkspaceConsolesRecovered { count: 3 },
            Msg::WorkspaceConsolesMissing { count: 1 },
            Msg::WorkspaceConsolesMissing { count: 2 },
            Msg::WorkspaceConsolesUnreadable { count: 1, path: path.clone() },
            Msg::WorkspaceConsolesFolderUnreadable { path, error: reason(&faults[0]) },
            Msg::WorkspaceBroken { error: reason(&faults[2]), path: "/state/workspace.toml.bak".into() },
            Msg::WorkspaceBrokenKept { error: reason(&faults[0]) },
            Msg::ScriptsIndexRebuilt { error: reason(&faults[2]), path: "/data/scripts.toml.bak".into(), count: 2 },
            Msg::ScriptsIndexKept { error: reason(&faults[0]) },
            Msg::TabTrashFailed { error: reason(&faults[0]) },
            Msg::TabTrashUnreadable { error: reason(&faults[0]) },
            Label::TabCloseConsoleUnsaved.into(),
            Msg::ScriptsWriteQuitFailed { error: reason(&faults[0]) },
            Label::ValidateScriptUnreadable.into(),
            Label::RecoverTitle.into(),
            Label::RecoverEmpty.into(),
            Label::CommandRecover.into(),
            Msg::RecoverAgeMinutes { count: 5 },
            Msg::RecoverAgeHours { count: 1 },
            Msg::RecoverAgeDays { count: 2 },
            Msg::ConfigSaveFailed { error: reason(&faults[4]) },
            Msg::MigrateSaveFailed { error: reason(&faults[0]) },
            Msg::MigratePartial { error: format!("prod: {}", reason(&faults[5])) },
            Msg::SecretMigrateFailed { error: reason(&faults[6]) },
            Msg::SecretErrorFile { error: reason(&faults[2]) },
            Msg::SecretUnavailable { error: reason(&faults[6]) },
            Msg::SecretErrorKeychain { error: reason(&faults[5]) },
            Label::SecretErrorCommandParse.into(),
            Msg::SecretErrorCommandStart { program: "pass".into(), error: reason(&faults[0]) },
            Msg::SourceChangeFailed { error: reason(&faults[0]) },
            Msg::SourceChangeRemoveFailed {
                name: "prod".into(),
                from: "OS keychain".into(),
                error: reason(&faults[5]),
            },
            Msg::FormSourceKeychainUnavailable { error: reason(&faults[6]) },
        ];
        for m in msgs {
            out.push_str(&format!("{} {}: {}\n", lang_tag(lang), m.key(), i.msg(&m)));
        }
        for f in &faults {
            out.push_str(&format!("{} fault {:?}: {}\n", lang_tag(lang), f.kind, reason(f)));
        }
        let config_errors = [
            datarig_core::config::parse("language = \"fr\"\n").unwrap_err(),
            datarig_core::config::parse("[[connections]]\nname = \"a\"\ncolor = \"reddish\"\n").unwrap_err(),
            datarig_core::config::parse("[[connections]]\nname = \"a\"\npassword_source = \"env\"\n").unwrap_err(),
            datarig_core::config::parse("version = 9\n").unwrap_err(),
            datarig_core::config::parse("page_size = 0\n").unwrap_err(),
            datarig_core::config::parse("a = [\n").unwrap_err(),
        ];
        for e in &config_errors {
            let body = i.msg(&datarig_tui::app::config_error_msg(&i, e)).to_string();
            out.push_str(&format!("{} error.config: {}\n", lang_tag(lang), i.msg(&Msg::ErrorConfig { error: body })));
        }
    }
    insta::assert_snapshot!("recovery_trash_failure_messages", english_pinned(&out));
}

/// `:recover` and closing a console whose save failed, in both languages.
#[cfg(unix)]
#[test]
fn recover_list_and_console_close_confirm_en_ko() {
    use std::os::unix::fs::PermissionsExt;
    for lang in [Lang::En, Lang::Ko] {
        let root = std::env::temp_dir().join(format!("datarig-snap-trash-{}-{}", std::process::id(), lang_tag(lang)));
        let _ = std::fs::remove_dir_all(&root);
        let mut h = Harness::connected(lang);
        h.app.set_paths(datarig_core::paths::Paths { data: Some(root.join("data")), state: Some(root.join("state")) });
        for text in ["select 'first closed';", "select 'second closed';"] {
            h.ctrl('t');
            h.keys("i");
            h.type_text(text);
            h.key(KeyCode::Esc);
            h.ctrl('w');
        }
        h.command("recover");
        assert_screen!(format!("recover_list_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
        h.key(KeyCode::Esc);
        let consoles = root.join("state").join("consoles");
        h.advance(std::time::Duration::from_secs(2));
        std::fs::set_permissions(&consoles, std::fs::Permissions::from_mode(0o555)).unwrap();
        h.keys("i");
        h.type_text(" -- more");
        h.key(KeyCode::Esc);
        h.ctrl('w');
        assert_screen!(format!("console_close_unsaved_{}_80x24", lang_tag(lang)), lang, h.draw(80, 24));
        std::fs::set_permissions(&consoles, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// What the driver reports when the server refuses the connection.
fn refused() -> datarig_core::driver::DbError {
    use datarig_core::fault::{Fault, FaultKind};
    datarig_core::driver::DbError::Connection(Fault::new(
        FaultKind::Io(std::io::ErrorKind::ConnectionRefused),
        "error connecting to server: Connection refused (os error 61)",
    ))
}

/// Database failures that are not the server's own message, as the status bar says them.
#[test]
fn database_failures_in_words_en_ko() {
    use datarig_core::driver::DbError;
    use datarig_core::fault::{Fault, FaultKind};
    use std::io::ErrorKind as K;
    let errors = [
        ("server", DbError::Server("FATAL: database \"x\" does not exist".into())),
        ("closed", DbError::Closed),
        ("no_answer", DbError::NoAnswer(Duration::from_secs(10))),
        ("settings", DbError::Settings(Fault::other("invalid connection string"))),
        ("refused", refused()),
        ("cut", DbError::Connection(Fault::new(FaultKind::Io(K::ConnectionReset), "reset"))),
        ("timed_out", DbError::Connection(Fault::new(FaultKind::Io(K::TimedOut), "timeout"))),
        ("unreachable", DbError::Connection(Fault::new(FaultKind::Io(K::HostUnreachable), "no route"))),
        ("no_host", DbError::Connection(Fault::new(FaultKind::HostNotFound, "failed to lookup address information"))),
        ("other", DbError::Connection(Fault::other("unexpected message from server"))),
        ("not_supported", DbError::NotSupported),
    ];
    let mut out = String::new();
    for lang in [Lang::En, Lang::Ko] {
        for (name, e) in &errors {
            let mut h = launched(lang, &sample_config(None));
            h.key(KeyCode::Enter);
            h.db(DbEvent::ConnectFailed { error: e.clone(), auth: false });
            // No tab yet: no editor mode, the message comes first.
            let s = h.status(160, 45);
            out.push_str(&format!("{} {name:<13} {}\n", lang_tag(lang), s.split('│').next().unwrap_or("").trim()));
        }
    }
    assert!(!out.contains("os error") && !out.contains("unexpected message"), "{out}");
    assert!(out.contains("en no_host       Connection failed: the host name could not be resolved"), "{out}");
    insta::assert_snapshot!("database_failures", english_pinned(&out));
}

/// A tab without a connection (its profile was deleted, `Space t u` brought it back) says so
/// in a banner on top of its editor, with the key that picks one.
#[test]
fn unbound_banner_en_ko_sizes() {
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            let mut h = Harness::with_config(&sample_config(None), lang);
            h.explore("v6");
            h.keys("o");
            h.keys("i");
            h.type_text("select 'kept';");
            h.key(KeyCode::Esc);
            h.explore("v6");
            h.keys("dy");
            h.keys(" tu");
            assert_eq!(h.app.tab().profile, None);
            // The delete's flash is not what this screen shows (in Korean it leaves the hint
            // line less room than in English).
            h.app.transient = None;
            assert_screen!(format!("unbound_banner_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
    }
}

/// The result of a JOIN of shop.orders, users and order_items, its columns named after their
/// table columns (and one expression), with the profile's keys known.
fn join_result(lang: Lang, icons: bool) -> Harness {
    let mut h = Harness::connected(lang);
    h.db(DbEvent::Keys(Ok(shop_keys())));
    if icons {
        h.app.icons = datarig_core::config::IconsSetting::On;
    }
    h.ctrl('e');
    let cols = vec![
        meta_from("id", "int8", true, ORDERS, 1),
        meta_from("user_id", "int8", true, ORDERS, 2),
        meta_from("email", "text", false, USERS, 2),
        meta_from("name", "text", false, USERS, 3),
        meta_from("order_id", "int8", true, ORDER_ITEMS, 1),
        meta_from("line_no", "int4", true, ORDER_ITEMS, 2),
        meta_from("product_id", "int8", true, ORDER_ITEMS, 3),
        meta("doubled", "numeric", true, false),
    ];
    let row = |i: u32| {
        [i, 1, 0, 0, i, 1, 7, 2]
            .iter()
            .enumerate()
            .map(|(c, v)| match c {
                2 => Some(format!("user{v}{i}@example.com")),
                3 => Some(format!("ユーザー {i}")),
                _ => Some(v.to_string()),
            })
            .collect::<Vec<_>>()
    };
    let rows = (1..=3).map(row).collect();
    h.db(DbEvent::Page { id: 1, columns: Some(cols), rows, more: false, elapsed: Duration::from_millis(3) });
    h.key(KeyCode::Tab); // the results
    h
}

/// Key marks in the grid's headers: PK/FK/UQ of each source table, every
/// member of a composite key, nothing for the expression; glyphs with icons on, letters off.
#[test]
fn grid_key_marks_en_ko_icons_on_off() {
    for lang in [Lang::En, Lang::Ko] {
        for icons in [false, true] {
            for (w, hh) in SIZES {
                let mut h = join_result(lang, icons);
                // The whole width for the headers (the inspector has its own snapshots).
                h.app.detail.visible = false;
                let name =
                    format!("grid_key_marks_{}_{}_{w}x{hh}", if icons { "icons" } else { "text" }, lang_tag(lang));
                assert_screen!(name, lang, h.draw(w, hh));
            }
        }
    }
    let mut h = join_result(Lang::En, false);
    h.app.detail.visible = false;
    let t = h.draw(160, 45);
    let header = row_text(t.backend().buffer(), h.app.layout.results.y + 1);
    for want in
        ["PK id", "FK user_id", "UQ email", "│ name", "PK FK order_id", "PK line_no", "FK product_id", "│ doubled"]
    {
        assert!(header.contains(want), "{want} in {header}");
    }
    // The marks are drawn in their own colors.
    let buf = t.backend().buffer();
    let y = h.app.layout.results.y + 1;
    let x = (0..160).find(|&x| buf[(x, y)].symbol() == "P").unwrap();
    assert_eq!(buf[(x, y)].fg, theme::DARK.key_pk);
    let x = (0..160).find(|&x| buf[(x, y)].symbol() == "U").unwrap();
    assert_eq!(buf[(x, y)].fg, theme::DARK.key_uq);
}

/// The result inspector: the Cell tab with a JSON value pretty-printed, and the Row
/// tab with the key marks of a JOIN's columns.
#[test]
fn inspector_cell_and_row_en_ko_sizes() {
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            let mut h = with_edge_results(lang);
            h.key(KeyCode::Tab);
            h.keys("$");
            assert_screen!(format!("inspector_cell_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
            let mut h = join_result(lang, false);
            h.keys("lI");
            assert_screen!(format!("inspector_row_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
    }
}

/// The grid's context menu (right click on a cell): the viewer, the inspector, copying the
/// cell or row and "copy as" each format, with their keys; and a selected range.
#[test]
fn grid_copy_menu_and_range_en_ko_sizes() {
    use ratatui::crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            let mut h = with_edge_results(lang);
            h.key(KeyCode::Tab);
            h.draw(w, hh);
            let (x, y) = (h.app.tabs.active().grid.hit_cols[1].0 + 2, h.app.layout.results.y + 4);
            h.app.handle_event(Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Right),
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            }));
            assert_screen!(format!("grid_copy_menu_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
    }
    let mut h = with_edge_results(Lang::En);
    h.key(KeyCode::Tab);
    h.keys("jlvjjl");
    let t = h.draw(160, 45);
    insta::assert_snapshot!("grid_range_en_160x45", t.backend());
    let buf = t.backend().buffer();
    let x = h.app.tabs.active().grid.hit_cols[1].0 + 2;
    let y = h.app.layout.results.y + 1 + 2 + 1;
    assert_eq!(buf[(x, y)].bg, theme::DARK.range.bg.unwrap(), "the range's first cell");
}

/// The settings screen: categories, current values and what they mean, the selected
/// setting's description; `icons` shows a preview of the glyphs and the hint (icons on and
/// off), the theme the terminal's background and a preview, the clipboard its tmux note.
#[test]
fn settings_screen_en_ko_sizes() {
    for lang in [Lang::En, Lang::Ko] {
        for (w, hh) in SIZES {
            let mut h = Harness::connected(lang);
            h.command("settings");
            assert_screen!(format!("settings_icons_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
            h.app.icons = datarig_core::config::IconsSetting::On;
            assert_screen!(format!("settings_icons_on_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
            // Icons, the command line, then the theme (the background was not asked).
            h.keys("jj");
            assert_screen!(format!("settings_theme_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
            // `l` previews the next theme; `Esc` would go back.
            h.keys("l");
            assert_screen!(format!("settings_theme_preview_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
            h.key(KeyCode::Esc);
            h.command("settings");
            // The cursor shape, the cell detail, then the clipboard.
            h.keys("jjjjj");
            assert_screen!(format!("settings_clipboard_{}_{w}x{hh}", lang_tag(lang)), lang, h.draw(w, hh));
        }
    }
}

/// The mode badge: each mode in its color with dark bold text (Visual by line says V-LINE),
/// COMMAND while the command line is open.
#[test]
fn mode_badge_colors_follow_the_mode() {
    use ratatui::style::Modifier;
    let mut h = Harness::connected(Lang::En);
    let mut lines = Vec::new();
    let mut badge = |h: &mut Harness, name: &str, bg| {
        let t = h.draw(80, 24);
        let buf = t.backend().buffer();
        let text = row_text(buf, 23);
        // The badge: a blank, the upper-case label, a blank.
        let end = 2 + text[1..].find(' ').expect("the badge's end");
        // The status bar after it (dimmed behind the command line).
        let after = buf[(end as u16, 23)].bg;
        assert!(
            after == theme::DARK.surface || after == theme::DARK.dim_bg(theme::DARK.surface),
            "{name}: the badge ends at {end}"
        );
        for x in 0..end as u16 {
            let c = &buf[(x, 23)];
            assert_eq!((c.bg, c.fg), (bg, theme::DARK.mode_fg), "{name}: cell {x} of {text:?}");
            assert!(c.modifier.contains(Modifier::BOLD), "{name}: bold");
        }
        lines.push(format!("{name:>8}: {}", &text[..end]));
    };
    badge(&mut h, "normal", theme::DARK.mode_normal);
    h.keys("i");
    badge(&mut h, "insert", theme::DARK.mode_insert);
    h.key(KeyCode::Esc);
    h.keys("v");
    badge(&mut h, "visual", theme::DARK.mode_visual);
    h.keys("V");
    badge(&mut h, "v-line", theme::DARK.mode_visual);
    h.key(KeyCode::Esc);
    h.ctrl('k');
    badge(&mut h, "command", theme::DARK.mode_command);
    h.key(KeyCode::Esc);
    insta::assert_snapshot!(lines.join("\n"));
}

/// The statement a run would take: a bar in the gutter and a faint tint on
/// its lines (the cursor's line keeps its own), following the cursor; with a Visual selection
/// the bar marks the selection's lines instead and nothing is tinted.
#[test]
fn current_statement_is_marked_in_the_gutter() {
    let mut h = Harness::connected(Lang::En);
    h.app.detail.visible = false;
    // The second statement (lines 3–6), the cursor on its second line.
    h.app.tab_mut().editor.row = 4;
    let t = h.draw(100, 30);
    insta::assert_snapshot!(t.backend());
    let buf = t.backend().buffer();
    let e = h.app.layout.editor;
    // The editor's text starts below the connection bar; its gutter is `nnn ` wide.
    let (x_bar, y0) = (e.x + 1 + 3, h.app.layout.editor_text.y);
    assert_eq!(y0, e.y + 2, "the connection bar takes the panel's first line");
    let marked = |buf: &Buffer, line: u16| buf[(x_bar, y0 + line)].symbol() == "▎";
    for line in 0..9u16 {
        let want = (3..=6).contains(&line);
        assert_eq!(marked(buf, line), want, "line {}: {}", line + 1, row_text(buf, y0 + line));
        if want {
            assert_eq!(buf[(x_bar, y0 + line)].fg, theme::DARK.current_stmt_bar);
            let bg = if line == 4 { theme::DARK.cursor_line.bg.unwrap() } else { theme::DARK.current_stmt.bg.unwrap() };
            assert_eq!(buf[(x_bar + 5, y0 + line)].bg, bg, "line {}", line + 1);
        } else {
            assert_eq!(buf[(x_bar + 5, y0 + line)].bg, theme::DARK.bg, "line {}", line + 1);
        }
    }
    // It follows the cursor (to the third statement's line; line 8 is blank).
    h.keys("jjjj");
    let t = h.draw(100, 30);
    let buf = t.backend().buffer();
    assert!(marked(buf, 8) && !marked(buf, 4), "the third statement now");
    // A Visual selection is marked instead of the statement, without the tint.
    h.keys("kvj");
    let t = h.draw(100, 30);
    let buf = t.backend().buffer();
    assert!(marked(buf, 7) && marked(buf, 8) && !marked(buf, 6) && !marked(buf, 9));
    assert_eq!(buf[(x_bar + 5, y0 + 6)].bg, theme::DARK.bg, "no tint on the statement before");
    insta::assert_snapshot!("current_statement_selection", t.backend());
}
