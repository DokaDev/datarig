//! A table tab's query bar: one line at the top of its results pane that says, in the order of
//! the statement it sends, what the rows shown were read with:
//! `WHERE [status = 'paid' ×] + │ ORDER BY ↓ total_amount + │ LIMIT 500 │ 7 cols │ 1–500 of
//! ~50,001 rows ‹ › … ⋯ Advanced`. A run on its way, a run that brought no rows or a refusal
//! goes first, before `WHERE`.
//!
//! [`layout`] is pure: from a [`Bar`] (every text already in the user's language) and a width it
//! picks the richest form that fits, narrowing in steps ([`LEVELS`]): `⋯ Advanced` becomes `⋯`,
//! the rows are said shorter, the order keys and the conditions after the first are counted
//! (`+1`), `ORDER BY` becomes `ORDER`, the first condition is cut, the conditions are counted
//! (`2 conditions`), the rows keep only their arrows, the column count leaves (`6/7` stays
//! while columns are hidden), the order is counted (`ORDER ↓2`), the rows go, and then a short form is left (`failed │ ▼2 + │ ↓2 │
//! L500 │ 6/7 │ ‹ › ⋯`) that narrows in its own steps down to marks (`! ▼2 ↓2 L ◌1 ⋯`).
//! Nothing applied ever leaves: a condition, an order, a limit, hidden columns and the state
//! keep a mark in every form. [`draw`] draws the form picked and returns where each clickable
//! part is.

use crate::app::App;
use crate::app::hover::PointerOn;
use crate::text::{clip, width};
use crate::theme;
use datarig_core::query::Dir;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// The fewest columns of its text the first condition is cut to.
pub(crate) const CUT_MIN: usize = 8;

/// A part of the bar a click (or the bar's keys) acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarHit {
    /// The `WHERE` keyword.
    Where,
    /// A condition, by its term's place among the terms of the condition text.
    Chip(usize),
    /// A condition's `×`.
    Remove(usize),
    /// The conditions left out (`+2`), or all of them counted (`2 conditions`, `▼2`).
    More,
    /// The `+` that adds a condition.
    Add,
    /// The `ORDER BY` part.
    Order,
    /// The `LIMIT` part.
    Limit,
    /// The column count.
    Cols,
    /// The rows.
    Rows,
    /// The previous and the next page.
    Prev,
    Next,
    /// `⋯ Advanced`.
    Advanced,
}

impl BarHit {
    /// A part of the `WHERE` segment the bar's focus moves among.
    pub fn in_where(self) -> bool {
        matches!(self, BarHit::Chip(_) | BarHit::More | BarHit::Add)
    }
}

/// A condition of the bar: the text sent for its term.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChipM {
    pub term: usize,
    pub text: String,
    /// A term the app does not read as one column's condition (drawn in the warning colour).
    pub expr: bool,
}

/// The state of the tab's query, when there is one to say: the spinner of its run on its way,
/// the word for a run that brought no rows or a refusal, and the mark that stands for that word
/// where it does not fit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateM {
    pub spinner: Option<String>,
    pub word: Option<String>,
    pub mark: String,
}

/// The rows of the result shown: from the longest form (`1–500 of ~50,001 rows`) to the
/// shortest (`~50k rows`), a short mark of its paging state (`warn`: in the warning colour), and
/// the previous and next page arrows with whether each can be used (none: one page only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowsM {
    pub full: String,
    pub mid: String,
    pub short: String,
    pub mark: Option<(String, bool)>,
    pub arrows: Option<(bool, bool)>,
}

/// The column count: `7 cols`, or `6/7 cols` with `short` (`6/7`) while some are hidden.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColsM {
    pub full: String,
    pub short: String,
    pub hidden: usize,
}

/// The `LIMIT` part: none (its word), a cap, or the field it is typed in (its cursor in
/// columns from the start of the text; `selected`: its text as it opened, which the first key
/// typed replaces).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LimitM {
    None(String),
    Cap(u64),
    Editing { text: String, cursor: usize, selected: bool },
}

