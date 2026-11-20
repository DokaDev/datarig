//! The words for why a fact about a server is not known.

use datarig_core::engine::Why;
use datarig_i18n::Label;

/// What the UI says for `why`.
pub fn why_text(why: &Why) -> Label {
    match why {
        Why::NotChecked => Label::EngineWhyNotChecked,
        Why::VersionUnknown => Label::EngineWhyVersionUnknown,
        Why::VersionsDisagree => Label::EngineWhyVersionsDisagree,
        Why::EnginesDisagree => Label::EngineWhyEnginesDisagree,
    }
}

#[cfg(test)]
mod tests;
