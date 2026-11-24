use super::*;
use datarig_core::engine::{Feature, Version};
use datarig_i18n::Lang;
use std::collections::BTreeSet;

/// Every reason has words of its own in each language; a feature that is off is named as the
/// server writes it.
#[test]
fn every_why_has_a_text_in_en_and_ko() {
    let all = [
        Why::NotChecked,
        Why::VersionUnknown,
        Why::VersionsDisagree,
        Why::EnginesDisagree,
        Why::NotOnEngine,
        Why::NotBuilt,
        Why::VersionTooOld { needed: Version::new(8, 0, 17) },
        Why::FeatureOff(Feature::PerformanceSchema),
        Why::MarkerUnconfirmed,
    ];
    // A new reason fails to compile here until it is listed above.
    for why in &all {
        match why {
            Why::NotChecked
            | Why::VersionUnknown
            | Why::VersionsDisagree
            | Why::EnginesDisagree
            | Why::NotOnEngine
            | Why::NotBuilt
            | Why::VersionTooOld { .. }
            | Why::FeatureOff(Feature::PerformanceSchema)
            | Why::MarkerUnconfirmed => {}
        }
    }
    for lang in [Lang::En, Lang::Ko] {
        let texts: BTreeSet<String> = all.iter().map(|w| why_text(w).render(lang)).collect();
        assert_eq!(texts.len(), all.len(), "{lang:?}: each reason its own text");
        assert!(texts.iter().all(|t| !t.trim().is_empty() && !t.contains('{')), "{lang:?}: {texts:?}");
    }
    for why in &all {
        assert_ne!(why_text(why).render(Lang::En), why_text(why).render(Lang::Ko), "{why:?}");
    }
    assert_eq!(
        why_text(&Why::FeatureOff(Feature::PerformanceSchema)).render(Lang::En),
        "performance_schema is off on the server"
    );
    assert_eq!(
        why_text(&Why::VersionTooOld { needed: Version::new(8, 0, 17) }).render(Lang::En),
        "the server's version is older than 8.0.17"
    );
}
