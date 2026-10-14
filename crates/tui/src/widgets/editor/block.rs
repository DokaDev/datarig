//! Visual mode by block (`Ctrl+V`): screen columns `start..=end` of a range of lines, and the
//! operators on it, as Vim does them.
//!
//! A block is made of screen columns, so a wide character or a tab may be only partly inside
//! it. What each operator then does follows Vim's rules ([`prep`]): `y` takes blanks for the
//! part inside; `d`, `c`, `r` take the whole character and leave blanks for the part outside;
//! `I` and `A` put the text before or after it with blanks for the part on their side; `~`
//! leaves it alone. A line too short for the block is left out by `I`, `c`, `>`, `<`; `A`
//! fills it with blanks up to the block's end first. After `$` the block reaches the end of
//! every line.
//!
//! Every operator is one undo step, and changes the block's lines with one splice over all of
//! them (the text typed for `I`, `A`, `c`, and a put, are one more each): its work grows with
//! the text the block covers, not with the square of its lines.

use super::buffer::{UNDO_BYTES, graphemes, gw};
use super::edit::{Case, recase};
use super::marks::Hint;
use super::registers::{RegKind, RegProblem, Registers};
use super::repeat::{Input, InsertRepeat};
use super::{EdEvent, Editor, Mode, Sel};
use crate::text::grapheme_width;
use unicode_segmentation::{GraphemeIndices, UnicodeSegmentation};

/// A block: lines `first..=last`, screen columns `start..=end`; `max`: to the end of each
/// line (after `$`; `end` is then the longest line's width once [`Editor::filled`] set it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Block {
    pub(super) first: usize,
    pub(super) last: usize,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) max: bool,
}

/// What an operator does with a character partly inside the block (Vim's `block_prep`
/// operator kinds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prep {
    Yank,
    Delete,
    Replace,
    Case,
    Insert,
    Append,
    ShiftLeft,
    ShiftRight,
}

impl Prep {
    /// The operator takes a character partly inside whole (a delete in Vim's terms).
    fn del(self) -> bool {
        !matches!(self, Prep::Yank | Prep::Case)
    }
}

/// What the block takes of one line: the bytes `col..col + len`, and the blanks that stand
/// for the part of a character that is inside (or, when the operator takes it whole, outside)
/// at the block's start and end. `short`: the line ends before the block does. `start_x` /
/// `end_x` are the screen columns where the characters at the block's edges end, `start_w` /
/// `end_w` their widths; `pre_white` the columns of the blanks just before the block
/// (`pre_white_n` of them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Piece {
    col: usize,
    len: usize,
    start_spaces: usize,
    end_spaces: usize,
    short: bool,
    start_x: usize,
    end_x: usize,
    start_w: usize,
    end_w: usize,
    pre_white: usize,
    pre_white_n: usize,
}

/// The graphemes of a line with their byte offsets. A line of ASCII only has one per byte
/// (ASCII characters never join), found without segmenting it.
enum Cells<'a> {
    Ascii(&'a str, usize),
    Text(GraphemeIndices<'a>),
}

fn cells(line: &str) -> Cells<'_> {
    if line.is_ascii() { Cells::Ascii(line, 0) } else { Cells::Text(line.grapheme_indices(true)) }
}

impl<'a> Iterator for Cells<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Cells::Ascii(line, i) => {
                let at = *i;
                (at < line.len()).then(|| {
                    *i += 1;
                    (at, &line[at..at + 1])
                })
            }
            Cells::Text(g) => g.next(),
        }
    }
}

/// Display width of `line`.
fn line_width(line: &str) -> usize {
    cells(line).map(|(_, g)| gw(g)).sum()
}

fn white(g: &str) -> bool {
    g == " " || g == "\t"
}