/// The marks the bar draws, by the icons setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyphs {
    pub filter: &'static str,
    pub prev: &'static str,
    pub next: &'static str,
}

/// What the bar says (texts in the user's language).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bar {
    pub state: Option<StateM>,
    pub chips: Vec<ChipM>,
    /// The conditions counted (`2 conditions`).
    pub conditions: String,
    pub order: Vec<(String, Dir)>,
    pub limit: LimitM,
    pub cols: Option<ColsM>,
    pub rows: Option<RowsM>,
    /// `Advanced`.
    pub advanced: String,
    /// The part of the `WHERE` segment the bar's focus is on.
    pub focus: Option<BarHit>,
    pub glyphs: Glyphs,
    /// The theme paints backgrounds: chips are drawn on one, else in brackets.
    pub backgrounds: bool,
}

/// What a piece of the bar is, for its style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// A clause's keyword (`WHERE`), and the same keyword of a clause that is empty.
    Keyword,
    KeywordDim,
    /// A condition, an expression's condition, its `×`, and the brackets around it where the
    /// theme paints no backgrounds.
    Chip,
    ChipExpr,
    ChipX,
    Bracket,
    /// `+` (add), an order key's arrow, a count (`+1`), a page arrow that works, `⋯`.
    Accent,
    /// A column name, a cap.
    Text,
    /// Words that say little (`none`, `7 cols`, the rows).
    Muted,
    /// Something to look at: hidden columns, a run that brought no rows.
    Warn,
    /// Between two segments.
    Sep,
    Spinner,
    /// A page arrow that does not work now.
    Dim,
    /// The `LIMIT` field being typed in, and its text while it is selected (the first key
    /// typed replaces it).
    Field,
    FieldSelected,
}

/// A piece of the bar: its text, its style, what a click on it does, the character of the
/// text that is underlined (the key of the segment), and whether the bar's focus is on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seg {
    pub text: String,
    pub tone: Tone,
    pub hit: Option<BarHit>,
    pub ul: Option<usize>,
    pub focus: bool,
}

/// The bar laid out: the pieces from the left, and those at the right end.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Line {
    pub left: Vec<Seg>,
    pub right: Vec<Seg>,
    /// The step of [`LEVELS`] that made it.
    pub level: usize,
}

impl Line {
    /// The columns it takes (one blank at least between the left and the right).
    pub fn width(&self) -> usize {
        let w = |s: &[Seg]| s.iter().map(|s| width(&s.text)).sum::<usize>();
        let right = w(&self.right);
        w(&self.left) + if right > 0 { right + 1 } else { 0 }
    }

    /// Every piece, left then right.
    pub fn segs(&self) -> impl Iterator<Item = &Seg> {
        self.left.iter().chain(&self.right)
    }

    /// Each piece with the column it starts at, in a bar `w` columns wide from column `x0`.
    pub fn placed(&self, x0: u16, w: u16) -> Vec<(u16, &Seg)> {
        let mut out = Vec::new();
        let mut x = x0;
        for s in &self.left {
            out.push((x, s));
            x = x.saturating_add(width(&s.text) as u16);
        }
        let right: usize = self.right.iter().map(|s| width(&s.text)).sum();
        let mut x = (x0 + w).saturating_sub(right as u16).max(x + u16::from(right > 0));
        for s in &self.right {
            out.push((x, s));
            x = x.saturating_add(width(&s.text) as u16);
        }
        out
    }
}

/// One step of narrowing; each step keeps what the ones before it did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lv {
    /// `⋯ Advanced` → `⋯`.
    adv_short: bool,
    /// The rows: 0 whole, 1 without the range, 2 the number shortened, 3 the arrows only, 4
    /// nothing.
    rows: u8,
    /// The order keys after the first counted (`+1`).
    order_more: bool,
    /// The conditions after the first counted (`+1`).
    chips_more: bool,
    /// `ORDER BY` → `ORDER`.
    order_word: bool,
    /// The first condition cut with `…`.
    cut: bool,
    /// The conditions counted (`2 conditions`).
    chips_count: bool,
    /// The column count of hidden columns said as numbers only (`6/7`); with none hidden it goes
    /// from step 2 on.
    cols_short: bool,
    /// The order counted (`ORDER ↓2`).
    order_count: bool,
    /// The short form (`failed │ ▼2 + │ ↓2 │ L500 │ 6/7 │ ‹ › ⋯`) and how far it is narrowed:
    /// 0 not used; 1 whole; 2 single blanks between the parts; 3 no page arrows; 4 no `+`; 5
    /// hidden columns as `◌1`; 6 the limit as `L` alone; 7 the state as its mark.
    short: u8,
    /// The state and `⋯` alone.
    bare: bool,
}

