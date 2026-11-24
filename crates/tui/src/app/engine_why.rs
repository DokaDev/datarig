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
        Why::SessionsDiffer => Label::EngineWhySessionsDiffer.into(),
        Why::EditFormatDiffers => Label::EngineWhyEditFormatDiffers.into(),
    }
}

/// A short name for `why` where a label stands alone (a dimmed entry): its words, or for a
/// reason with details ([`why_text`] names them) the reason without them.
pub fn why_label(why: &Why) -> Label {
    match why {
        Why::NotChecked => Label::EngineWhyNotChecked,
        Why::VersionUnknown => Label::EngineWhyVersionUnknown,
        Why::VersionsDisagree => Label::EngineWhyVersionsDisagree,
        Why::EnginesDisagree => Label::EngineWhyEnginesDisagree,
        Why::NotOnEngine => Label::EngineWhyNotOnEngine,
        Why::NotBuilt => Label::EngineWhyNotBuilt,
        Why::VersionTooOld { .. } => Label::EngineWhyVersionTooOldShort,
        Why::FeatureOff(_) => Label::EngineWhyFeatureOffShort,
        Why::MarkerUnconfirmed => Label::EngineWhyMarkerUnconfirmed,
        Why::SessionsDiffer => Label::EngineWhySessionsDiffer,
        Why::EditFormatDiffers => Label::EngineWhyEditFormatDiffers,
    }
}

#[cfg(test)]
mod tests;