/// What block `b` takes of `line` for operator `op` (Vim's `block_prep`, line by line).
fn prep(line: &str, b: &Block, op: Prep) -> Piece {
    let del = op.del();
    let mut p = Piece::default();
    let mut it = cells(line);
    let (mut x, mut incr, mut prev_start, mut start) = (0, 0, 0, 0);
    while x < b.start {
        let Some((i, g)) = it.next() else { break };
        incr = gw(g);
        x += incr;
        if white(g) {
            p.pre_white += incr;
            p.pre_white_n += 1;
        } else {
            p.pre_white = 0;
            p.pre_white_n = 0;
        }
        prev_start = i;
        start = i + g.len();
    }
    p.start_x = x;
    p.start_w = incr;
    if x < b.start {
        // The line ends before the block.
        p.end_x = x;
        p.short = true;
        if !del || op == Prep::Append {
            p.end_spaces = b.end - b.start + 1;
        }
        p.col = start;
        return p;
    }
    p.start_spaces = x - b.start;
    if del && p.start_spaces > 0 {
        p.start_spaces = p.start_w.saturating_sub(p.start_spaces);
    }
    let mut end = start;
    p.end_x = x;
    if x > b.end {
        // The block lies inside one character.
        match op {
            Prep::Insert => p.end_spaces = p.start_w.saturating_sub(p.start_spaces),
            Prep::Append => {
                p.start_spaces += b.end - b.start + 1;
                p.end_spaces = p.start_w.saturating_sub(p.start_spaces);
            }
            _ => {
                p.start_spaces = b.end - b.start + 1;
                if del && op != Prep::ShiftLeft {
                    p.start_spaces = p.start_w.saturating_sub(x - b.start);
                    p.end_spaces = x - b.end - 1;
                }
            }
        }
    } else {
        let mut prev_end = end;
        while x <= b.end {
            let Some((i, g)) = it.next() else { break };
            prev_end = i;
            incr = gw(g);
            x += incr;
            end = i + g.len();
        }
        p.end_x = x;
        if x <= b.end && (!del || op == Prep::Append || op == Prep::Replace) {
            p.short = true;
            if op == Prep::Append {
                p.end_spaces = b.end - x + 1;
            }
        } else if x > b.end {
            p.end_spaces = x - b.end - 1;
            if !del && p.end_spaces > 0 {
                p.end_spaces = incr - p.end_spaces;
                if end != start {
                    end = prev_end;
                }
            }
        }
    }
    p.end_w = incr;
    if del && p.start_spaces > 0 {
        start = prev_start;
    }
    p.len = end - start;
    p.col = start;
    p
}

/// The piece of `line` a yank of block `b` takes: blanks for the parts of characters inside.
fn yank_piece(line: &str, b: &Block) -> String {
    let p = prep(line, b, Prep::Yank);
    let mut s = String::with_capacity(p.start_spaces + p.len + p.end_spaces);
    s.push_str(&" ".repeat(p.start_spaces));
    s.push_str(&line[p.col..p.col + p.len]);
    s.push_str(&" ".repeat(p.end_spaces));
    s
}

/// The character starting at byte `at` of `line` (empty at its end).
fn grapheme_at(line: &str, at: usize) -> &str {
    line[at..].graphemes(true).next().unwrap_or("")
}

/// Line `line` with block `b` shifted right or left by `amount` columns (Vim's
/// `shift_block`, with `expandtab`: the blanks around the block's start are written as
/// spaces); `None` when it does not change.
fn shift_line(line: &str, b: &Block, right: bool, amount: usize) -> Option<String> {
    if line.is_empty() {
        return None;
    }
    let p = prep(line, b, if right { Prep::ShiftRight } else { Prep::ShiftLeft });
    if p.short {
        return None;
    }
    if right {
        let mut total = amount + p.pre_white;
        let mut start_spaces = p.start_spaces;
        let mut text = p.col;
        if start_spaces > 0 {
            // A tab split by the block's start is part of the blanks; a wide character stays.
            let g = grapheme_at(line, text);
            if g.len() == 1 {
                text += 1;
            } else {
                start_spaces = 0;
            }
        }
        for g in line[text..].graphemes(true).take_while(|g| white(g)) {
            total += gw(g);
            text += g.len();
        }
        let col = p.col - (p.pre_white_n - usize::from(start_spaces != 0));
        return Some(format!("{}{}{}", &line[..col], " ".repeat(total), &line[text..]));
    }
    let mut non_white = p.col;
    if p.start_spaces > 0 {
        non_white += grapheme_at(line, non_white).len();
    }
    let mut non_white_x = p.start_x;
    for g in line[non_white..].graphemes(true).take_while(|g| white(g)) {
        non_white_x += gw(g);
        non_white += g.len();
    }
    let shift = (non_white_x - b.start).min(amount);
    if shift == 0 {
        return None;
    }
    let dest = non_white_x - shift;
    let mut keep = p.col;
    let mut keep_x = if p.start_spaces > 0 { p.start_x - p.start_w } else { p.start_x };
    for g in line[keep..].graphemes(true) {
        let w = gw(g);
        if keep_x >= dest || keep_x + w > dest {
            break;
        }
        keep_x += w;
        keep += g.len();
    }
    Some(format!("{}{}{}", &line[..keep], " ".repeat(dest - keep_x), &line[non_white..]))
}

