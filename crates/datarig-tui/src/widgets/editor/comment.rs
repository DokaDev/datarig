//! `gc`: comment lines out with the text's dialect's line comment (SQL's `-- `), or back in, as
//! Neovim's built-in commenting does with `commentstring` `-- %s`. When every line of the range
//! that is not blank starts with a marker the dialect strips (after its indent), they lose it;
//! otherwise every line gets the marker and a space after the smallest indent of the lines that
//! are not blank (a blank line becomes that indent and the marker).

use super::motion::Pos;
use super::{EdEvent, Editor};
use datarig_core::sql::dialect::Dialect;
use unicode_segmentation::UnicodeSegmentation;

/// What goes after the comment marker on a line that has text.
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

/// The lines toggled in dialect `d`: commented in ([`Dialect::comment_marker`]), or out when
/// every line that is not blank is a comment ([`Dialect::uncomment_markers`]).
pub(super) fn toggle(lines: &[String], d: Dialect) -> Vec<String> {
    let mark = d.comment_marker();
    let marks = d.uncomment_markers();
    let mut indent: Option<&str> = None;
    let mut commented = true;
    for l in lines.iter().filter(|l| !blank(l)) {
        let w = indent_len(l);
        if indent.is_none_or(|i| w < i.len()) {
            indent = Some(&l[..w]);
        }
        commented = commented && marks.iter().any(|m| l[w..].starts_with(m));
    }
    let indent = indent.unwrap_or("");
    lines
        .iter()
        .map(|l| match commented {
            true => uncomment(l, marks),
            false if blank(l) => format!("{indent}{mark}"),
            // Every line that is not blank has at least `indent`'s bytes of blanks.
            false => format!("{indent}{mark}{SPACE}{}", &l[indent.len()..]),
        })
        .collect()
}

/// `line` without its comment marker (the first of `marks` it starts with, and the space after
/// it); the indent goes too when only blanks are left.
fn uncomment(line: &str, marks: &[&str]) -> String {
    let w = indent_len(line);
    let rest = &line[w..];
    let Some(text) = marks.iter().find_map(|m| rest.strip_prefix(m)) else { return line.to_string() };
    let text = text.strip_prefix(SPACE).unwrap_or(text);
    if blank(text) { text.to_string() } else { format!("{}{text}", &line[..w]) }
}

impl Editor {
    /// `gc` over lines `first..=last` as one undo step; the cursor goes to `to` first (an undo
    /// puts it back there) and keeps its byte in the line, as Neovim's does.
    pub(super) fn comment_lines(&mut self, first: usize, last: usize, to: Pos) -> EdEvent {
        let new = toggle(&self.lines[first..=last], self.lang.dialect());
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
