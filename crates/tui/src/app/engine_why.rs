//! The words for why a fact about a server is not known.

use datarig_core::engine::Why;
use datarig_i18n::{Label, Msg};

/// What the UI says for `why`.
pub fn why_text(why: &Why) -> Msg {
    match why {
        Why::NotChecked => Label::EngineWhyNotChecked.into(),
        Why::VersionUnknown => Label::EngineWhyVersionUnknown.into(),
        Why::VersionsDisagree => Label::EngineWhyVersionsDisagree.into(),
        Why::EnginesDisagree => Label::EngineWhyEnginesDisagree.into(),
        Why::NotOnEngine => Label::EngineWhyNotOnEngine.into(),
        Why::NotBuilt => Label::EngineWhyNotBuilt.into(),
        Why::VersionTooOld { needed } => {
            Msg::EngineWhyVersionTooOld { needed: format!("{}.{}.{}", needed.major, needed.minor, needed.patch) }
        }
        Why::FeatureOff(feature) => Msg::EngineWhyFeatureOff { feature: feature.name().to_string() },
        Why::MarkerUnconfirmed => Label::EngineWhyMarkerUnconfirmed.into(),
    }
}

#[cfg(test)]
mod tests;
