use super::*;

#[test]
fn registry_ids_unique() {
    let mut ids: Vec<_> = REGISTRY.iter().map(|s| s.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), REGISTRY.len());
    assert_eq!(spec(Action::Quit).id, "app.quit");
}

#[test]
fn fuzzy_matching() {
    assert!(fuzzy_score("rst", "Run statement").is_some());
    assert!(fuzzy_score("xyz", "Run statement").is_none());
    assert_eq!(fuzzy_score("", "anything"), Some(0));
    assert!(fuzzy_score("RUN", "run statement").is_some(), "case-insensitive");
    // Word-start / consecutive matches beat scattered ones.
    assert!(fuzzy_score("test", "Test connection").unwrap() > fuzzy_score("test", "Switch connection").unwrap_or(-99));
    // Hangul syllables (as escapes) match like any other letters.
    assert!(
        fuzzy_score("\u{C5B8}\u{C5B4}", "\u{C5B8}\u{C5B4} \u{BCC0}\u{ACBD} → \u{D55C}\u{AD6D}\u{C5B4}").is_some(),
        "Korean syllables match"
    );
    assert!(
        fuzzy_score("conn", "Test connection").unwrap() > fuzzy_score("conn", "Cancel running query").unwrap_or(-99)
    );
}

#[test]
fn command_search_ranks_and_matches_english_in_korean_ui() {
    let en = I18n::new(Lang::En);
    let first = |q: &str, i: &I18n| REGISTRY[search(q, i)[0]].action;
    assert_eq!(first("run", &en), Action::RunStatement);
    assert_eq!(first("quit", &en), Action::Quit);
    assert_eq!(first("test conn", &en), Action::TestConnection);
    assert_eq!(first("korean", &en), Action::SetLanguage(LangSetting::Ko));
    assert_eq!(search("", &en).len(), REGISTRY.len(), "empty query lists everything in order");
    assert_eq!(search("", &en)[0], 0);
    assert!(search("zzzz", &en).is_empty());
    let ko = I18n::new(Lang::Ko);
    assert_eq!(first(datarig_core::i18n::Label::ActionAppQuit.text(Lang::Ko), &ko), Action::Quit);
    assert_eq!(first("quit", &ko), Action::Quit, "English words still work in the Korean UI");
    assert_eq!(first("conn.test", &ko), Action::TestConnection, "action ids match too");
}

/// The grid's screen keys never read like the result's pages (`n` / `p`), and the
/// last-row key no longer claims to load more (pages are explicit), in every language.
#[test]
fn grid_scroll_labels_differ_from_result_pages() {
    for lang in [Lang::En, Lang::Ko] {
        let text = |id: &str| by_id(id).unwrap().label.text(lang).to_string();
        for (screen, page) in [("grid.page_down", "results.page.next"), ("grid.page_up", "results.page.prev")] {
            assert_ne!(text(screen), text(page), "{lang:?}");
            assert!(!text(screen).contains(&text(page).replace("→", ":")), "{lang:?}: {}", text(screen));
        }
        assert!(!text("grid.bottom").contains('('), "{lang:?}: {}", text("grid.bottom"));
    }
}
