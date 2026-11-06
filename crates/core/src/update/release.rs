//! GitHub's answer about the latest release (`GET /repos/DokaDev/datarig/releases/latest`): the
//! version, its date and the one-line summary that opens its notes.

use super::Version;

/// The most of an answer that is read; a longer one is a bad answer. A real one is about 20 KiB.
pub const MAX_ANSWER: usize = 256 * 1024;

/// The most characters of a summary that are kept.
pub const MAX_SUMMARY: usize = 300;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// `YYYY-MM-DD` of `published_at`, when it has one.
    pub date: Option<String>,
    /// The first paragraph of the notes as one line of plain text, when they open with one.
    pub summary: Option<String>,
}

/// The answer is not a stable release datarig can name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BadAnswer;

#[derive(serde::Deserialize)]
struct Answer {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    body: Option<String>,
}

impl Release {
    /// From the answer's bytes. A draft, a pre-release, a tag that is not `vX.Y.Z`, or more than
    /// [`MAX_ANSWER`] bytes is a bad answer.
    pub fn from_json(bytes: &[u8]) -> Result<Self, BadAnswer> {
        if bytes.len() > MAX_ANSWER {
            return Err(BadAnswer);
        }
        let a: Answer = serde_json::from_slice(bytes).map_err(|_| BadAnswer)?;
        let version = Version::parse_tag(&a.tag_name).filter(|v| !v.is_rc()).ok_or(BadAnswer)?;
        if a.draft || a.prerelease {
            return Err(BadAnswer);
        }
        let date = a.published_at.as_deref().and_then(|d| d.get(..10)).filter(|d| is_date(d)).map(str::to_string);
        Ok(Release { version, date, summary: a.body.as_deref().and_then(summary) })
    }
}

/// `YYYY-MM-DD`.
pub(crate) fn is_date(d: &str) -> bool {
    let b = d.as_bytes();
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    b.len() == 10 && b[4] == b'-' && b[7] == b'-' && digits(0..4) && digits(5..7) && digits(8..10)
}

/// The first paragraph of release notes as one line of plain text (marks taken out, control
/// characters dropped, spaces collapsed), at most [`MAX_SUMMARY`] characters. `None` when the
/// notes open with something else (a heading, a list) or have no text.
pub fn summary(body: &str) -> Option<String> {
    let body = body.replace("\r\n", "\n");
    let first = body.trim_start().split("\n\n").next()?;
    if first.starts_with(['#', '-', '*', '+', '>', '|']) {
        return None;
    }
    let joined = first.lines().map(str::trim).collect::<Vec<_>>().join(" ");
    // Marks the notes may not have stay as they are.
    let text = super::notes::plain_text(&joined).unwrap_or(joined);
    let text: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut one = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.is_empty() {
        return None;
    }
    if one.chars().count() > MAX_SUMMARY {
        one = one.chars().take(MAX_SUMMARY - 1).collect::<String>().trim_end().to_string() + "\u{2026}";
    }
    Some(one)
}

#[cfg(test)]
mod tests;
