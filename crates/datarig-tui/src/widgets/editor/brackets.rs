//! Brackets: the one matching a bracket (`%`) and the unmatched one around a position (the
//! `i(` text objects). Brackets in strings, quoted identifiers, dollar bodies and comments do
//! not count, unless the search starts in one of these: then only the brackets in it count.

use super::Editor;
use super::motion::Pos;
use datarig_core::sql::lexer::{Tok, lex_in};
use unicode_segmentation::UnicodeSegmentation;

/// The brackets `%` knows, open and close.
const PAIRS: [(&str, &str); 3] = [("(", ")"), ("[", "]"), ("{", "}")];

/// What a search found in a region of lines.
enum Scan {
    Found(Pos),
    /// There is none (the search left the string it started in).
    None,
    /// The region ended first.
    More,
}

impl Editor {
    /// Byte ranges in the text of the tokens of lines `first..last` whose brackets are not
    /// code: strings, quoted identifiers, dollar bodies and comments.
    fn quoted_spans(&mut self, first: usize, last: usize) -> Vec<(usize, usize)> {
        let (base, region) = self.region_text(first, last);
        lex_in(&region, self.lang.dialect())
            .into_iter()
            .filter(|t| {
                matches!(t.kind, Tok::Str | Tok::Dollar | Tok::QuotedIdent | Tok::LineComment | Tok::BlockComment)
            })
            .map(|t| (base + t.start, base + t.end))
            .collect()
    }

    /// `%`: the bracket matching the first one at or after the cursor on its line.
    pub(super) fn match_pair(&mut self) -> Option<Pos> {
        let (c, open, close, forward) =
            self.lines[self.row].graphemes(true).enumerate().skip(self.col).find_map(|(i, g)| {
                PAIRS.iter().find_map(|&(o, c)| match g {
                    _ if g == o => Some((i, o, c, true)),
                    _ if g == c => Some((i, o, c, false)),
                    _ => None,
                })
            })?;
        self.unmatched((self.row, c), forward, open, close, true)
    }

    /// From `from` (not included), the first `close` going forward (or `open` going back) that
    /// is not matched by another bracket in between. `own`: when `from` is in a string or a
    /// comment, only the brackets in it count (else only those of the code do).
    pub(super) fn unmatched(&mut self, from: Pos, forward: bool, open: &str, close: &str, own: bool) -> Option<Pos> {
        let n = self.lines.len();
        let mut k = self.region.max(1);
        loop {
            let (first, last) =
                if forward { (from.0, (from.0 + k).min(n)) } else { (from.0.saturating_sub(k), from.0 + 1) };
            let spans = self.quoted_spans(first, last);
            match self.scan(&spans, (first, last), from, forward, (open, close), own) {
                Scan::Found(p) => return Some(p),
                Scan::None => return None,
                Scan::More if (forward && last == n) || (!forward && first == 0) => return None,
                Scan::More => k = k.saturating_mul(4),
            }
        }
    }

    /// [`Editor::unmatched`] within lines `first..last`, `spans` the strings and comments there.
    fn scan(
        &self,
        spans: &[(usize, usize)],
        (first, last): (usize, usize),
        from: Pos,
        forward: bool,
        (open, close): (&str, &str),
        own: bool,
    ) -> Scan {
        let span_at = |off: usize| {
            let i = spans.partition_point(|s| s.0 <= off);
            (i > 0 && spans[i - 1].1 > off).then(|| spans[i - 1])
        };
        let mut off = self.line_start(from.0);
        let inside = if own { span_at(off + self.byte_at(from.0, from.1)) } else { None };
        let mut depth = 0usize;
        // One grapheme: `Some` ends the search.
        let mut step = |at: usize, g: &str, p: Pos| -> Option<Scan> {
            match inside {
                Some((a, b)) if at < a || at >= b => return Some(Scan::None),
                Some(_) => {}
                None if span_at(at).is_some() => return None,
                None => {}
            }
            let (deeper, target) = if forward { (open, close) } else { (close, open) };
            if g == deeper {
                depth += 1;
            } else if g == target {
                if depth == 0 {
                    return Some(Scan::Found(p));
                }
                depth -= 1;
            }
            None
        };
        if forward {
            for r in from.0..last {
                let line = &self.lines[r];
                for (c, (b, g)) in line.grapheme_indices(true).enumerate() {
                    if r == from.0 && c <= from.1 {
                        continue;
                    }
                    if let Some(s) = step(off + b, g, (r, c)) {
                        return s;
                    }
                }
                off += line.len() + 1;
            }
        } else {
            for r in (first..=from.0).rev() {
                if r < from.0 {
                    off -= self.lines[r].len() + 1;
                }
                let gs: Vec<(usize, &str)> = self.lines[r].grapheme_indices(true).collect();
                let end = if r == from.0 { from.1.min(gs.len()) } else { gs.len() };
                for c in (0..end).rev() {
                    let (b, g) = gs[c];
                    if let Some(s) = step(off + b, g, (r, c)) {
                        return s;
                    }
                }
            }
        }
        Scan::More
    }

    /// The byte range of the string or comment `p` is in, if it is in one.
    pub(super) fn quoted_at(&mut self, p: Pos) -> Option<(usize, usize)> {
        let spans = self.quoted_spans(p.0, p.0 + 1);
        let off = self.offset_of(p.0, p.1);
        spans.into_iter().find(|&(a, b)| a <= off && off < b)
    }
}

#[cfg(test)]
mod tests;