const fn lv(n: usize) -> Lv {
    Lv {
        adv_short: n >= 1,
        rows: if n >= 12 {
            4
        } else if n >= 9 {
            3
        } else if n >= 3 {
            2
        } else if n >= 2 {
            1
        } else {
            0
        },
        order_more: n >= 4,
        chips_more: n >= 5,
        order_word: n >= 6,
        cut: n >= 7,
        chips_count: n >= 8,
        cols_short: n >= 10,
        order_count: n >= 11,
        short: if n >= 13 && n <= 19 {
            (n - 12) as u8
        } else if n > 19 {
            7
        } else {
            0
        },
        bare: n >= 20,
    }
}

/// The steps, from the whole bar to the narrowest.
pub const LEVELS: [Lv; 21] = [
    lv(0),
    lv(1),
    lv(2),
    lv(3),
    lv(4),
    lv(5),
    lv(6),
    lv(7),
    lv(8),
    lv(9),
    lv(10),
    lv(11),
    lv(12),
    lv(13),
    lv(14),
    lv(15),
    lv(16),
    lv(17),
    lv(18),
    lv(19),
    lv(20),
];

/// A count in at most four characters (`99+` above 99).
fn capped(n: usize) -> String {
    if n > 99 { "99+".to_string() } else { n.to_string() }
}

/// A number in few characters, never more than it is: `500`, `50k`, `1.2M`, `9.2E`.
pub fn compact(n: u64) -> String {
    const UNITS: [&str; 6] = ["k", "M", "G", "T", "P", "E"];
    if n < 1000 {
        return n.to_string();
    }
    let mut scale = 1000u64;
    let mut unit = 0;
    while unit + 1 < UNITS.len() && n / scale >= 1000 {
        scale *= 1000;
        unit += 1;
    }
    let whole = n / scale;
    if whole < 10 {
        let tenth = (n % scale) / (scale / 10);
        if tenth > 0 {
            return format!("{whole}.{tenth}{}", UNITS[unit]);
        }
    }
    format!("{whole}{}", UNITS[unit])
}

/// The character of `text` that is the key `key` (the first one, either case), when it has one.
fn mnemonic(text: &str, key: char) -> Option<usize> {
    text.chars().position(|c| c == key).or_else(|| text.chars().position(|c| c.eq_ignore_ascii_case(&key)))
}

fn seg(text: impl Into<String>, tone: Tone, hit: Option<BarHit>) -> Seg {
    Seg { text: text.into(), tone, hit, ul: None, focus: false }
}

fn keyword(text: &str, key: char, dim: bool, hit: BarHit) -> Seg {
    let tone = if dim { Tone::KeywordDim } else { Tone::Keyword };
    Seg { ul: mnemonic(text, key), ..seg(text, tone, Some(hit)) }
}

/// The arrow of an order key's direction.
fn arrow(d: Dir) -> &'static str {
    if d == Dir::Desc { "\u{2193}" } else { "\u{2191}" }
}

/// A condition: its text and `×` on the chip's background, else in brackets.
fn chip(out: &mut Vec<Seg>, text: &str, expr: bool, term: usize, bg: bool) {
    let tone = if expr { Tone::ChipExpr } else { Tone::Chip };
    let (body, x) = (Some(BarHit::Chip(term)), Some(BarHit::Remove(term)));
    if bg {
        out.push(seg(format!(" {text} "), tone, body));
        out.push(seg("\u{00d7}", Tone::ChipX, x));
        out.push(seg(" ", tone, x));
    } else {
        out.push(seg("[", Tone::Bracket, body));
        out.push(seg(format!("{text} "), tone, body));
        out.push(seg("\u{00d7}", Tone::ChipX, x));
        out.push(seg("]", Tone::Bracket, x));
    }
}

