//! The release notes of What's new (`docs/site/src/whats-new.md`) as data, for the What's new
//! screen of the app.
//!
//! Only the small part of Markdown the notes are written in is read, and anything else is an
//! error naming its line, so notes that would not show right are found by a test before they
//! ship:
//!
//! * `# What's new`, the page's title, on the first line, and the paragraphs before the first
//!   version (the page's introduction, not part of any version);
//! * `## X.Y.Z (YYYY-MM-DD)`, a version, newest first;
//! * `### Added`, `### Changed`, `### Fixed`, `### Removed`, `### Security`;
//! * paragraphs (their lines are joined);
//! * `- ` list items, continued on lines indented by two spaces;
//! * in text: `` `code` ``, `**bold**`, `[text](url)` (the text) and `<https://…>` (the address).
//!
//! Not read: other headings, numbered or nested lists, tables, quotes, code blocks, rules, HTML,
//! images, `*` or `_` emphasis and backslash escapes.

use super::Version;

/// What the notes say, newest version first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notes {
    pub sections: Vec<Section>,
}

/// One version's notes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub version: Version,
    /// `YYYY-MM-DD`.
    pub date: String,
    pub blocks: Vec<Block>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// A paragraph, its lines joined with a space.
    Paragraph(Vec<Span>),
    /// `### Added` and the like: the word.
    Heading(String),
    /// A list item, its lines joined with a space.
    Item(Vec<Span>),
}

/// A run of text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Span {
    Text(String),
    Code(String),
    Bold(String),
}

/// A line the notes may not have.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsupported {
    /// From 1.
    pub line: usize,
    pub what: &'static str,
}

/// The `###` headings a version may have.
pub const KINDS: [&str; 5] = ["Added", "Changed", "Fixed", "Removed", "Security"];

impl Notes {
    /// The versions after `after` up to and including `upto`, newest first.
    pub fn between(&self, after: &Version, upto: &Version) -> Vec<&Section> {
        self.sections.iter().filter(|s| s.version > *after && s.version <= *upto).collect()
    }

    /// The section of `version`.
    pub fn get(&self, version: &Version) -> Option<&Section> {
        self.sections.iter().find(|s| s.version == *version)
    }
}

/// The text of `spans` without its marks.
pub fn plain(spans: &[Span]) -> String {
    spans
        .iter()
        .map(|s| match s {
            Span::Text(t) | Span::Code(t) | Span::Bold(t) => t.as_str(),
        })
        .collect()
}

/// The plain text of a paragraph's text; `None` when it has marks the notes may not have.
pub(crate) fn plain_text(s: &str) -> Option<String> {
    inline(s).ok().map(|spans| plain(&spans))
}

/// What a block being read is.
enum Open {
    Paragraph,
    Item,
}

pub fn parse(text: &str) -> Result<Notes, Unsupported> {
    let mut notes = Notes::default();
    // The block being read: what it is, its lines joined, the line it started on.
    let mut open: Option<(Open, String, usize)> = None;
    let mut in_intro = true;
    let close =
        |open: &mut Option<(Open, String, usize)>, notes: &mut Notes, in_intro: bool| -> Result<(), Unsupported> {
            let Some((kind, text, line)) = open.take() else { return Ok(()) };
            let spans = inline(&text).map_err(|what| Unsupported { line, what })?;
            if in_intro {
                return Ok(());
            }
            let section = notes.sections.last_mut().expect("a version is open after the introduction");
            section.blocks.push(match kind {
                Open::Paragraph => Block::Paragraph(spans),
                Open::Item => Block::Item(spans),
            });
            Ok(())
        };
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let line = raw.trim_end();
        let err = |what| Unsupported { line: n, what };
        if line.is_empty() {
            close(&mut open, &mut notes, in_intro)?;
            continue;
        }
        if line.contains('\t') {
            return Err(err("a tab"));
        }
        // A list item's next line.
        if let Some(rest) = line.strip_prefix("  ")
            && let Some((Open::Item, item, _)) = &mut open
        {
            let rest = rest.trim_start();
            if is_list(rest) {
                return Err(err("a nested list"));
            }
            item.push(' ');
            item.push_str(rest);
            continue;
        }
        if line.starts_with(' ') {
            return Err(err("an indented line"));
        }
        if let Some(rest) = line.strip_prefix("- ") {
            close(&mut open, &mut notes, in_intro)?;
            if in_intro {
                return Err(err("a list before the first version"));
            }
            open = Some((Open::Item, rest.trim_start().to_string(), n));
            continue;
        }
        if line.starts_with('#') {
            close(&mut open, &mut notes, in_intro)?;
            if n == 1 && line.starts_with("# ") {
                continue;
            }
            if let Some(rest) = line.strip_prefix("## ") {
                let (version, date) = version_heading(rest)
                    .ok_or_else(|| err("a version heading that is not `## X.Y.Z (YYYY-MM-DD)`"))?;
                if let Some(last) = notes.sections.last()
                    && version >= last.version
                {
                    return Err(err("a version not older than the one before it"));
                }
                notes.sections.push(Section { version, date, blocks: Vec::new() });
                in_intro = false;
                continue;
            }
            if let Some(rest) = line.strip_prefix("### ") {
                if in_intro || !KINDS.contains(&rest) {
                    return Err(err("a `###` heading other than Added, Changed, Fixed, Removed or Security"));
                }
                notes
                    .sections
                    .last_mut()
                    .expect("not in the introduction")
                    .blocks
                    .push(Block::Heading(rest.to_string()));
                continue;
            }
            return Err(err("a heading other than `## X.Y.Z (YYYY-MM-DD)` and `### <kind>`"));
        }
        if let Some(what) = unsupported_start(line) {
            return Err(err(what));
        }
        match &mut open {
            Some((Open::Paragraph, para, _)) => {
                para.push(' ');
                para.push_str(line);
            }
            // A line right under a list item that is not indented: a lazy continuation, which
            // Markdown reads as part of the item but a reader of the file would not.
            Some((Open::Item, _, _)) => return Err(err("a list item's line that is not indented")),
            None => open = Some((Open::Paragraph, line.to_string(), n)),
        }
    }
    close(&mut open, &mut notes, in_intro)?;
    Ok(notes)
}