/// Line `line` with every character of block `b` replaced by `ch` (Vim's block `r`): a wide
/// `ch` takes two columns (a column left over becomes a blank), a character partly inside
/// leaves blanks for its part outside; a line break splits the line there (two lines). `None`:
/// the block takes nothing of the line.
fn replace_line(line: &str, b: &Block, ch: char) -> Option<String> {
    let mut p = prep(line, b, Prep::Replace);
    if p.len == 0 {
        return None;
    }
    let mut n = b.end - b.start + 1;
    if p.short {
        n -= (b.end - p.end_x) + 1;
    }
    let cells = if ch == '\t' { 1 } else { grapheme_width(ch.encode_utf8(&mut [0; 4])) };
    if cells > 1 {
        if n % 2 == 1 && !p.short {
            p.end_spaces += 1;
        }
        n /= 2;
    }
    let before = format!("{}{}", &line[..p.col], " ".repeat(p.start_spaces));
    let after = &line[p.col + p.len..];
    if ch == '\n' {
        // The line breaks there: what follows the block goes on a line of its own.
        return Some(format!("{before}\n{after}"));
    }
    let mut s = before;
    s.push_str(&ch.to_string().repeat(n));
    if !p.short {
        s.push_str(&" ".repeat(p.end_spaces));
        s.push_str(after);
    }
    Some(s)
}

/// Line `line` with `text` inserted before block `b` (`I`, `append` false) or after it (`A`)
/// as Vim's `block_insert` does it on the block's other lines. `None`: `I` leaves out a line
/// that ends before the block.
fn insert_line(line: &str, b: &Block, text: &str, append: bool) -> Option<String> {
    let p = prep(line, b, if append { Prep::Append } else { Prep::Insert });
    if p.short && !append {
        return None;
    }
    let (w, spaces, mut at) = if !append {
        (p.start_w, p.start_spaces, p.col)
    } else if !p.short {
        let spaces = if p.end_spaces > 0 { p.end_w - p.end_spaces } else { 0 };
        (p.end_w, spaces, p.col + p.len - usize::from(spaces != 0))
    } else {
        // Blanks up to the block's end, unless it reaches the end of every line.
        let spaces = if b.max { 0 } else { b.end - p.end_x + 1 };
        (p.end_w, spaces, p.col + p.len)
    };
    if spaces > 0 {
        // Back to the start of the character the block splits.
        at = grapheme_start(line, at);
    }
    let mut s = String::with_capacity(line.len() + spaces + text.len() + w);
    s.push_str(&line[..at]);
    s.push_str(&" ".repeat(spaces));
    s.push_str(text);
    let mut rest = &line[at..];
    if spaces > 0 && !p.short && rest.starts_with('\t') {
        // A split tab becomes blanks on both sides of the text.
        s.push_str(&" ".repeat(w - spaces));
        rest = &rest[1..];
    }
    s.push_str(rest);
    Some(s)
}

/// The start of the character (grapheme) byte `at` of `line` falls in.
fn grapheme_start(line: &str, at: usize) -> usize {
    let mut start = 0;
    for (i, g) in line.grapheme_indices(true) {
        if i + g.len() > at {
            return i;
        }
        start = i + g.len();
    }
    start
}

