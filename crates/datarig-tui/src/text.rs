//! Display-width helpers.
//!
//! Width is always computed per grapheme cluster with `unicode-width` 0.2, which is the
//! same function ratatui uses for its buffer diff. Keeping both in agreement is what keeps
//! the grid's `│` separators aligned. unicode-width 0.2 already implements the rules:
//! W/F = 2, emoji presentation / ZWJ / flag sequences = 2, combining marks = 0, ambiguous = 1.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Display width of a single grapheme cluster.
pub fn grapheme_width(g: &str) -> usize {
    UnicodeWidthStr::width(g)
}

/// Display width of a string, summed per grapheme cluster.
pub fn width(s: &str) -> usize {
    s.graphemes(true).map(grapheme_width).sum()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

/// Make a single-line cell value: newline -> `↵`, tab -> `→`, other control chars -> U+FFFD.
pub fn sanitize_cell(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('↵');
            }
            '\n' => out.push('↵'),
            '\t' => out.push('→'),
            c if c.is_control() => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
    out
}

/// Truncate `s` so that it occupies exactly `w` columns, ending with `…`.
/// Never splits a grapheme cluster. If a wide grapheme leaves one column free,
/// that column is padded with a space. Caller guarantees `width(s) > w`.
fn truncate_exact(s: &str, w: usize) -> String {
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut acc = 0;
    for g in s.graphemes(true) {
        let gw = grapheme_width(g);
        if acc + gw + 1 > w {
            break;
        }
        out.push_str(g);
        acc += gw;
    }
    out.push('…');
    acc += 1;
    while acc < w {
        out.push(' ');
        acc += 1;
    }
    out
}

/// Fit `s` into exactly `w` columns: pad (left/right aligned) or truncate with `…`.
pub fn fit(s: &str, w: usize, align: Align) -> String {
    let sw = width(s);
    if sw > w {
        return truncate_exact(s, w);
    }
    let pad = " ".repeat(w - sw);
    match align {
        Align::Left => format!("{s}{pad}"),
        Align::Right => format!("{pad}{s}"),
    }
}

/// Clip `s` to at most `w` columns (no padding). Adds `…` when clipped.
pub fn clip(s: &str, w: usize) -> String {
    if width(s) <= w { s.to_string() } else { truncate_exact(s, w).trim_end_matches(' ').to_string() }
}

/// Word-agnostic hard wrap used by the cell viewer: keeps explicit newlines, expands tabs
/// to 4 spaces, and breaks lines at grapheme boundaries when they exceed `w` columns.
pub fn wrap(s: &str, w: usize) -> Vec<String> {
    let w = w.max(2);
    let mut out = Vec::new();
    for raw in s.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        let mut line = String::new();
        let mut acc = 0;
        for g in raw.graphemes(true) {
            let (g, gw) = if g == "\t" { ("    ", 4) } else { (g, grapheme_width(g)) };
            if acc + gw > w && acc > 0 {
                out.push(std::mem::take(&mut line));
                acc = 0;
            }
            line.push_str(g);
            acc += gw;
        }
        out.push(line);
    }
    out
}

/// Wrap prose at spaces so words stay whole; a word wider than `w` is broken like [`wrap`].
pub fn wrap_words(s: &str, w: usize) -> Vec<String> {
    let w = w.max(2);
    let mut out: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in s.split(' ').filter(|x| !x.is_empty()) {
        let sep = usize::from(!line.is_empty());
        if width(&line) + sep + width(word) <= w {
            if sep == 1 {
                line.push(' ');
            }
            line.push_str(word);
            continue;
        }
        if !line.is_empty() {
            out.push(std::mem::take(&mut line));
        }
        let mut parts = wrap(word, w);
        line = parts.pop().unwrap_or_default();
        out.extend(parts);
    }
    if !line.is_empty() || out.is_empty() {
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests;