/// `X.Y.Z (YYYY-MM-DD)`: a stable version and its date.
fn version_heading(s: &str) -> Option<(Version, String)> {
    let (v, date) = s.split_once(" (")?;
    let date = date.strip_suffix(')')?;
    let version = Version::parse(v).filter(|v| !v.is_rc())?;
    super::release::is_date(date).then(|| (version, date.to_string()))
}

/// A list marker other than `- `.
fn is_list(s: &str) -> bool {
    let ordered = s.split_once(['.', ')']).is_some_and(|(n, rest)| {
        !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) && (rest.is_empty() || rest.starts_with(' '))
    });
    s.starts_with("- ") || s.starts_with("* ") || s.starts_with("+ ") || ordered
}

/// What a line starting a block may not be.
fn unsupported_start(line: &str) -> Option<&'static str> {
    if is_list(line) {
        return Some("a list other than `- ` items");
    }
    if line.starts_with('>') {
        return Some("a quote");
    }
    if line.starts_with("```") || line.starts_with("~~~") {
        return Some("a code block");
    }
    if line.starts_with('|') {
        return Some("a table");
    }
    let rule = |c: char| line.chars().all(|x| x == c || x == ' ') && line.chars().filter(|x| *x == c).count() >= 3;
    if rule('-') || rule('*') || rule('_') || rule('=') {
        return Some("a rule or an underlined heading");
    }
    None
}

/// The spans of a paragraph or list item.
fn inline(s: &str) -> Result<Vec<Span>, &'static str> {
    let mut spans = Vec::new();
    let mut text = String::new();
    let mut rest = s;
    let flush = |text: &mut String, spans: &mut Vec<Span>| {
        if !text.is_empty() {
            spans.push(Span::Text(std::mem::take(text)));
        }
    };
    while let Some(c) = rest.chars().next() {
        match c {
            '`' => {
                let ticks = rest.len() - rest.trim_start_matches('`').len();
                if ticks > 1 {
                    return Err("code marked with more than one backtick");
                }
                let end = rest[1..].find('`').ok_or("a backtick without its closing one")?;
                flush(&mut text, &mut spans);
                spans.push(Span::Code(rest[1..1 + end].to_string()));
                rest = &rest[end + 2..];
            }
            '*' if rest.starts_with("**") => {
                let end = rest[2..].find("**").ok_or("`**` without its closing one")?;
                let inner = &rest[2..2 + end];
                if inner.contains(['`', '*', '[', '<', '\\']) {
                    return Err("marks inside bold text");
                }
                flush(&mut text, &mut spans);
                spans.push(Span::Bold(inner.to_string()));
                rest = &rest[end + 4..];
            }
            '*' => return Err("a `*` outside code (emphasis on the site): write it in code (`5 * 3`) or as ×"),
            '_' if emphasis_underscore(s, rest) => {
                return Err("a `_` at a word's edge outside code (emphasis on the site): write it in code");
            }
            '!' if rest.starts_with("![") => return Err("an image"),
            '[' => {
                let close =
                    rest.find(']').filter(|i| rest[i + 1..].starts_with('(')).ok_or("a `[` that is not a link")?;
                let end = rest[close..].find(')').ok_or("a link without its closing `)`")?;
                let label = &rest[1..close];
                if label.contains(['`', '*', '[', '<', '\\']) {
                    return Err("marks inside a link's text");
                }
                text.push_str(label);
                rest = &rest[close + end + 1..];
            }
            // A tag or an address (`a < b` is text).
            '<' if rest[1..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '/' || c == '!') => {
                let end = rest.find('>').ok_or("a `<` that is not an address")?;
                let inner = &rest[1..end];
                if !(inner.starts_with("https://") || inner.starts_with("http://")) || inner.contains(' ') {
                    return Err("HTML");
                }
                text.push_str(inner);
                rest = &rest[end + 1..];
            }
            '\\' if rest[1..].starts_with(|c: char| c.is_ascii_punctuation()) => return Err("a backslash escape"),
            '&' if entity(rest) => return Err("an HTML entity"),
            _ => {
                text.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    flush(&mut text, &mut spans);
    Ok(spans)
}

/// An `_` that starts or ends emphasis: at a word's edge (inside a word, as in `snake_case`, it is
/// a letter).
fn emphasis_underscore(all: &str, rest: &str) -> bool {
    let at = all.len() - rest.len();
    let before = all[..at].chars().next_back();
    let after = rest[1..].chars().next();
    let word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    !(word(before) && word(after))
}

/// `&name;` or `&#123;`.
fn entity(rest: &str) -> bool {
    let body = &rest[1..];
    let len = body.find(';').unwrap_or(0);
    len > 0 && len <= 10 && body[..len].trim_start_matches('#').chars().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests;