/// The Insert session of a block's `I`, `A` or `c`: when it ends on line `row`, the text it
/// added from byte `at` (the line's length was `len`) goes on the block's other lines.
#[derive(Clone, Copy, Debug)]
pub(super) struct BlockInsert {
    block: Block,
    /// `A` puts the text after the block, `I` and `c` before.
    append: bool,
    /// `c`: the block's text is gone, the text goes where it started.
    change: bool,
    row: usize,
    at: usize,
    len: usize,
    /// Where the cursor goes when text was added (`I`, `A`: the block's start).
    cursor: Option<usize>,
}

impl Editor {
    /// The Visual block: its lines, and its columns from where the characters at its corners
    /// start to where they end (after `$` from the cursor's line end, and to the end of every
    /// line). `.` takes a block as wide as it took before, from the cursor.
    pub(super) fn block(&self) -> Block {
        let span = |(r, c): (usize, usize)| {
            let x = self.display_x(r, c);
            let w = graphemes(&self.lines[r]).get(c).map_or(1, |g| gw(g).max(1));
            (x, x + w - 1)
        };
        let (a, c) = (self.anchor, (self.row, self.col));
        let (first, last) = (a.0.min(c.0), a.0.max(c.0));
        let (s1, e1) = span(a);
        if let Some(w) = self.redo_width {
            let max = w == usize::MAX;
            let end = if max { s1 } else { s1 + w - 1 };
            return Block { first, last, start: s1, end, max };
        }
        let max = self.want_x == Some(usize::MAX);
        let (s2, e2) = if max {
            let w = self.display_x(c.0, self.gcount(c.0));
            (w, w)
        } else {
            span(c)
        };
        Block { first, last, start: s1.min(s2), end: e1.max(e2), max }
    }

    /// Block `b` as an operator takes it: to the end of the longest line after `$`.
    fn filled(&self, mut b: Block) -> Block {
        if b.max {
            let widths = self.lines[b.first..=b.last].iter().map(|l| line_width(l));
            b.end = widths.max().unwrap_or(0).max(b.start);
        }
        b
    }

    /// The block an operator takes ([`Editor::filled`]), recorded for `.`.
    fn op_block(&mut self, b: Block) -> Block {
        let b = self.filled(b);
        if b.max {
            self.block_work += self.span_bytes(&b);
        }
        let width = if b.max { usize::MAX } else { b.end - b.start + 1 };
        self.rec.select(Input::Block { lines: b.last - b.first + 1, width });
        b
    }

    /// Bytes of the lines of block `b`.
    fn span_bytes(&self, b: &Block) -> usize {
        self.lines[b.first..=b.last].iter().map(|l| l.len() + 1).sum()
    }

    /// The text of the Visual block as `y` takes it: one piece per line.
    pub(super) fn selected_block_text(&self) -> String {
        self.block_text(&self.filled(self.block()))
    }

    /// The text of block `b` as `y` takes it: one piece per line.
    fn block_text(&self, b: &Block) -> String {
        let mut s = String::new();
        for (i, line) in self.lines[b.first..=b.last].iter().enumerate() {
            if i > 0 {
                s.push('\n');
            }
            s.push_str(&yank_piece(line, b));
        }
        s
    }

    /// Bytes the block operators walked since the last call (the benchmark counts them).
    pub fn take_block_work(&mut self) -> usize {
        std::mem::take(&mut self.block_work)
    }

    /// The grapheme of line `r` that byte `b` starts or falls in.
    fn col_of_byte(&self, r: usize, b: usize) -> usize {
        let line = &self.lines[r];
        let mut b = b.min(line.len());
        while !line.is_char_boundary(b) {
            b -= 1;
        }
        line[..b].graphemes(true).count()
    }

    /// Replace lines `b.first..=b.last` with `new` (one splice); the block's lines after
    /// `b.first` that `new` does not hold keep their text.
    fn replace_block_lines(&mut self, first: usize, last: usize, new: &str) {
        let (a, _) = self.line_bounds(first);
        let end = a + self.lines[first..=last].iter().map(|l| l.len() + 1).sum::<usize>() - 1;
        self.block_work += self.lines[..first].len() + (end - a);
        // `'.` at the block's top left, where the operator put the cursor (Vim).
        let col = if self.row == first { self.col } else { 0 };
        self.splice(a, end, new);
        self.marks.set_change((first, col));
    }

