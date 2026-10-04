//! `gc`: comment lines out with SQL's `-- `, or back in, as Neovim's built-in commenting does
//! with `commentstring` `-- %s`. When every line of the range that is not blank starts with
//! `--` (after its indent), they lose it; otherwise every line gets `-- ` after the smallest
//! indent of the lines that are not blank (a blank line becomes that indent and `--`).

use super::motion::Pos;
use super::{EdEvent, Editor};
use unicode_segmentation::UnicodeSegmentation;

/// The comment marker, and what goes after it on a line that has text.
const MARK: &str = "--";
const SPACE: &str = " ";

/// A blank as Lua's `%s` takes it (Neovim matches the lines with Lua patterns).
fn space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// Bytes of the blanks `line` starts with.
fn indent_len(line: &str) -> usize {
    line.bytes().take_while(|b| space(*b)).count()
}

fn blank(line: &str) -> bool {
    indent_len(line) == line.len()
}

/// The lines toggled: commented in, or out when every line that is not blank is a comment.
pub(super) fn toggle(lines: &[String]) -> Vec<String> {
    let mut indent: Option<&str> = None;
    let mut commented = true;
    for l in lines.iter().filter(|l| !blank(l)) {
        let w = indent_len(l);
        if indent.is_none_or(|i| w < i.len()) {
            indent = Some(&l[..w]);
        }
        commented = commented && l[w..].starts_with(MARK);
    }
    let indent = indent.unwrap_or("");
    lines
        .iter()
        .map(|l| match commented {
            true => uncomment(l),
            false if blank(l) => format!("{indent}{MARK}"),
            // Every line that is not blank has at least `indent`'s bytes of blanks.
            false => format!("{indent}{MARK}{SPACE}{}", &l[indent.len()..]),
        })
        .collect()
}

/// `line` without its comment marker (and the space after it); the indent goes too when only
/// blanks are left.
fn uncomment(line: &str) -> String {
    let w = indent_len(line);
    let rest = &line[w..];
    let Some(text) = rest.strip_prefix(MARK) else { return line.to_string() };
    let text = text.strip_prefix(SPACE).unwrap_or(text);
    if blank(text) { text.to_string() } else { format!("{}{text}", &line[..w]) }
}

impl Editor {
    /// `gc` over lines `first..=last` as one undo step; the cursor goes to `to` first (an undo
    /// puts it back there) and keeps its byte in the line, as Neovim's does.
    pub(super) fn comment_lines(&mut self, first: usize, last: usize, to: Pos) -> EdEvent {
        let new = toggle(&self.lines[first..=last]);
        let changed = new.iter().zip(&self.lines[first..=last]).any(|(n, o)| n != o);
        self.set_pos(to.0, to.1);
        // Neovim counts it as a change even when the lines stay as they are.
        self.marks.set_change((first, 0));
        self.rec.repeat_unchanged();
        if changed {
            let byte = self.byte_at(self.row, self.col);
            self.snapshot();
            let (a, _) = self.line_bounds(first);
            let (_, b) = self.line_bounds(last);
            self.splice(a, b, &new.join("\n"));
            let line = &self.lines[self.row];
            let c = line.grapheme_indices(true).take_while(|(i, _)| *i <= byte).count().saturating_sub(1);
            self.set_pos(self.row, c);
        }
        if changed { EdEvent::Changed { typed: None } } else { EdEvent::Moved }
    }
}

#[cfg(test)]
mod tests;