/// A token drawn as a chip without `×` (`+2`, `2 conditions`).
fn token(out: &mut Vec<Seg>, text: &str, hit: BarHit, bg: bool) {
    if bg {
        out.push(seg(format!(" {text} "), Tone::Chip, Some(hit)));
    } else {
        out.push(seg("[", Tone::Bracket, Some(hit)));
        out.push(seg(text, Tone::Chip, Some(hit)));
        out.push(seg("]", Tone::Bracket, Some(hit)));
    }
}

const SEP: &str = " \u{2502} ";

/// The short form at its step `k` ([`Lv::short`]) after the state already in `out`: a mark for
/// each part applied (`▼2 +`, `↓2`, `L500`, `6/7`) and the page arrows.
fn short_form(m: &Bar, k: u8, out: &mut Vec<Seg>) {
    let n = m.chips.len();
    let mut parts: Vec<Vec<Seg>> = Vec::new();
    if n > 0 {
        let mut p = vec![seg(format!("{}{}", m.glyphs.filter, capped(n)), Tone::Accent, Some(BarHit::More))];
        if k < 4 {
            p.push(seg(" ", Tone::Muted, None));
            p.push(seg("+", Tone::Accent, Some(BarHit::Add)));
        }
        parts.push(p);
    }
    if let Some((_, d)) = m.order.first() {
        let text = format!("{}{}", arrow(*d), capped(m.order.len()));
        parts.push(vec![seg(text, Tone::Accent, Some(BarHit::Order))]);
    }
    let l = Some(BarHit::Limit);
    match &m.limit {
        LimitM::None(_) => {}
        LimitM::Cap(c) if k < 6 => parts.push(vec![seg(format!("L{}", compact(*c)), Tone::Text, l)]),
        LimitM::Cap(_) => parts.push(vec![seg("L", Tone::Text, l)]),
        LimitM::Editing { text, selected, .. } => {
            let tone = if *selected { Tone::FieldSelected } else { Tone::Field };
            parts.push(vec![seg("L ", Tone::Text, l), seg(format!("{text} "), tone, l)])
        }
    }
    if let Some(c) = m.cols.as_ref().filter(|c| c.hidden > 0) {
        let text = if k < 5 { c.short.clone() } else { format!("\u{25cc}{}", capped(c.hidden)) };
        parts.push(vec![seg(text, Tone::Warn, Some(BarHit::Cols))]);
    }
    if let Some((prev, next)) = m.rows.as_ref().and_then(|r| r.arrows).filter(|_| k < 3) {
        let tone = |on: bool| if on { Tone::Accent } else { Tone::Dim };
        parts.push(vec![
            seg(m.glyphs.prev, tone(prev), Some(BarHit::Prev)),
            seg(" ", Tone::Muted, None),
            seg(m.glyphs.next, tone(next), Some(BarHit::Next)),
        ]);
    }
    let sep = if k < 2 { SEP } else { " " };
    // After the state: the same separator as between the parts.
    if m.state.is_some() && !parts.is_empty() && k < 2 {
        out.pop();
        out.push(seg(SEP, Tone::Sep, None));
    }
    for (i, p) in parts.into_iter().enumerate() {
        if i > 0 {
            out.push(seg(sep, if k < 2 { Tone::Sep } else { Tone::Muted }, None));
        }
        out.extend(p);
    }
}

