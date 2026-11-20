use super::*;
use datarig_i18n::Lang;
use std::collections::BTreeSet;

/// Every reason has words of its own in each language.
#[test]
fn every_why_has_a_text_in_en_and_ko() {
    let all = [Why::NotChecked, Why::VersionUnknown, Why::VersionsDisagree, Why::EnginesDisagree];
    // A new reason fails to compile here until it is listed above.
    for why in &all {
        match why {
            Why::NotChecked | Why::VersionUnknown | Why::VersionsDisagree | Why::EnginesDisagree => {}
        }
    }
    for lang in [Lang::En, Lang::Ko] {
        let texts: BTreeSet<&str> = all.iter().map(|w| why_text(w).text(lang)).collect();
        assert_eq!(texts.len(), all.len(), "{lang:?}: each reason its own text");
        assert!(texts.iter().all(|t| !t.trim().is_empty()), "{lang:?}: {texts:?}");
    }
    for why in &all {
        assert_ne!(why_text(why).text(Lang::En), why_text(why).text(Lang::Ko), "{why:?}");
    }
}
