//! Text objects (`iw`, `a(`, `i"`, `ip`, …): the text around the cursor an operator takes, or
//! that Visual mode selects (or grows to), found as Vim finds it.

use super::Editor;
use super::buffer::graphemes;
use super::motion::{Pos, Range, RangeKind, Walk};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Object {
    /// `iw` `aw`, `iW` `aW`.
    Word { big: bool },
    /// `i(` `a(` (`ib`), `i[`, `i{` (`iB`), `i<`.
    Block { open: &'static str, close: &'static str },
    /// `i"`, `i'`, `` i` ``: a string on the cursor's line.
    Quote(char),
    /// `ip` `ap`: lines up to a blank line.
    Paragraph,
}

impl Object {
    pub(super) fn of_char(c: char) -> Option<Object> {
        Some(match c {
            'w' => Object::Word { big: false },
            'W' => Object::Word { big: true },
            '(' | ')' | 'b' => Object::Block { open: "(", close: ")" },
            '[' | ']' => Object::Block { open: "[", close: "]" },
            '{' | '}' | 'B' => Object::Block { open: "{", close: "}" },
            '<' | '>' => Object::Block { open: "<", close: ">" },
            '"' | '\'' | '`' => Object::Quote(c),
            'p' => Object::Paragraph,
            _ => return None,
        })
    }
}

/// What a text object found: from `start` to `end`, taken as `kind` by an operator. In Visual
/// mode the selection goes from `start` to `end` (the grapheme before it when exclusive),
/// with the line break after `end` when `eol`, by line when `kind` is linewise, unless `keep`
/// (the selection keeps its mode).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Found {
    pub start: Pos,
    pub end: Pos,
    pub kind: RangeKind,
    pub eol: bool,
    pub keep: bool,
}

/// `incl` of Vim: one position on, past the end of a non-empty line.
fn incl(w: &mut Walk) -> i8 {
    let mut r = w.inc();
    if r >= 1 && w.c > 0 {
        r = w.inc();
    }
    r
}

/// `decl` of Vim: one position back, past the end of a non-empty line.
fn decl(w: &mut Walk) -> i8 {
    let mut r = w.dec();
    if r == 1 && w.c > 0 {
        r = w.dec();
    }
    r
}

/// Back to the start of the word or the blanks under the position, on its line.
fn back_in_line(w: &mut Walk) {
    let cls = w.cls();
    while w.c > 0 {
        w.dec();
        if w.cls() != cls {
            w.inc();
            break;
        }
    }
}

/// A line of blanks only (or empty): paragraphs end there.
fn blank(line: &str) -> bool {
    line.chars().all(|c| c == ' ' || c == '\t')
}

impl Editor {
    /// Object `o` taken `count` times (`around`: `a…`, else `i…`). `visual`: the other end of
    /// a Visual selection that is more than the cursor, which the object grows.
    pub(super) fn object(&mut self, o: Object, count: usize, around: bool, visual: Option<Pos>) -> Option<Found> {
        match o {
            Object::Word { big } => self.word_object(count, around, big, visual),
            Object::Block { open, close } => self.block_object(count, around, open, close, visual),
            Object::Quote(q) => self.quote_object(count, around, q),
            Object::Paragraph => self.paragraph_object(count, around, visual),
        }
    }

    /// The range an operator takes for object `o` (Vim's rule for exclusive ranges that end
    /// at the start of a line applies).
    pub(super) fn object_range(&mut self, o: Object, count: usize, around: bool) -> Option<Range> {
        let f = self.object(o, count, around, None)?;
        Some(self.exclusive_adjusted(Range { start: f.start, end: f.end, kind: f.kind }))
    }