/// The bar at step `lv`, the first condition cut to `cut` columns when given.
fn line(m: &Bar, lv: Lv, cut: Option<usize>) -> Vec<Seg> {
    let mut out = Vec::new();
    if let Some(s) = &m.state {
        let mut parts: Vec<Seg> = Vec::new();
        if let Some(sp) = &s.spinner {
            parts.push(seg(sp.clone(), Tone::Spinner, None));
        }
        match (&s.word, lv.short >= 7) {
            (Some(w), false) => parts.push(seg(w.clone(), Tone::Warn, None)),
            (Some(_), true) => parts.push(seg(s.mark.clone(), Tone::Warn, None)),
            (None, _) => {}
        }
        for (i, p) in parts.into_iter().enumerate() {
            if i > 0 {
                out.push(seg(" ", Tone::Muted, None));
            }
            out.push(p);
        }
        out.push(seg(" ", Tone::Muted, None));
    }
    if lv.bare {
        return out;
    }
    let n = m.chips.len();
    if lv.short > 0 {
        short_form(m, lv.short, &mut out);
        return out;
    }
    // WHERE
    out.push(keyword("WHERE", 'W', n == 0, BarHit::Where));
    if lv.chips_count && n > 0 {
        out.push(seg(" ", Tone::Muted, None));
        token(&mut out, &m.conditions, BarHit::More, m.backgrounds);
    } else {
        let shown = if lv.chips_more { n.min(1) } else { n };
        for (i, c) in m.chips.iter().take(shown).enumerate() {
            out.push(seg(" ", Tone::Muted, None));
            let text = match cut {
                Some(keep) if i == 0 => clip(&c.text, keep),
                _ => c.text.clone(),
            };
            chip(&mut out, &text, c.expr, c.term, m.backgrounds);
        }
        if shown < n {
            out.push(seg(" ", Tone::Muted, None));
            token(&mut out, &format!("+{}", capped(n - shown)), BarHit::More, m.backgrounds);
        }
    }
    out.push(seg(" ", Tone::Muted, None));
    out.push(seg("+", Tone::Accent, Some(BarHit::Add)));
    // ORDER BY
    out.push(seg(SEP, Tone::Sep, None));
    let word = if lv.order_word { "ORDER" } else { "ORDER BY" };
    out.push(keyword(word, 'O', m.order.is_empty(), BarHit::Order));
    let o = Some(BarHit::Order);
    match m.order.first() {
        None => {
            out.push(seg(" ", Tone::Muted, o));
            out.push(seg("+", Tone::Accent, o));
        }
        Some((_, d)) if lv.order_count => {
            out.push(seg(" ", Tone::Muted, o));
            out.push(seg(arrow(*d), Tone::Accent, o));
            out.push(seg(m.order.len().to_string(), Tone::Accent, o));
        }
        Some(_) => {
            let shown = if lv.order_more { 1 } else { m.order.len() };
            for (i, (col, d)) in m.order.iter().take(shown).enumerate() {
                out.push(seg(if i == 0 { " " } else { ", " }, Tone::Muted, o));
                out.push(seg(arrow(*d), Tone::Accent, o));
                out.push(seg(format!(" {col}"), Tone::Text, o));
            }
            if shown < m.order.len() {
                out.push(seg(format!(" +{}", capped(m.order.len() - shown)), Tone::Accent, o));
            } else {
                out.push(seg(" ", Tone::Muted, o));
                out.push(seg("+", Tone::Accent, o));
            }
        }
    }
    // LIMIT
    out.push(seg(SEP, Tone::Sep, None));
    let l = Some(BarHit::Limit);
    out.push(keyword("LIMIT", 'L', matches!(m.limit, LimitM::None(_)), BarHit::Limit));
    out.push(seg(" ", Tone::Muted, l));
    match &m.limit {
        LimitM::None(word) => out.push(seg(word.clone(), Tone::Muted, l)),
        LimitM::Cap(n) => out.push(seg(n.to_string(), Tone::Text, l)),
        LimitM::Editing { text, selected, .. } => {
            let tone = if *selected { Tone::FieldSelected } else { Tone::Field };
            out.push(seg(format!("{text} "), tone, l))
        }
    }
    // cols
    if let Some(c) = &m.cols {
        // Nothing hidden: the count says little and is the first to go; hidden columns keep
        // their word longer, then their numbers.
        let text = match (c.hidden > 0, lv.cols_short) {
            (false, _) if lv.rows >= 1 => None,
            (_, false) => Some(&c.full),
            (true, true) => Some(&c.short),
            (false, true) => None,
        };
        if let Some(t) = text {
            out.push(seg(SEP, Tone::Sep, None));
            let tone = if c.hidden > 0 { Tone::Warn } else { Tone::Muted };
            out.push(Seg { ul: mnemonic(t, 'c'), ..seg(t.clone(), tone, Some(BarHit::Cols)) });
        }
    }
    // rows
    if let Some(r) = m.rows.as_ref().filter(|_| lv.rows < 4) {
        out.push(seg(SEP, Tone::Sep, None));
        let text = match lv.rows {
            0 => Some(&r.full),
            1 => Some(&r.mid),
            2 => Some(&r.short),
            _ => None,
        };
        let mut first = true;
        if let Some(t) = text {
            out.push(Seg { ul: mnemonic(t, 'r'), ..seg(t.clone(), Tone::Muted, Some(BarHit::Rows)) });
            first = false;
        }
        if let Some((mark, warn)) = &r.mark {
            let sep = if first { "" } else { " \u{00b7} " };
            let tone = if *warn { Tone::Warn } else { Tone::Muted };
            out.push(seg(format!("{sep}{mark}"), tone, Some(BarHit::Rows)));
            first = false;
        }
        if let Some((prev, next)) = r.arrows {
            let tone = |on: bool| if on { Tone::Accent } else { Tone::Dim };
            if !first {
                out.push(seg(" ", Tone::Muted, None));
            }
            out.push(seg(m.glyphs.prev, tone(prev), Some(BarHit::Prev)));
            out.push(seg(" ", Tone::Muted, None));
            out.push(seg(m.glyphs.next, tone(next), Some(BarHit::Next)));
        } else if first {
            // Nothing of the rows is left: no separator for it.
            out.pop();
        }
    }
    out
}