    /// Lines `first..=last` made by `f` from each line (`None`: it stays), spliced in one go
    /// when any changed (as a new undo step with `snapshot`); whether one did.
    fn edit_block_lines(
        &mut self,
        first: usize,
        last: usize,
        snapshot: bool,
        mut f: impl FnMut(&str) -> Option<String>,
    ) -> bool {
        let mut out = String::new();
        let mut changed = false;
        for (i, line) in self.lines[first..=last].iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            match f(line) {
                Some(new) if new != *line => {
                    changed = true;
                    out.push_str(&new);
                }
                _ => out.push_str(line),
            }
        }
        if changed {
            if snapshot {
                self.snapshot();
            }
            self.replace_block_lines(first, last, &out);
        }
        changed
    }

    /// Leave Visual mode with the cursor on the block's top left corner (where Vim's
    /// operators start: past the end of a line too short to reach it, until the operator
    /// keeps the cursor on the text), and start an undo step there when `snapshot`.
    fn start_block_op(&mut self, b: &Block, snapshot: bool) {
        self.mode = Mode::Normal;
        self.row = b.first;
        self.col = self.col_for_x(b.first, b.start);
        self.want_x = None;
        if snapshot {
            self.snapshot();
        }
    }

    /// The register the command names takes a yank or delete; a register Vim fills itself
    /// (`".`) takes none and the command fails.
    fn block_register_ok(&mut self) -> bool {
        if Registers::can_write(self.reg) {
            return true;
        }
        self.problem = self.reg.map(RegProblem::ReadOnly);
        let b = self.block();
        self.start_block_op(&b, false);
        self.clamp();
        false
    }

    /// The command keys of Visual mode by block.
    pub(super) fn block_command(&mut self, c: char, n: usize) -> EdEvent {
        match c {
            'O' => self.block_other_corner(),
            'y' | 'Y' => self.block_yank(),
            'd' | 'x' | 'X' => self.block_delete(false),
            'D' => self.block_delete(true),
            'c' | 's' => self.block_change(false),
            'C' => self.block_change(true),
            'I' => self.block_insert(false, n),
            'A' => self.block_insert(true, n),
            'u' | 'U' | '~' => self.block_case(c),
            '>' | '<' => self.block_shift(c == '>', n),
            'p' | 'P' => self.block_put(c == 'P', n),
            // Whole lines, as by line.
            'S' | 'R' => {
                self.sel = Sel::Lines;
                self.visual_command('S', n)
            }
            _ => self.visual_command(c, n),
        }
    }

    /// `O`: the cursor goes to the other corner on its line (the other end goes to the other
    /// side too); when that is where it is, both go to the other line's corners. Corners are
    /// screen columns: on a line too short to reach one, the end of the line.
    fn block_other_corner(&mut self) -> EdEvent {
        let b = self.block();
        let old = self.col;
        self.anchor.1 = self.col_for_x(self.anchor.0, b.start);
        let c = self.col_for_x(self.row, b.end);
        if c == old {
            self.anchor.1 = self.col_for_x(self.anchor.0, b.end);
            self.col = self.col_for_x(self.row, b.start);
            self.want_x = Some(b.start);
        } else {
            self.col = c;
            self.want_x = Some(b.end);
        }
        EdEvent::Moved
    }

    /// `y`: the block's pieces into the register, the cursor on its top left corner.
    fn block_yank(&mut self) -> EdEvent {
        if !self.block_register_ok() {
            return EdEvent::Moved;
        }
        let b = self.filled(self.block());
        let text = self.block_text(&b);
        self.block_work += self.span_bytes(&b);
        self.start_block_op(&b, false);
        self.clamp();
        self.yank_to_register(text, RegKind::Blockwise);
        EdEvent::Moved
    }

    /// Delete block `b` (registers as `d` writes them unless `register` is false), the cursor
    /// where it started on the first line (or before, on a line it left shorter). Text was
    /// deleted.
    fn delete_block(&mut self, b: &Block, register: bool) -> bool {
        // A block of one empty line takes nothing, not even the registers (Vim).
        let register = register && !(b.first == b.last && self.lines[b.first].is_empty());
        if register {
            let text = self.block_text(b);
            self.delete_to_register(text, RegKind::Blockwise);
        }
        self.block_work += (1 + usize::from(register)) * self.span_bytes(b);
        let first = prep(&self.lines[b.first], b, Prep::Delete);
        let changed = self.edit_block_lines(b.first, b.last, false, |line| {
            let p = prep(line, b, Prep::Delete);
            (p.len > 0).then(|| {
                let spaces = " ".repeat(p.start_spaces + p.end_spaces);
                format!("{}{spaces}{}", &line[..p.col], &line[p.col + p.len..])
            })
        });
        if first.len > 0 {
            self.col = self.col_of_byte(b.first, first.col + first.start_spaces);
        }
        self.clamp();
        changed
    }

    /// `d` `x` `X` (and `D`, `to_end`: to the end of every line).
    fn block_delete(&mut self, to_end: bool) -> EdEvent {
        if !self.block_register_ok() {
            return EdEvent::Moved;
        }
        let mut b = self.block();
        b.max |= to_end;
        let b = self.op_block(b);
        self.start_block_op(&b, true);
        if self.delete_block(&b, true) {
            return EdEvent::Changed { typed: None };
        }
        self.drop_empty_step();
        EdEvent::Moved
    }

    /// `c` `s` (and `C`, `to_end`): the block goes, and what Insert mode types then goes on
    /// each of its lines (those that reach its start).
    fn block_change(&mut self, to_end: bool) -> EdEvent {
        if !self.block_register_ok() {
            return EdEvent::Moved;
        }
        let mut b = self.block();
        b.max |= to_end;
        let b = self.op_block(b);
        self.start_block_op(&b, true);
        let first = prep(&self.lines[b.first], &b, Prep::Delete);
        let start = self.byte_at(b.first, self.col);
        self.delete_block(&b, true);
        let at = if first.len > 0 { first.col + first.start_spaces } else { start };
        let at = at.min(self.lines[b.first].len());
        self.begin_block_insert(b, false, true, at, None);
        EdEvent::Changed { typed: None }
    }

    /// `I` (before the block) and `A` (`append`: after it; on a line too short, after blanks
    /// up to its end; after `$`, at the end of each line): Insert mode on the first line, and
    /// what it types goes on the other lines when it ends. A count types it that many times.
    fn block_insert(&mut self, append: bool, n: usize) -> EdEvent {
        let b = self.op_block(self.block());
        self.start_block_op(&b, false);
        if (b.last - b.first + 1).saturating_mul(n) > UNDO_BYTES {
            // Too much text for an undo step: nothing happens, as for a put.
            self.clamp();
            return EdEvent::None;
        }
        let cursor = self.col;
        if !append {
            self.snapshot();
            let at = self.byte_at(b.first, cursor).min(self.lines[b.first].len());
            if n > 1 {
                self.ins_repeat = Some(InsertRepeat::new(n, false));
            }
            self.begin_block_insert(b, false, false, at, Some(cursor));
            return EdEvent::Changed { typed: None };
        }
        // `A` starts its undo step where the text goes (undo and redo come back there).
        let line = &self.lines[b.first];
        let p = prep(line, &b, Prep::Append);
        let mut at = (p.col + p.len).min(line.len());
        self.col = self.col_of_byte(b.first, at);
        self.snapshot();
        self.redo_to_before();
        if p.short && !b.max && p.end_spaces > 0 {
            // The first line is too short: blanks up to the block's end first.
            let off = self.line_start(b.first) + at;
            self.splice(off, off, &" ".repeat(p.end_spaces));
            at += p.end_spaces;
        }
        if n > 1 {
            self.ins_repeat = Some(InsertRepeat::new(n, false));
        }
        self.begin_block_insert(b, true, false, at, Some(cursor));
        EdEvent::Changed { typed: None }
    }

    /// Insert mode at byte `at` of the block's first line, for [`BlockInsert`].
    fn begin_block_insert(&mut self, block: Block, append: bool, change: bool, at: usize, cursor: Option<usize>) {
        let row = block.first;
        let len = self.lines[row].len();
        let col = self.col_of_byte(row, at);
        self.start_insert(false, (row, col));
        self.block_insert = Some(BlockInsert { block, append, change, row, at, len, cursor });
    }

    /// The Insert session of a block's `I`, `A` or `c` ended: the text it added on the first
    /// line goes on the block's other lines (nothing when the cursor left that line, or when
    /// it added nothing), as Vim does.
    pub(super) fn finish_block_insert(&mut self) {
        let Some(bi) = self.block_insert.take() else { return };
        let line = &self.lines[bi.row.min(self.lines.len() - 1)];
        if self.row != bi.row || line.len() <= bi.len {
            return;
        }
        let end = bi.at + line.len() - bi.len;
        if !line.is_char_boundary(bi.at) || !line.is_char_boundary(end) {
            return;
        }
        let text = line[bi.at..end].to_string();
        let b = bi.block;
        // Text too large to copy to every line within an undo step stays on the first one.
        if b.last > b.first && text.len().saturating_mul(b.last - b.first) <= UNDO_BYTES {
            self.block_work += self.span_bytes(&b);
            self.edit_block_lines(b.first + 1, b.last, false, |line| {
                if bi.change {
                    let p = prep(line, &b, Prep::Delete);
                    (!p.short).then(|| format!("{}{text}{}", &line[..p.col], &line[p.col..]))
                } else {
                    insert_line(line, &b, &text, bi.append)
                }
            });
        }
        if let Some(c) = bi.cursor {
            // `I` after the cursor moved left on the line: where the typing started there (Vim).
            let c = if bi.append || self.ins_start.0 != bi.row { c } else { c.min(self.ins_start.1) };
            self.set_pos(bi.row, c);
        }
    }

    /// `r{ch}` on the block: every character in it becomes `ch` (a line break splits each of
    /// its lines there); the cursor on its top left corner.
    pub(super) fn block_replace(&mut self, ch: char) -> EdEvent {
        let b = self.op_block(self.block());
        self.start_block_op(&b, false);
        self.block_work += self.span_bytes(&b);
        let changed = self.edit_block_lines(b.first, b.last, true, |line| replace_line(line, &b, ch));
        let c = self.col;
        self.set_pos(b.first, c);
        if changed { EdEvent::Changed { typed: None } } else { EdEvent::Moved }
    }

    /// `u` `U` `~` (and `gu` `gU` `g~`) on the block; the cursor on its top left corner.
    pub(super) fn block_case(&mut self, c: char) -> EdEvent {
        let case = match c {
            'u' => Case::Lower,
            'U' => Case::Upper,
            _ => Case::Toggle,
        };
        let b = self.op_block(self.block());
        self.start_block_op(&b, false);
        self.block_work += self.span_bytes(&b);
        let changed = self.edit_block_lines(b.first, b.last, true, |line| {
            let p = prep(line, &b, Prep::Case);
            let old = &line[p.col..p.col + p.len];
            Some(format!("{}{}{}", &line[..p.col], recase(old, case), &line[p.col + p.len..]))
        });
        if changed {
            // Vim's case operators put `'.` at the line's start.
            self.marks.set_change((b.first, 0));
        }
        self.clamp();
        if changed { EdEvent::Changed { typed: None } } else { EdEvent::Moved }
    }

    /// `>` / `<` on the block, `n` times: blanks go in at the block's start, or the blanks
    /// there go (Vim's block shift, with spaces); a line that ends before the block stays.
    fn block_shift(&mut self, right: bool, n: usize) -> EdEvent {
        let b = self.op_block(self.block());
        self.start_block_op(&b, false);
        let at = self.byte_at(b.first, self.col);
        let amount = super::TAB_WIDTH.saturating_mul(n);
        if (b.last - b.first + 1).saturating_mul(amount) > UNDO_BYTES {
            // Too many blanks for an undo step: nothing happens, as for a put.
            self.clamp();
            return EdEvent::None;
        }
        self.block_work += self.span_bytes(&b);
        let changed = self.edit_block_lines(b.first, b.last, true, |line| shift_line(line, &b, right, amount));
        self.col = self.col_of_byte(b.first, at);
        self.clamp();
        if changed { EdEvent::Changed { typed: None } } else { EdEvent::Moved }
    }

    /// `p` / `P` on the block: the block goes (into the registers as a delete with `p`), then
    /// the register is put there: a block from where it started, lines below (`p`: below the
    /// cursor's line) or above it, text of one line on each of its lines, other text where it
    /// started. `.` repeats only the delete, as Neovim does.
    fn block_put(&mut self, keep: bool, n: usize) -> EdEvent {
        let name = self.reg.unwrap_or('"');
        if matches!(name, '+' | '*') {
            self.rec.put_clipboard();
        }
        let src = if self.rec.replaying() { None } else { self.put_source(name) };
        if src.as_ref().is_some_and(|r| self.put_size(r, n) > UNDO_BYTES) {
            return EdEvent::None;
        }
        let cursor_row = self.row;
        let b = self.op_block(self.block());
        self.start_block_op(&b, true);
        let start_col = self.col;
        self.reg = None;
        self.delete_block(&b, !keep);
        // Past the end of a line the block left shorter, the text goes after its last character.
        let after = self.col < start_col;
        match src {
            Some(r) if r.kind == RegKind::Blockwise => self.put_block(&r.text, after, n),
            Some(r) if r.kind == RegKind::Linewise => {
                let text = vec![r.text.as_str(); n].join("\n");
                let below = if keep { after } else { true };
                let at = if keep { b.first } else { cursor_row.min(self.lines.len() - 1) };
                let (start, end) = self.line_bounds(at);
                let row = if below {
                    self.splice(end, end, &format!("\n{text}"));
                    at + 1
                } else {
                    self.mark_hint = Some(Hint::Above);
                    self.splice(start, start, &format!("{text}\n"));
                    at
                };
                self.set_pos(row, self.first_nonblank(row));
            }
            Some(r) if !r.text.is_empty() && !r.text.contains('\n') => self.put_on_block_lines(&b, &r.text, after, n),
            Some(r) if !r.text.is_empty() => {
                let text = r.text.repeat(n);
                let mut off = self.offset();
                if after && self.gcount(self.row) > 0 {
                    off = self.offset_of(self.row, self.col + 1);
                }
                self.splice(off, off, &text);
                self.set_cursor_offset(off);
                self.want_x = None;
                self.clamp();
            }
            _ => {}
        }
        EdEvent::Changed { typed: None }
    }

    /// Text of one line `n` times on each line of block `b`, at the screen column where the
    /// cursor puts it on the first line (after the cursor's character with `after`); a line
    /// that ends before that column gets nothing. The cursor on the last character put.
    fn put_on_block_lines(&mut self, b: &Block, text: &str, after: bool, n: usize) {
        let text = text.repeat(n);
        let (row, mut col) = (self.row, self.col);
        if after && self.gcount(row) > 0 {
            col += 1;
        }
        let x = self.display_x(row, col);
        self.block_work += self.span_bytes(b);
        self.edit_block_lines(b.first, b.last, false, |line| {
            let mut acc = 0;
            let mut at = line.len();
            for (i, g) in line.grapheme_indices(true) {
                if acc >= x || acc + gw(g) > x {
                    at = i;
                    break;
                }
                acc += gw(g);
            }
            (at < line.len() || acc == x).then(|| format!("{}{text}{}", &line[..at], &line[at..]))
        });
        let last = text.graphemes(true).count().saturating_sub(1);
        self.set_pos(row, col + last);
    }

    /// A paste from the terminal over the block: the block goes, the text goes in where it
    /// started, as one undo step.
    pub(super) fn paste_over_block(&mut self, text: &str) -> EdEvent {
        let b = self.filled(self.block());
        self.start_block_op(&b, true);
        self.delete_block(&b, false);
        let off = self.offset();
        self.splice(off, off, text);
        self.set_cursor_offset(off + text.len());
        self.want_x = None;
        self.clamp();
        EdEvent::Changed { typed: None }
    }
}

#[cfg(test)]
mod tests;