    /// `iw` `aw` (`big`: `iW` `aW`), as Vim's `current_word`.
    fn word_object(&self, count: usize, include: bool, big: bool, visual: Option<Pos>) -> Option<Found> {
        let mut w = Walk::new(&self.lines, (self.row, self.col), big);
        let mut start = w.pos();
        let mut count = count;
        let mut inclusive = true;
        let mut include_white = false;
        if visual.is_none() {
            back_in_line(&mut w);
            start = w.pos();
            if (w.cls() == 0) == include {
                if !w.end_word(1, true, true) {
                    return None;
                }
            } else {
                w.fwd_word(1, true);
                if w.c == 0 {
                    decl(&mut w);
                } else {
                    w.dec();
                }
                include_white = include;
            }
            count -= 1;
        } else if let Some(anchor) = visual {
            start = anchor;
        }
        while count > 0 {
            inclusive = true;
            match visual {
                Some(anchor) if w.pos() < anchor => {
                    if decl(&mut w) == -1 {
                        return None;
                    }
                    if include != (w.cls() != 0) {
                        if !w.bck_word(1, true) {
                            return None;
                        }
                    } else {
                        if !w.bckend_word(1, true) {
                            return None;
                        }
                        incl(&mut w);
                    }
                }
                _ => {
                    if incl(&mut w) == -1 {
                        return None;
                    }
                    if include != (w.cls() == 0) {
                        if !w.fwd_word(1, true) && count > 1 {
                            return None;
                        }
                        if w.c == 0 {
                            inclusive = false;
                        } else {
                            w.dec();
                        }
                    } else if !w.end_word(1, true, true) {
                        return None;
                    }
                }
            }
            count -= 1;
        }
        if include_white && (w.cls() != 0 || (w.c == 0 && !inclusive)) {
            // No blanks after the words: take those before them instead, but not an indent.
            let mut s = Walk::new(&self.lines, start, big);
            if s.c > 0 {
                s.dec();
                back_in_line(&mut s);
                if s.cls() == 0 && s.c > 0 {
                    start = s.pos();
                }
            }
        }
        let kind = if inclusive { RangeKind::Inclusive } else { RangeKind::Exclusive };
        Some(Found { start, end: w.pos(), kind, eol: false, keep: false })
    }

    /// Grapheme `c` of line `r` is part of its indent.
    fn in_indent(&self, (r, c): Pos) -> bool {
        graphemes(&self.lines[r]).iter().take(c + 1).all(|g| *g == " " || *g == "\t") && c < self.gcount(r)
    }

    /// The block `count` levels out from `from` (not included): its brackets. `own`: from
    /// inside a string or a comment, the brackets in it (else those of the code). Outside
    /// any block, the next one after `from` (Vim's forward search, a closing bracket counting
    /// as one more to skip).
    fn block_around(&mut self, from: Pos, count: usize, (open, close): (&str, &str), own: bool) -> Option<(Pos, Pos)> {
        let start = match self.unmatched(from, false, open, close, own) {
            Some(mut p) => {
                for _ in 1..count {
                    p = self.unmatched(p, false, open, close, true)?;
                }
                p
            }
            None => {
                let mut p = from;
                for _ in 0..count {
                    p = self.unmatched(p, true, close, open, own)?;
                }
                p
            }
        };
        Some((start, self.unmatched(start, true, open, close, true)?))
    }

