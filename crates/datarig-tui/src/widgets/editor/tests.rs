use super::*;
use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

fn key(c: KeyCode) -> KeyEvent {
    KeyEvent { code: c, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE }
}
fn ctrl(c: char) -> KeyEvent {
    KeyEvent {
        code: KeyCode::Char(c),
        modifiers: KeyModifiers::CONTROL,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}
fn typ(e: &mut Editor, s: &str) {
    for c in s.chars() {
        let code = match c {
            '\n' => KeyCode::Enter,
            '\x1b' => KeyCode::Esc,
            '\x08' => KeyCode::Backspace,
            c => KeyCode::Char(c),
        };
        e.handle_key(key(code));
    }
}

#[test]
fn dd_p_u_redo() {
    let mut e = Editor::new("a\nb\nc");
    typ(&mut e, "ddp");
    assert_eq!(e.text(), "b\na\nc");
    typ(&mut e, "u");
    assert_eq!(e.text(), "b\nc");
    typ(&mut e, "u");
    assert_eq!(e.text(), "a\nb\nc");
    e.handle_key(ctrl('r'));
    assert_eq!(e.text(), "b\nc");
}

#[test]
fn yy_p_o_a() {
    let mut e = Editor::new("one\ntwo");
    typ(&mut e, "yyjp");
    assert_eq!(e.text(), "one\ntwo\none");
    typ(&mut e, "ggoNEW\x1b");
    assert_eq!(e.text(), "one\nNEW\ntwo\none");
    typ(&mut e, "A!\x1b");
    assert_eq!(e.text(), "one\nNEW!\ntwo\none");
    typ(&mut e, "u");
    assert_eq!(e.text(), "one\nNEW\ntwo\none");
    typ(&mut e, "GP");
    assert_eq!(e.text(), "one\nNEW\ntwo\none\none");
}

#[test]
fn word_motions() {
    let mut e = Editor::new("SELECT u.name, x\n\nFROM t");
    typ(&mut e, "w");
    assert_eq!((e.row, e.col), (0, 7));
    typ(&mut e, "w");
    assert_eq!((e.row, e.col), (0, 8));
    typ(&mut e, "ww");
    assert_eq!((e.row, e.col), (0, 13));
    typ(&mut e, "ww");
    assert_eq!((e.row, e.col), (1, 0));
    typ(&mut e, "w");
    assert_eq!((e.row, e.col), (2, 0));
    typ(&mut e, "b");
    assert_eq!((e.row, e.col), (1, 0));
    typ(&mut e, "b");
    assert_eq!((e.row, e.col), (0, 15));
    typ(&mut e, "0e");
    assert_eq!((e.row, e.col), (0, 5));
    typ(&mut e, "G");
    assert_eq!(e.row, 2);
    typ(&mut e, "gg");
    assert_eq!(e.row, 0);
}

#[test]
fn grapheme_editing() {
    let mut e = Editor::new("");
    typ(&mut e, "iこんにちは 🐘");
    assert_eq!(e.text(), "こんにちは 🐘");
    assert_eq!(e.col, 7);
    assert_eq!(e.display_x(0, e.col), 13);
    typ(&mut e, "\x08\x08");
    assert_eq!(e.text(), "こんにちは");
    typ(&mut e, " 👨‍👩‍👧‍👦\x08");
    assert_eq!(e.text(), "こんにちは ");
    typ(&mut e, "\x1b0x");
    assert_eq!(e.text(), "んにちは ");
}

#[test]
fn visual_yank_delete_and_selection() {
    let mut e = Editor::new("SELECT 1;\nSELECT 2;");
    typ(&mut e, "vj$");
    assert_eq!(e.selection().unwrap(), "SELECT 1;\nSELECT 2;");
    typ(&mut e, "\x1bgg0vey");
    assert_eq!(e.mode, Mode::Normal);
    typ(&mut e, "$p");
    assert_eq!(e.lines[0], "SELECT 1;SELECT");
    typ(&mut e, "0vld");
    assert_eq!(e.lines[0], "LECT 1;SELECT");
}

#[test]
fn paste_in_insert() {
    let mut e = Editor::new("x");
    typ(&mut e, "i");
    e.paste("a\r\nb");
    assert_eq!(e.text(), "a\nbx");
}

/// A small deterministic random source (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// SQL-ish text with `;` inside strings, comments, dollar bodies and quoted identifiers, and
/// tokens that span lines.
fn sqlish(rng: &mut Rng, pieces: usize) -> String {
    const P: &[&str] = &[
        "SELECT 1",
        ";",
        ";",
        " ",
        "\n",
        "\n\n",
        "'a;\nb'",
        "$$x;\ny$$",
        "$t$ ; $$ \n $t$",
        "/* c; /* nested; */ \n */",
        "-- d;\n",
        "\"q;\n\"",
        "E'\\';'",
        "x.y",
        "f(",
        ")",
        "BEGIN",
        "'unterminated",
        "/* open",
    ];
    (0..pieces).map(|_| P[rng.below(P.len())]).collect()
}

/// The lexer state at every line start, from a lex of the whole text.
fn states_of(text: &str) -> Vec<LineState> {
    let toks = lex(text);
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    let pos = |off: usize| {
        let line = starts.partition_point(|s| *s <= off) - 1;
        (line, off - starts[line])
    };
    starts
        .iter()
        .map(|&p| match toks.iter().find(|t| t.start < p && p < t.end) {
            Some(t) if matches!(t.kind, Tok::BlockComment | Tok::Str | Tok::Dollar | Tok::QuotedIdent) => {
                let (line, byte) = pos(t.start);
                LineState::Inside { line, byte }
            }
            _ => LineState::Normal,
        })
        .collect()
}

/// The token kind of every byte of `text`.
fn kinds(text: &str) -> Vec<Tok> {
    let mut out = vec![Tok::Whitespace; text.len()];
    for t in lex(text) {
        for k in &mut out[t.start..t.end] {
            *k = t.kind;
        }
    }
    out
}

#[test]
fn line_states_and_screen_tokens_match_a_whole_lex_after_edits() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for round in 0..300 {
        let n = 5 + rng.below(40);
        let text = sqlish(&mut rng, n);
        let mut e = Editor::new(&text);
        // Cache states a few lines at a time (as scrolling does), then edit somewhere and check
        // again.
        for upto in (0..e.lines.len()).step_by(1 + rng.below(3)) {
            e.ensure_states(upto);
        }
        assert_eq!(e.states[..e.valid], states_of(&text)[..e.valid], "round {round}: {text:?}");
        for _ in 0..3 {
            let len = e.len_bytes();
            let mut a = rng.below(len + 1);
            let mut b = (a + rng.below(8)).min(len);
            let t = e.text();
            while !t.is_char_boundary(a) {
                a -= 1;
            }
            while !t.is_char_boundary(b) {
                b += 1;
            }
            let n = rng.below(3);
            let ins = sqlish(&mut rng, n);
            e.replace_range(a, b, &ins);
            let t = e.text();
            assert_eq!(e.len_bytes(), t.len());
            let want = states_of(&t);
            e.ensure_states(usize::MAX);
            assert_eq!(e.states[..e.valid], want[..], "round {round}: {t:?}");
            // The tokens of any run of lines, lexed from its restart point, are the whole
            // text's tokens there.
            let first = rng.below(e.lines.len());
            let last = first + 1 + rng.below(4);
            let (base, region) = e.region_text(first, last);
            let (whole, part) = (kinds(&t), kinds(&region));
            let from = e.line_start(first) - base;
            assert_eq!(
                part[from..],
                whole[base + from..base + part.len()],
                "round {round}: {t:?} lines {first}..{last}"
            );
        }
    }
}