/// The `⋯ Advanced` end of the bar at step `lv`.
fn right(m: &Bar, lv: Lv) -> Vec<Seg> {
    let a = Some(BarHit::Advanced);
    if lv.adv_short || lv.short > 0 {
        return vec![seg("\u{22ef}", Tone::Accent, a)];
    }
    vec![
        seg("\u{22ef} ", Tone::Accent, a),
        Seg { ul: mnemonic(&m.advanced, 'A'), ..seg(m.advanced.clone(), Tone::Muted, a) },
    ]
}

/// The bar at step `level` of [`LEVELS`] when it fits in `w` columns (the first condition cut
/// where that step cuts it), else `None`.
pub fn at_level(m: &Bar, level: usize, w: usize) -> Option<Line> {
    let lv = LEVELS[level];
    let make = |cut: Option<usize>| Line { left: line(m, lv, cut), right: right(m, lv), level };
    let l = make(None);
    if l.width() <= w {
        return Some(l);
    }
    // The first condition cut to what is left, keeping enough of it to read.
    let first = m.chips.first().filter(|_| lv.cut && !lv.chips_count && lv.short == 0)?;
    let keep = width(&first.text).saturating_sub(l.width() - w);
    (keep >= CUT_MIN).then(|| make(Some(keep))).filter(|l| l.width() <= w)
}

/// The bar in `w` columns: the first step of [`LEVELS`] whose form fits (the last when none
/// does), the bar's focus marked on what it is on.
pub fn layout(m: &Bar, w: usize) -> Line {
    let last = LEVELS.len() - 1;
    let mut out = (0..LEVELS.len()).find_map(|level| at_level(m, level, w)).unwrap_or_else(|| Line {
        left: line(m, LEVELS[last], None),
        right: right(m, LEVELS[last]),
        level: last,
    });
    if let Some(f) = m.focus {
        mark_focus(&mut out, f);
    }
    out
}

/// The focus on `f`, else (a condition left out) on what counts it, else on `+`.
fn mark_focus(l: &mut Line, f: BarHit) {
    let has = |l: &Line, h: BarHit| l.segs().any(|s| s.hit == Some(h));
    let on = match f {
        BarHit::Chip(_) if !has(l, f) => BarHit::More,
        f => f,
    };
    let on = if has(l, on) { on } else { BarHit::Add };
    let remove = match on {
        BarHit::Chip(t) => Some(BarHit::Remove(t)),
        _ => None,
    };
    for s in l.left.iter_mut().chain(l.right.iter_mut()) {
        if s.hit == Some(on) || (s.hit.is_some() && s.hit == remove) {
            s.focus = true;
        }
    }
}