    /// `i(` `a(` and the other brackets, as Vim's `current_block`: `count` levels out from the
    /// cursor (on a bracket, the block it opens or closes). Inside, a closing bracket alone on
    /// its line leaves its line out, and so does an opening one at the end of its line.
    fn block_object(
        &mut self,
        count: usize,
        include: bool,
        open: &str,
        close: &str,
        visual: Option<Pos>,
    ) -> Option<Found> {
        let cursor = (self.row, self.col);
        let (mut old_start, mut old_end) = (cursor, cursor);
        let mut cur = cursor;
        match visual {
            None => {
                if open == "{" {
                    while self.in_indent(cur) {
                        let mut w = Walk::new(&self.lines, cur, false);
                        let moved = w.inc();
                        cur = w.pos();
                        if moved != 0 {
                            break;
                        }
                    }
                }
                if graphemes(&self.lines[cur.0]).get(cur.1) == Some(&open) {
                    cur.1 += 1;
                }
            }
            Some(anchor) if anchor < cursor => {
                old_start = anchor;
                cur = anchor;
            }
            Some(anchor) => old_end = anchor,
        }
        // From a string or a comment, the block in it, else the one around it.
        let quoted = cur.1 < self.gcount(cur.0) && self.quoted_at(cur).is_some();
        let (mut start, mut end) = self
            .block_around(cur, count, (open, close), true)
            .or_else(|| if quoted { self.block_around(cur, count, (open, close), false) } else { None })?;
        let mut sol = false;
        let mut grown = false;
        if !include {
            loop {
                let mut s = Walk::new(&self.lines, start, false);
                incl(&mut s);
                start = s.pos();
                sol = end.1 == 0;
                let mut e = Walk::new(&self.lines, end, false);
                decl(&mut e);
                while e.c < e.line_len() && self.in_indent(e.pos()) {
                    sol = true;
                    if decl(&mut e) != 0 {
                        break;
                    }
                }
                end = e.pos();
                // In Visual mode an object no bigger than the selection grows to the next one out.
                if visual.is_some() && !grown && start >= old_start && old_end >= end && start != end {
                    let mut w = Walk::new(&self.lines, old_start, false);
                    decl(&mut w);
                    start = self.unmatched(w.pos(), false, open, close, true)?;
                    grown = true;
                    end = self.unmatched(start, true, open, close, true)?;
                    continue;
                }
                break;
            }
        }
        if visual.is_some() || self.mode == super::Mode::Visual {
            let eol = sol && end.1 < self.gcount(end.0);
            return Some(Found { start, end, kind: RangeKind::Inclusive, eol, keep: false });
        }
        if sol {
            let mut e = Walk::new(&self.lines, end, false);
            incl(&mut e);
            return Some(Found { start, end: e.pos(), kind: RangeKind::Exclusive, eol: false, keep: false });
        }
        if start <= end {
            Some(Found { start, end, kind: RangeKind::Inclusive, eol: false, keep: false })
        } else {
            // Nothing between the brackets.
            Some(Found { start, end: start, kind: RangeKind::Exclusive, eol: false, keep: false })
        }
    }

    /// `i"` `a"` and the other quotes, as Vim's `current_quote`: the string the cursor is in
    /// or on, else the next one on its line; `a…` with the blanks after it (or before it when
    /// there are none after). A backslash escapes a quote.
    fn quote_object(&self, count: usize, include: bool, q: char) -> Option<Found> {
        let gs: Vec<char> = graphemes(&self.lines[self.row]).iter().map(|g| g.chars().next().unwrap_or(' ')).collect();
        let is_q = |i: usize| gs.get(i) == Some(&q);
        let next_quote = |mut i: usize, escape: bool| -> Option<usize> {
            while i < gs.len() {
                if escape && gs[i] == '\\' {
                    i += 1;
                } else if gs[i] == q {
                    return Some(i);
                }
                i += 1;
            }
            None
        };
        let prev_quote = |mut i: usize| -> usize {
            while i > 0 {
                i -= 1;
                let escapes = gs[..i].iter().rev().take_while(|c| **c == '\\').count();
                if escapes % 2 == 1 {
                    i -= escapes;
                } else if gs[i] == q {
                    break;
                }
            }
            i
        };
        let col = self.col.min(gs.len());
        let (mut a, mut b);
        if is_q(col) {
            // On a quote: opening or closing, as counted from the start of the line.
            a = 0;
            loop {
                a = next_quote(a, false).filter(|&s| s <= col)?;
                b = next_quote(a + 1, true)?;
                if a <= col && col <= b {
                    break;
                }
                a = b + 1;
            }
        } else {
            a = prev_quote(col);
            if !is_q(a) {
                a = next_quote(a, false)?;
            }
            b = next_quote(a + 1, true)?;
        }
        if include {
            if gs.get(b + 1).is_some_and(|c| *c == ' ' || *c == '\t') {
                while gs.get(b + 1).is_some_and(|c| *c == ' ' || *c == '\t') {
                    b += 1;
                }
            } else {
                while a > 0 && (gs[a - 1] == ' ' || gs[a - 1] == '\t') {
                    a -= 1;
                }
            }
        }
        let r = self.row;
        if !include && count < 2 {
            return Some(Found { start: (r, a + 1), end: (r, b), kind: RangeKind::Exclusive, eol: false, keep: false });
        }
        Some(Found { start: (r, a), end: (r, b), kind: RangeKind::Inclusive, eol: false, keep: false })
    }

