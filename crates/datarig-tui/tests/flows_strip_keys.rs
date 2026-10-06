//! The result tab strip says, dim at its right end, the keys that move between its tabs (the
//! ones bound in the results pane, remapping included), when there are tabs to move between
//! and the keys fit after them; nothing when they do not.

mod common;

use common::*;
use datarig_core::driver::{DbCommand, DbEvent};
use datarig_core::i18n::Lang;
use datarig_tui::app::Focus;
use datarig_tui::app::tabs::ResultView;
use datarig_tui::theme;
use datarig_tui::widgets::editor::Editor;
use std::time::Duration;

/// `h` ran two `SELECT`s, each with rows: two result tabs and the Messages.
fn two_results(mut h: Harness) -> Harness {
    h.app.tab_mut().editor = Editor::new("SELECT 1;\nSELECT 2;");
    h.sent();
    h.keys("ggVG");
    h.ctrl('e');
    let sent = h.sent();
    let Some(DbCommand::Execute { id, .. }) = sent.into_iter().find(|c| matches!(c, DbCommand::Execute { .. })) else {
        panic!("a run")
    };
    let rows = |n: &str| vec![vec![Some(n.to_string())]];
    h.db(DbEvent::StepRows {
        id,
        index: 0,
        columns: Some(vec![meta("a", "int4", true, false)]),
        rows: rows("1"),
        more: false,
    });
    h.db(DbEvent::Page {
        id,
        columns: Some(vec![meta("a", "int4", true, false)]),
        rows: rows("2"),
        more: false,
        elapsed: Duration::from_millis(1),
    });
    h.app.focus = Focus::Results;
    h
}

/// The strip's line and where on it `text` starts, at `w`×30.
fn strip_line(h: &mut Harness, w: u16) -> (String, u16, ratatui::buffer::Buffer) {
    let t = h.draw(w, 30);
    let buf = t.backend().buffer().clone();
    let strip = h.app.layout.strip;
    assert!(strip.height == 1, "the strip is shown");
    (row_text(&buf, strip.y), strip.y, buf)
}

#[test]
fn the_strip_shows_the_keys_that_move_between_its_tabs() {
    let mut h = two_results(Harness::connected(Lang::En));
    let (line, y, buf) = strip_line(&mut h, 120);
    assert!(line.contains("Result 1") && line.contains("[Result 2]") && line.contains("Messages"), "{line}");
    let at = line.find("H/L").unwrap_or_else(|| panic!("the keys: {line}"));
    // At the right end of the strip, one blank before the border.
    let strip = h.app.layout.strip;
    let x = datarig_tui::text::width(&line[..at]) as u16;
    assert_eq!(x + 3, strip.x + strip.width - 1, "{line}");
    for dx in 0..3 {
        assert_eq!(buf[(x + dx, y)].fg, theme::DARK.fg_dim, "dim");
        assert_eq!(buf[(x + dx, y)].bg, theme::DARK.surface);
    }
    insta::assert_snapshot!("strip_keys_en_120x30", h.draw(120, 30).backend());
    // They work as they say, and the Plan tab's are the same keys.
    h.keys("L");
    assert_eq!(h.app.tab().exec.view, ResultView::Messages);
    h.keys("H");
    assert_eq!(h.app.tab().exec.shown, Some(1));
}

#[test]
fn the_keys_go_first_when_the_strip_is_narrow() {
    let mut h = two_results(Harness::connected(Lang::En));
    let mut seen = (false, false);
    for w in 60..=120 {
        h.draw(w, 30);
        if h.app.layout.strip.height == 0 {
            continue;
        }
        let (line, _, _) = strip_line(&mut h, w);
        match line.find("H/L") {
            Some(at) => {
                seen.0 = true;
                // Never over a tab: the last label ends before the keys, a blank between.
                let before = line[..at].trim_end_matches(' ');
                assert!(before.ends_with("Messages") && before.len() < at - 1, "{w}: {line}");
            }
            None => seen.1 = true,
        }
        assert!(line.contains("[Result 2]"), "the shown tab stays at {w}: {line}");
    }
    assert_eq!(seen, (true, true), "shown when wide, left out when narrow");
}

#[test]
fn a_single_result_has_no_tabs_to_move_between() {
    // A run without rows next to nothing: the strip, if any, has no keys to show.
    let mut h = Harness::connected(Lang::En);
    h.app.tab_mut().editor = Editor::new("SET a = 1;\nSET b = 2;");
    h.sent();
    h.keys("ggVG");
    h.ctrl('e');
    let Some(DbCommand::Execute { id, .. }) = h.sent().into_iter().find(|c| matches!(c, DbCommand::Execute { .. }))
    else {
        panic!("a run")
    };
    h.db(DbEvent::Done {
        id,
        outcome: datarig_core::driver::Outcome::Command("SET".into()),
        elapsed: Duration::from_millis(1),
    });
    let screen = h.screen(120, 30);
    assert!(!screen.contains("H/L"), "{screen}");
}

#[test]
fn the_keys_follow_the_key_map() {
    let mut cfg = test_db_config();
    cfg.keymap = datarig_core::config::parse(
        "[keymap.grid]\n\"H\" = \"none\"\n\"L\" = \"none\"\n\"[\" = \"results.tab.prev\"\n\"]\" = \"results.tab.next\"\n",
    )
    .unwrap()
    .keymap;
    let h = Harness::with_config(&cfg, Lang::En);
    let mut h = connected(h);
    h = two_results(h);
    let (line, _, _) = strip_line(&mut h, 120);
    assert!(line.contains("[/]") && !line.contains("H/L"), "{line}");
}

/// `h` connected as `Harness::connected` does it.
fn connected(mut h: Harness) -> Harness {
    h.db(DbEvent::Connected);
    h.db(DbEvent::Schemas(Ok(vec!["public".into()])));
    h.db(DbEvent::Catalog(Ok(catalog())));
    h.sent();
    h
}
