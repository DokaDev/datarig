use super::*;
use datarig_core::sql::lexer::{Tok, lex};
use datarig_core::sql::split::{split, statement_at};

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
        "\n;",
        ";SELECT 2",
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
        // Search one to three lines around the cursor first, so the region has to grow.
        e.region = 1 + rng.below(3);
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

/// Setting the language the editor already has keeps the cached line states; read again from
/// the start, as a new language reads them, they are the whole text's states.
#[test]
fn set_language_keeps_the_states_of_the_same_language() {
    use datarig_core::sql::dialect::{Dialect, Language};
    let text = "SELECT 'a;\nb';\n/* c\nd */ SELECT $$x\ny$$;\nSELECT \"q\nr\";";
    let mut e = Editor::new(text);
    assert_eq!(e.language(), Language::Sql(Dialect::Postgres));
    e.ensure_states(usize::MAX);
    let (valid, version) = (e.valid, e.version());
    e.set_language(Language::Sql(Dialect::Postgres));
    assert_eq!(e.language(), Language::Sql(Dialect::Postgres));
    assert_eq!(e.valid, valid, "the same language: the cache stays");
    assert_eq!(e.version(), version, "and so does the text's version");
    e.valid = 0;
    e.ensure_states(usize::MAX);
    assert_eq!(e.states[..e.valid], states_of(text)[..]);
}

/// Right after a `;` that starts its line, below lines that do not reach the statement it ends,
/// the cursor is in that statement, as in the whole text (not in the one after it). The first
/// texts are that case; the last ones (nothing before the `;`, or the next statement on another
/// line) were right before and must stay so.
#[test]
fn right_after_a_semicolon_that_starts_its_line_is_the_statement_it_ends() {
    for text in [
        "select 1\n\n;select 2",
        "select 1\n-- c\n;select 2",
        "select 1;\n\n;select 2",
        "select 1\n\n\n\n\n\n;\nselect 2",
        "\n\n;select 2",
    ] {
        let mut e = Editor::new(text);
        e.region = 1;
        let off = text.find("\n;").expect("a ; starting a line") + 2;
        e.row = text[..off].matches('\n').count();
        e.col = 1;
        assert_eq!(e.offset(), off);
        let stmts = split(text);
        let want = statement_at(&stmts, off).map(|i| (stmts[i].start, stmts[i].end, stmts[i].body(text).to_string()));
        assert_eq!(e.current_statement(), want, "{text:?}");
    }
}