#[test]
fn the_statement_under_the_cursor_is_the_one_in_the_whole_text() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    for round in 0..200 {
        let n = 10 + rng.below(60);
        let text = sqlish(&mut rng, n);
        let mut e = Editor::new(&text);
        // Search a single line around the cursor first, so the region has to grow.
        e.region = 1;
        let stmts = split(&text);
        for row in 0..e.lines.len() {
            for col in 0..=e.gcount(row) {
                e.row = row;
                e.col = col;
                let off = e.offset();
                let want =
                    statement_at(&stmts, off).map(|i| (stmts[i].start, stmts[i].end, stmts[i].body(&text).to_string()));
                assert_eq!(e.current_statement(), want, "round {round}: {text:?} at {off}");
                let (base, region, c) = e.completion_context();
                let (a, b) = datarig_core::sql::split::segment_at(&region, c);
                assert_eq!(
                    (base + a, base + b),
                    datarig_core::sql::split::segment_at(&text, off),
                    "round {round}: {text:?} at {off}"
                );
            }
        }
    }
}

#[test]
fn undo_and_redo_walk_every_step_back_and_forth() {
    let mut e = Editor::new("alpha beta\ngamma\ndelta;\nepsilon");
    let mut seen = vec![e.text()];
    for keys in ["x", "dd", "oNEW line\x1b", "yyP", "jjp", "ia\nb\x08\x08c\x1b", "vjd", "A!\x1b", "Otop\x1b"] {
        typ(&mut e, keys);
        if e.text() != *seen.last().unwrap() {
            seen.push(e.text());
        }
    }
    for want in seen.iter().rev().skip(1) {
        typ(&mut e, "u");
        assert_eq!(&e.text(), want);
    }
    typ(&mut e, "u");
    assert_eq!(e.text(), seen[0], "nothing more to undo");
    for want in &seen[1..] {
        e.handle_key(ctrl('r'));
        assert_eq!(&e.text(), want);
    }
    assert_eq!(e.len_bytes(), e.text().len());
}

#[test]
fn edits_in_a_large_text_touch_only_their_lines() {
    let text: String = (0..50_000).map(|i| format!("SELECT {i} FROM t WHERE x = '{i};';\n")).collect();
    let mut e = Editor::new(&text);
    e.row = 25_000;
    let v = e.version();
    typ(&mut e, "A -- note\x1b");
    assert_ne!(e.version(), v);
    assert!(e.lines[25_000].ends_with(" -- note"));
    typ(&mut e, "ddu");
    assert_eq!(e.lines.len(), 50_001);
    assert_eq!(e.len_bytes(), e.text().len());
    let (start, _, body) = e.current_statement().unwrap();
    assert!(body.starts_with("SELECT 25000 FROM t"), "{body}");
    assert_eq!(start, e.line_start(25_000));
}