/// The style of a piece.
fn style_of(tone: Tone, bg: bool) -> Style {
    let th = theme::cur();
    let chip = |fg| if bg { Style::new().fg(fg).bg(th.surface_alt) } else { Style::new().fg(fg) };
    match tone {
        Tone::Keyword => Style::new().fg(th.fg_muted).add_modifier(Modifier::BOLD),
        Tone::KeywordDim => Style::new().fg(th.fg_dim).add_modifier(Modifier::BOLD),
        Tone::Chip => chip(th.fg),
        Tone::ChipExpr => chip(th.warning),
        Tone::ChipX => chip(th.fg_muted),
        Tone::Bracket => Style::new().fg(th.fg_muted),
        Tone::Accent | Tone::Spinner => Style::new().fg(th.accent),
        Tone::Text => Style::new().fg(th.fg),
        Tone::Muted => Style::new().fg(th.fg_muted),
        Tone::Warn => Style::new().fg(th.warning).add_modifier(Modifier::BOLD),
        Tone::Sep => Style::new().fg(th.border),
        Tone::Dim => Style::new().fg(th.fg_dim),
        Tone::Field => Style::new().fg(th.fg).bg(th.surface_alt).add_modifier(Modifier::UNDERLINED),
        Tone::FieldSelected => Style::new().fg(th.fg).patch(th.selection).add_modifier(Modifier::UNDERLINED),
    }
}

/// Where each clickable part of the bar was drawn.
pub(crate) type Hits = Vec<(Rect, BarHit)>;

/// Draw bar `m` on the line `area` and return where each clickable part is, and where the
/// cursor of the `LIMIT` field is while it is typed in.
pub(crate) fn draw(app: &App, m: &Bar, area: Rect, buf: &mut Buffer) -> (Hits, Option<(u16, u16)>) {
    let th = theme::cur();
    buf.set_style(area, Style::new().bg(th.bg));
    let l = layout(m, usize::from(area.width));
    let hovered = match app.pointer_hover() {
        Some(PointerOn::Bar(h)) => Some(h),
        _ => None,
    };
    let end = area.x + area.width;
    let mut hits: Hits = Vec::new();
    let mut cursor = None;
    for (x, s) in l.placed(area.x, area.width) {
        if x >= end {
            break;
        }
        let room = usize::from(end - x);
        let mut style = Style::new().bg(th.bg).patch(style_of(s.tone, m.backgrounds));
        if s.focus {
            // The selection, in bold and underlined: it reads on a chip's own background too.
            style = style.patch(th.selection).add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
        }
        if s.hit.is_some() && s.hit == hovered {
            style = style.patch(crate::widgets::pointer_style());
        }
        let w = crate::widgets::put(buf, x, area.y, &s.text, room, style);
        if let Some(i) = s.ul {
            // The key's letter underlined: the cell it is drawn in.
            let before: String = s.text.chars().take(i).collect();
            let at = x + width(&before) as u16;
            if at < x + w {
                let cell = Rect { x: at, y: area.y, width: 1, height: 1 };
                buf.set_style(cell, Style::new().add_modifier(Modifier::UNDERLINED));
            }
        }
        if let Some(h) = s.hit {
            let r = Rect { x, y: area.y, width: w, height: 1 };
            match hits.last_mut() {
                Some((last, lh)) if *lh == h && last.x + last.width == x => last.width += w,
                _ => hits.push((r, h)),
            }
        }
        if matches!(s.tone, Tone::Field | Tone::FieldSelected)
            && let LimitM::Editing { cursor: c, .. } = &m.limit
        {
            cursor = Some(((x + *c as u16).min(end.saturating_sub(1)), area.y));
        }
    }
    (hits, cursor)
}

#[cfg(test)]
mod tests;
