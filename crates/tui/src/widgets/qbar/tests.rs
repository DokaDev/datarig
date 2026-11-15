use super::*;
use datarig_i18n::{I18n, Label, Lang, Msg};

/// The texts of the bar in `lang`, as the app builds them.
struct Words {
    i: I18n,
}

impl Words {
    fn new(lang: Lang) -> Self {
        Self { i: I18n::new(lang) }
    }

    fn label(&self, l: Label) -> String {
        self.i.label(l).to_string()
    }

    fn msg(&self, m: &Msg) -> String {
        self.i.msg(m).to_string()
    }
}

/// What a bar has applied, to build it from.
#[derive(Clone, Default)]
struct Spec {
    chips: Vec<(&'static str, bool)>,
    order: Vec<(&'static str, Dir)>,
    limit: Option<u64>,
    hidden: usize,
    total_cols: usize,
    running: bool,
    failed: bool,
    icons: bool,
    backgrounds: bool,
    rows: Option<u64>,
}

fn bar(w: &Words, s: &Spec) -> Bar {
    let chips: Vec<ChipM> =
        s.chips.iter().enumerate().map(|(i, (t, e))| ChipM { term: i, text: t.to_string(), expr: *e }).collect();
    let total = s.total_cols.max(1);
    let shown = total - s.hidden.min(total);
    let full = if s.hidden > 0 {
        w.msg(&Msg::TableBarColsHidden { shown: shown.to_string(), total: total.to_string() })
    } else {
        w.msg(&Msg::TableBarCols { count: total as u64 })
    };
    let state = (s.running || s.failed).then(|| StateM {
        spinner: s.running.then(|| "\u{280b}".to_string()),
        word: s.failed.then(|| w.label(Label::TableStatusFailed)),
        mark: "!".to_string(),
    });
    let rows = s.rows.map(|n| {
        let long = format!("~{}", datarig_i18n::fmt_count(n));
        RowsM {
            full: w.msg(&Msg::TableBarRowsRange { from: "1".into(), to: "500".into(), total: long.clone() }),
            mid: w.msg(&Msg::TableBarRowsText { rows: long }),
            short: w.msg(&Msg::TableBarRowsText { rows: format!("~{}", compact(n)) }),
            state: Some((w.label(Label::ResultsTitleInTx), false)),
            notes: vec![w.label(Label::TableBarPlanner)],
            arrows: Some((false, true)),
        }
    });
    let (prev, next) = crate::icons::page_arrows(s.icons);
    Bar {
        state,
        conditions: w.msg(&Msg::TableBarConditions { count: chips.len() as u64 }),
        chips,
        order: s.order.iter().map(|(c, d)| (c.to_string(), *d)).collect(),
        limit: match s.limit {
            Some(n) => LimitM::Cap(n),
            None => LimitM::None(w.label(Label::TableBarNone)),
        },
        cols: Some(ColsM { full, short: format!("{shown}/{total}"), hidden: s.hidden }),
        rows,
        advanced: w.label(Label::TableBarAdvanced),
        focus: None,
        glyphs: Glyphs { filter: crate::icons::filter_status(s.icons), prev, next },
        backgrounds: s.backgrounds,
    }
}

fn text(l: &Line) -> String {
    let left: String = l.left.iter().map(|s| s.text.as_str()).collect();
    let right: String = l.right.iter().map(|s| s.text.as_str()).collect();
    format!("{left} | {right}")
}

fn has(l: &Line, h: BarHit) -> bool {
    l.segs().any(|s| s.hit == Some(h))
}

/// Every state the bar is walked through.
fn specs() -> Vec<Spec> {
    let base = Spec { total_cols: 7, rows: Some(50_001), backgrounds: true, ..Spec::default() };
    let applied = Spec {
        chips: vec![("status = 'paid'", false), ("total_amount > 1000", false)],
        order: vec![("total_amount", Dir::Desc), ("ordered_at", Dir::Asc)],
        limit: Some(500),
        ..base.clone()
    };
    let many: Vec<(&'static str, bool)> =
        (0..120).map(|_| ("a_rather_long_column_name = 'some value'", false)).collect();
    let keys: Vec<(&'static str, Dir)> = (0..105).map(|_| ("ordered_at", Dir::Asc)).collect();
    vec![
        base.clone(),
        applied.clone(),
        Spec { chips: vec![("lower(email) LIKE '%@example.com'", true)], ..base.clone() },
        Spec { failed: true, ..applied.clone() },
        Spec { running: true, ..applied.clone() },
        Spec { running: true, failed: true, ..applied.clone() },
        Spec { hidden: 1, ..applied.clone() },
        Spec { hidden: 150, total_cols: 300, ..applied.clone() },
        Spec { limit: Some(9_223_372_036_854_775_807), ..applied.clone() },
        Spec {
            chips: many,
            order: keys,
            limit: Some(9_223_372_036_854_775_807),
            hidden: 120,
            total_cols: 200,
            running: true,
            failed: true,
            ..base.clone()
        },
        Spec { chips: vec![("name = '\u{540d}\u{524d}\u{540d}\u{524d}\u{540d}\u{524d}'", false)], ..applied.clone() },
        Spec { icons: false, backgrounds: false, ..applied.clone() },
        Spec { rows: None, ..applied },
    ]
}

#[test]
fn every_width_fits_steps_are_monotone_and_nothing_applied_leaves() {
    for lang in [Lang::En, Lang::Ko] {
        let w = Words::new(lang);
        for (k, s) in specs().iter().enumerate() {
            let m = bar(&w, s);
            let mut last = usize::MAX;
            for width in 30..=200 {
                let l = layout(&m, width);
                let what = format!("{lang:?} spec {k} width {width}: {}", text(&l));
                assert!(l.width() <= width, "does not fit: {what}");
                assert!(l.level <= last, "a step back: {what}");
                last = l.level;
                // The richest form that fits: no step before it fits.
                for k in 0..l.level {
                    assert!(at_level(&m, k, width).is_none(), "step {k} fits too: {what}");
                }
                // No step is wider than the one before it: none is passed over for a narrower
                // one that says less.
                if width == 200 {
                    let whole = |k: usize| at_level(&m, k, usize::MAX).map(|l| l.width()).unwrap_or(0);
                    for k in 1..LEVELS.len() {
                        assert!(whole(k) <= whole(k - 1), "step {k} wider than step {}: {what}", k - 1);
                    }
                }
                if !s.chips.is_empty() {
                    assert!(has(&l, BarHit::More) || has(&l, BarHit::Chip(0)), "conditions left: {what}");
                }
                if !s.order.is_empty() {
                    assert!(has(&l, BarHit::Order), "order left: {what}");
                }
                if s.limit.is_some() {
                    assert!(has(&l, BarHit::Limit), "limit left: {what}");
                }
                if s.hidden > 0 {
                    assert!(has(&l, BarHit::Cols), "hidden columns left: {what}");
                }
                if s.failed {
                    assert!(l.segs().any(|g| g.tone == Tone::Warn && g.hit.is_none()), "state left: {what}");
                }
                if s.running {
                    assert!(l.segs().any(|g| g.tone == Tone::Spinner), "spinner left: {what}");
                }
                assert!(has(&l, BarHit::Advanced), "no way to the dialog: {what}");
            }
        }
    }
}

#[test]
fn the_whole_bar_and_its_steps() {
    let w = Words::new(Lang::En);
    let s = &specs()[1];
    let m = bar(&w, s);
    let whole = layout(&m, 200);
    assert_eq!(whole.level, 0);
    assert_eq!(
        text(&whole),
        "WHERE  status = 'paid' ×   total_amount > 1000 ×  + │ ORDER BY ↓ total_amount, ↑ ordered_at + │ LIMIT 500 │ 7 cols │ 1–500 of ~50,001 rows · in tx · planner ‹ › | ⋯ Advanced"
    );
    // Narrower, each step in its turn.
    let at = |level: usize| {
        (30..=200).rev().map(|n| layout(&m, n)).find(|l| l.level >= level).map(|l| text(&l)).unwrap_or_default()
    };
    assert!(at(1).ends_with("| ⋯"), "{}", at(1));
    assert!(at(2).contains("~50,001 rows · in tx · planner"), "{}", at(2));
    assert!(at(3).contains("~50k rows"), "{}", at(3));
    assert!(at(4).contains("ORDER BY ↓ total_amount +1"), "{}", at(4));
    assert!(at(5).contains(" +1  +"), "{}", at(5));
    assert!(at(6).contains("ORDER ↓ total_amount +1"), "{}", at(6));
    assert!(at(7).contains("~50k rows · in tx ‹ ›") && !at(7).contains("planner"), "notes go: {}", at(7));
    assert!(at(9).contains("2 conditions"), "{}", at(9));
    assert!(!at(2).contains("7 cols"), "nothing hidden: the count goes early: {}", at(2));
    assert!(!at(11).contains("cols"), "{}", at(11));
    assert!(at(12).contains("ORDER ↓2"), "{}", at(12));
    // The short form's own steps, each as it is drawn.
    let short = |level: usize| text(&at_level(&m, level, 200).unwrap());
    assert!(short(13).ends_with("│ in tx | ⋯"), "the rows go, their state stays: {}", short(13));
    assert_eq!(short(14), "▼2 + │ ↓2 │ L500 │ ~50k rows · in tx ‹ › | ⋯");
    assert_eq!(short(15), "▼2 + │ ↓2 │ L500 │ in tx ‹ › | ⋯");
    assert_eq!(short(16), "▼2 + ↓2 L500 in tx ‹ › | ⋯");
    assert_eq!(short(17), "▼2 + ↓2 L500 in tx | ⋯");
    assert_eq!(short(18), "▼2 ↓2 L500 in tx | ⋯");
    assert_eq!(short(20), "▼2 ↓2 L in tx | ⋯");
    // The keys' letters underlined: W̲HERE O̲RDER BY L̲IMIT c̲ols r̲ows A̲dvanced.
    let ul: Vec<String> =
        whole.segs().filter_map(|g| g.ul.map(|i| g.text.chars().nth(i).unwrap().to_string())).collect();
    assert_eq!(ul, ["W", "O", "L", "c", "r", "A"]);
}

#[test]
fn the_empty_bar_is_dim() {
    let w = Words::new(Lang::En);
    let m = bar(&w, &specs()[0]);
    let l = layout(&m, 200);
    assert_eq!(
        text(&l),
        "WHERE + │ ORDER BY + │ LIMIT none │ 7 cols │ 1–500 of ~50,001 rows · in tx · planner ‹ › | ⋯ Advanced"
    );
    let dim: Vec<&str> = l.segs().filter(|g| g.tone == Tone::KeywordDim).map(|g| g.text.as_str()).collect();
    assert_eq!(dim, ["WHERE", "ORDER BY", "LIMIT"]);
}

#[test]
fn chips_without_backgrounds_are_in_brackets_of_the_same_width() {
    let w = Words::new(Lang::En);
    let s = Spec { chips: vec![("status = 'paid'", false)], total_cols: 7, ..Spec::default() };
    let on = layout(&bar(&w, &Spec { backgrounds: true, ..s.clone() }), 200);
    let off = layout(&bar(&w, &s), 200);
    assert!(text(&off).starts_with("WHERE [status = 'paid' ×] +"), "{}", text(&off));
    assert_eq!(on.width(), off.width());
    let brackets: Vec<Tone> = off.segs().filter(|g| g.text == "[" || g.text == "]").map(|g| g.tone).collect();
    assert_eq!(brackets, [Tone::Bracket, Tone::Bracket]);
}

#[test]
fn an_expression_is_in_the_warning_colour_and_hidden_columns_too() {
    let w = Words::new(Lang::En);
    let l = layout(&bar(&w, &specs()[2]), 200);
    assert!(l.segs().any(|g| g.tone == Tone::ChipExpr && g.hit == Some(BarHit::Chip(0))));
    let l = layout(&bar(&w, &specs()[6]), 200);
    let cols = l.segs().find(|g| g.hit == Some(BarHit::Cols)).unwrap();
    assert_eq!((cols.text.as_str(), cols.tone), ("6/7 cols", Tone::Warn));
}

#[test]
fn the_overflow_is_its_own_part_and_the_focus_follows_a_chip_into_it() {
    let w = Words::new(Lang::En);
    let mut m = bar(&w, &specs()[1]);
    m.focus = Some(BarHit::Chip(1));
    let narrow = (30..=200).rev().map(|n| layout(&m, n)).find(|l| l.level == 5).unwrap();
    let more: Vec<&Seg> = narrow.segs().filter(|g| g.hit == Some(BarHit::More)).collect();
    assert_eq!(more.iter().map(|g| g.text.as_str()).collect::<String>(), " +1 ");
    assert!(more.iter().all(|g| g.focus), "{}", text(&narrow));
    // Where it is drawn, the chip and its `×` have the focus.
    let whole = layout(&m, 200);
    let focused: Vec<Option<BarHit>> = whole.segs().filter(|g| g.focus).map(|g| g.hit).collect();
    assert!(focused.contains(&Some(BarHit::Chip(1))) && focused.contains(&Some(BarHit::Remove(1))));
}

#[test]
fn numbers_are_short_and_bounded() {
    assert_eq!(compact(500), "500");
    assert_eq!(compact(50_001), "50k");
    assert_eq!(compact(1_250_000), "1.2M");
    assert_eq!(compact(9_223_372_036_854_775_807), "9.2E");
    assert_eq!(capped(100), "99+");
    assert_eq!(capped(99), "99");
}

#[test]
fn placed_puts_the_right_end_at_the_right() {
    let w = Words::new(Lang::En);
    let l = layout(&bar(&w, &specs()[0]), 120);
    let placed = l.placed(10, 120);
    let (x, last) = placed.last().unwrap();
    assert_eq!(usize::from(*x) + width(&last.text), 130);
}

/// The bar's room at 80x24 with the explorer open (the pane inside its border, a blank at both
/// ends).
const NARROW: usize = 52;

#[test]
fn at_80_columns_the_short_form_keeps_every_part_it_can() {
    let w = Words::new(Lang::En);
    let applied = Spec { rows: Some(50_001), ..specs()[1].clone() };
    // Without a paging state the long form still fits, the rows left out.
    let mut quiet = bar(&w, &applied);
    if let Some(r) = quiet.rows.as_mut() {
        r.state = None;
    }
    let l = layout(&quiet, NARROW);
    assert_eq!(text(&l), "WHERE  2 conditions  + │ ORDER ↓2 │ LIMIT 500 | ⋯");
    let hidden = Spec { hidden: 1, ..applied.clone() };
    assert_eq!(text(&layout(&bar(&w, &hidden), NARROW)), "▼2 + │ ↓2 │ L500 │ 6/7 │ ~50k rows · in tx ‹ › | ⋯");
    let failed = Spec { failed: true, hidden: 1, ..applied.clone() };
    assert_eq!(text(&layout(&bar(&w, &failed), NARROW)), "failed │ ▼2 + │ ↓2 │ L500 │ 6/7 │ in tx ‹ › | ⋯");
    // At 120 columns (the pane with the explorer) `6/7 cols` keeps its word: the conditions
    // shorten first.
    let l = layout(&bar(&w, &hidden), 92);
    assert!(text(&l).contains("6/7 cols"), "{}", text(&l));
    // At 120 columns the first condition stays with the count of the others: the rows' notes
    // and range go before it is counted.
    // (The rows of a capped result read whole: no paging state, no note.)
    let mut plain = bar(&w, &Spec { limit: Some(500), ..applied.clone() });
    if let Some(r) = plain.rows.as_mut() {
        (r.state, r.notes, r.arrows) = (None, Vec::new(), None);
        (r.full, r.mid, r.short) = ("500 rows".into(), "500 rows".into(), "500 rows".into());
    }
    let l = layout(&plain, 86);
    assert_eq!(text(&l), "WHERE  status = 'paid' ×   +1  + │ ORDER BY ↓ total_amount +1 │ LIMIT 500 │ 500 rows | ⋯");
    // The limit as a short number in the short form.
    let big = Spec { limit: Some(50_000), failed: true, hidden: 1, ..applied };
    assert!(text(&layout(&bar(&w, &big), NARROW)).contains("L50k"), "{}", text(&layout(&bar(&w, &big), NARROW)));
}

#[test]
fn a_limit_typed_shows_selected_until_a_key_replaces_it() {
    let w = Words::new(Lang::En);
    let mut m = bar(&w, &specs()[1]);
    m.limit = LimitM::Editing { text: "500".into(), cursor: 3, selected: true };
    let l = layout(&m, 200);
    let field = l.segs().find(|g| g.tone == Tone::FieldSelected).unwrap();
    assert_eq!((field.text.as_str(), field.caret), ("500", Some(2)), "the cap's columns, the cursor on its last");
}

/// The field opening with the cap in it takes the cap's columns: the bar does not move, at any
/// width, in the long forms and the short ones.
#[test]
fn the_limit_field_opens_without_moving_the_bar() {
    for lang in [Lang::En, Lang::Ko] {
        let w = Words::new(lang);
        for (k, s) in specs().iter().enumerate().filter(|(_, s)| s.limit == Some(500)) {
            let applied = bar(&w, s);
            let mut editing = applied.clone();
            editing.limit = LimitM::Editing { text: "500".into(), cursor: 3, selected: true };
            for width in 30..=200 {
                let (a, e) = (layout(&applied, width), layout(&editing, width));
                let what = format!("{lang:?} spec {k} width {width}: {} / {}", text(&a), text(&e));
                assert_eq!((a.level, a.width()), (e.level, e.width()), "{what}");
                let others = |l: &Line| {
                    l.segs().filter(|g| g.hit != Some(BarHit::Limit)).map(|g| g.text.clone()).collect::<Vec<_>>()
                };
                assert_eq!(others(&a), others(&e), "{what}");
            }
        }
    }
}

/// The bar at 80x24 with the explorer open and a hidden column, the result read whole (500
/// rows, one page): the short form keeps the rows' words, it does not leave half the bar blank.
#[test]
fn at_80_columns_a_hidden_column_keeps_the_rows() {
    let w = Words::new(Lang::En);
    let mut m = bar(&w, &Spec { hidden: 1, ..specs()[1].clone() });
    if let Some(r) = m.rows.as_mut() {
        (r.state, r.notes, r.arrows) = (None, Vec::new(), None);
        (r.full, r.mid, r.short) = ("500 rows".into(), "500 rows".into(), "500 rows".into());
    }
    let l = layout(&m, NARROW);
    assert_eq!(text(&l), "▼2 + │ ↓2 │ L500 │ 6/7 │ 500 rows | ⋯");
    assert!(has(&l, BarHit::Rows));
}

/// Without backgrounds the focus is in the text alone: no block of the selection behind it, the
/// brackets in the accent.
#[test]
fn the_focus_without_backgrounds_is_in_the_text() {
    let th = theme::cur();
    let base = Style::new().fg(th.fg_muted);
    let bracket = focused(base, Tone::Bracket, false);
    assert_eq!((bracket.fg, bracket.bg), (Some(th.accent), None));
    assert!(!bracket.add_modifier.contains(Modifier::REVERSED));
    let text = focused(Style::new().fg(th.fg), Tone::Chip, false);
    assert_eq!((text.fg, text.bg), (Some(th.fg), None));
    assert!(text.add_modifier.contains(Modifier::BOLD | Modifier::UNDERLINED));
    assert!(!text.add_modifier.contains(Modifier::REVERSED));
    // With backgrounds: the selection.
    assert_eq!(focused(base, Tone::Chip, true).bg, th.selection.bg);
}

/// The bar while its `LIMIT` is typed in, at every width: the field is drawn (at most a few
/// columns of it, around its cursor, which is in it) and nothing applied leaves, also with the
/// largest cap and a wide paste.
#[test]
fn the_limit_field_is_drawn_at_every_width() {
    let wide = "\u{540d}\u{524d}".repeat(10);
    let texts = [("9223372036854775807".to_string(), 19), (wide.clone(), 20), (wide.clone(), 6), (String::new(), 0)];
    for lang in [Lang::En, Lang::Ko] {
        let w = Words::new(lang);
        for base in [specs()[1].clone(), Spec { hidden: 1, failed: true, running: true, ..specs()[1].clone() }] {
            for (text, cursor) in &texts {
                let mut m = bar(&w, &base);
                m.limit = LimitM::Editing { text: text.clone(), cursor: *cursor, selected: false };
                for width in 30..=200 {
                    let l = layout(&m, width);
                    let what = format!("{lang:?} {text} @{cursor} width {width}: {}", super::tests::text(&l));
                    assert!(l.width() <= width, "does not fit: {what}");
                    let field = l.segs().find(|g| g.caret.is_some()).unwrap_or_else(|| panic!("no field: {what}"));
                    assert!(field.caret.unwrap() < crate::text::width(&field.text), "cursor out of the field: {what}");
                    assert!(has(&l, BarHit::More) || has(&l, BarHit::Chip(0)), "{what}");
                    assert!(has(&l, BarHit::Order), "{what}");
                    if base.hidden > 0 {
                        assert!(has(&l, BarHit::Cols), "{what}");
                    }
                }
            }
        }
    }
}

/// The paging state (held, in a transaction, a countdown in the warning colour) stays in the
/// short forms, bounded.
#[test]
fn the_paging_state_stays_in_the_short_forms() {
    let w = Words::new(Lang::En);
    for (state, warn) in [("paging", false), ("in tx", false), ("4:59", true), ("first page only", true)] {
        let mut m = bar(&w, &Spec { hidden: 1, ..specs()[1].clone() });
        if let Some(r) = m.rows.as_mut() {
            r.state = Some((state.to_string(), warn));
        }
        for level in 13..LEVELS.len() - 1 {
            let l = at_level(&m, level, 200).unwrap();
            let g = l
                .segs()
                .find(|g| g.hit == Some(BarHit::Rows) && !g.text.contains("rows"))
                .unwrap_or_else(|| panic!("{state} at {level}: {}", text(&l)));
            let mark = g.text.trim_start_matches(" \u{00b7} ");
            assert!(crate::text::width(mark) <= STATE_MAX, "{}", g.text);
            assert_eq!(g.tone, if warn { Tone::Warn } else { Tone::Muted });
        }
    }
}

/// The last form is presence marks within 16 columns besides the run's state and the paging
/// mark, whatever is applied (counts capped, the cap only as `L`), in every language.
#[test]
fn the_glyph_form_stays_within_its_budget() {
    let last = LEVELS.len() - 1;
    for lang in [Lang::En, Lang::Ko] {
        let w = Words::new(lang);
        for (k, s) in specs().iter().enumerate() {
            let m = bar(&w, s);
            let l = Line { left: line(&m, LEVELS[last], None), right: right(&m, LEVELS[last]), level: last };
            let state = m.state.is_some();
            let mut marks = 0;
            let mut lead = state;
            for g in l.segs() {
                // The run's state comes first and carries no target.
                if lead && g.hit.is_none() {
                    continue;
                }
                lead = false;
                // The paging mark is bounded on its own.
                if g.hit == Some(BarHit::Rows) {
                    assert!(crate::text::width(&g.text) <= STATE_MAX, "{lang:?} spec {k}: {}", g.text);
                    continue;
                }
                marks += crate::text::width(&g.text);
            }
            assert!(marks <= 16, "{lang:?} spec {k}: {marks} columns: {}", text(&l));
        }
    }
}