    /// `ip` `ap`, as Vim's `current_par`: the paragraph (lines up to a blank one) or the blank
    /// lines under the cursor, `count` of them; `ap` with the blank lines after (or before).
    /// In Visual mode over more than one line, the selection grows by as many.
    fn paragraph_object(&self, count: usize, include: bool, visual: Option<Pos>) -> Option<Found> {
        let white = |r: usize| blank(&self.lines[r]);
        let last = self.lines.len() - 1;
        let mut start = self.row;
        if let Some(anchor) = visual.filter(|a| a.0 != self.row) {
            return self.extend_paragraphs(count, include, anchor);
        }
        let white_in_front = white(start);
        while start > 0 {
            if white_in_front {
                if !white(start - 1) {
                    break;
                }
            } else if white(start - 1) {
                break;
            }
            start -= 1;
        }
        // The end of the blank lines at the start (the line before the start when there are
        // none).
        let last = last as isize;
        let white = |r: isize| blank(&self.lines[r as usize]);
        let mut end = start as isize;
        while end <= last && white(end) {
            end += 1;
        }
        end -= 1;
        let mut i = count;
        if !include && white_in_front {
            i -= 1;
        }
        while i > 0 {
            i -= 1;
            if end == last {
                return None;
            }
            let do_white = !include && white(end + 1);
            if include || !do_white {
                end += 1;
                while end < last && !white(end + 1) {
                    end += 1;
                }
            }
            if i == 0 && white_in_front && include {
                break;
            }
            if include || do_white {
                while end < last && white(end + 1) {
                    end += 1;
                }
            }
        }
        let end = end.max(start as isize) as usize;
        if !white_in_front && !white(end as isize) && include {
            while start > 0 && white(start as isize - 1) {
                start -= 1;
            }
        }
        Some(Found { start: (start, 0), end: (end, 0), kind: RangeKind::Linewise, eol: false, keep: false })
    }

    /// `ip` / `ap` over a Visual selection of several lines: `count` more paragraphs (or runs
    /// of blank lines) on the cursor's side.
    fn extend_paragraphs(&self, count: usize, include: bool, anchor: Pos) -> Option<Found> {
        let white = |r: usize| blank(&self.lines[r]);
        let last = self.lines.len() - 1;
        let back = self.row < anchor.0;
        let edge = if back { 0 } else { last };
        let step = |r: usize| if back { r - 1 } else { r + 1 };
        let mut r = self.row;
        for _ in 0..count {
            if r == edge {
                return None;
            }
            let mut prev_white = None;
            for _ in 0..2 {
                r = step(r);
                let w = white(r);
                if prev_white == Some(w) {
                    r = if back { r + 1 } else { r - 1 };
                    break;
                }
                while r != edge && w == white(step(r)) {
                    r = step(r);
                }
                if !include || r == edge {
                    break;
                }
                prev_white = Some(w);
            }
        }
        Some(Found { start: anchor, end: (r, 0), kind: RangeKind::Linewise, eol: false, keep: true })
    }
}

#[cfg(test)]
mod tests;
