use super::*;
use datarig_core::sql::dialect::{Dialect, Language, MySqlMode};
use datarig_core::sql::lexer::{Tok, lex, lex_in};
use datarig_core::sql::split::{segment_at_in, split, split_in, statement_at};

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
                LineState::Inside { line, byte, state: LexState::default() }
            }
            _ => LineState::Normal(LexState::default()),
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
            let (base, region, state) = e.region_text(first, last);
            assert_eq!(state, LexState::default());
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
                let (base, region, _, c) = e.completion_context();
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

const MYSQL: Dialect =
    Dialect::MySql(MySqlMode { ansi_quotes: false, no_backslash_escapes: false, dollar_quotes: false });

/// MySQL-ish text: `DELIMITER` lines (also ones that are not commands: in a statement, or not at
/// a line's start), terminators that are not `;`, executable comments, `#` and `-- ` comments,
/// backslash escapes, backticks and variables, with `;` inside them and tokens that span lines.
fn mysqlish(rng: &mut Rng, pieces: usize) -> String {
    const P: &[&str] = &[
        "SELECT 1",
        ";",
        ";",
        " ",
        "\n",
        "\n\n",
        "DELIMITER //\n",
        "delimiter $$\n",
        "DELIMITER ;\n",
        "  Delimiter\t;;\n",
        "delimiter",
        "//",
        "$$",
        ";;",
        "end$$",
        "BEGIN SELECT 1; END",
        "/*!80000 ",
        "/*! x; */",
        "*/",
        "/* c; \n */",
        "/*+ h; */",
        "# c; ' \n",
        "-- d;\n",
        "--x",
        "'a\\';\nb'",
        "\"s;\n\"",
        "`q;\n``r`",
        "@'v;'",
        "@@x.y",
        "x.y",
        "t.1e5",
        "0x1F",
        "'unterminated",
        "/* open",
        "\n;",
        ";SELECT 2",
        "\r\n",
        "\\\n",
        "\\G",
        "$t$ x;\n $t$",
        "x'4\\'; '",
    ];
    (0..pieces).map(|_| P[rng.below(P.len())]).collect()
}

/// The MySQL modes the fuzz tests read text in: the default, and dollar quotes with
/// `ANSI_QUOTES` and `NO_BACKSLASH_ESCAPES`.
const MODES: [Dialect; 2] =
    [MYSQL, Dialect::MySql(MySqlMode { ansi_quotes: true, no_backslash_escapes: true, dollar_quotes: true })];

/// The lexer state at every line start of text in dialect `d`, from a lex of the whole text.
fn states_in(text: &str, d: Dialect) -> Vec<LineState> {
    let toks = lex_in(text, d);
    let mut starts = vec![0];
    starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    let pos = |off: usize| {
        let line = starts.partition_point(|s| *s <= off) - 1;
        (line, off - starts[line])
    };
    let (mut ti, mut state) = (0, LexState::default());
    starts
        .iter()
        .map(|&p| {
            while ti < toks.len() && toks[ti].end <= p {
                state = state.after(&toks[ti], text, d);
                ti += 1;
            }
            match toks.get(ti) {
                Some(t) if t.start < p && t.kind != Tok::Whitespace => {
                    let (line, byte) = pos(t.start);
                    LineState::Inside { line, byte, state }
                }
                _ => LineState::Normal(state),
            }
        })
        .collect()
}

/// The token kind of every byte of `text`, lexed in `d` from `state`.
fn kinds_in(text: &str, d: Dialect, state: LexState) -> Vec<Tok> {
    let mut out = vec![Tok::Whitespace; text.len()];
    for t in datarig_core::sql::lexer::lex_from(text, d, state) {
        for k in &mut out[t.start..t.end] {
            *k = t.kind;
        }
    }
    out
}

fn mysql_editor(text: &str, d: Dialect) -> Editor {
    let mut e = Editor::new(text);
    e.set_language(Language::Sql(d));
    e
}

/// MySQL: the cached line states carry the terminator a `DELIMITER` line above set, whether a
/// statement has begun and an open executable comment; after edits they, and the tokens of any
/// run of lines lexed from its restart point, are the whole text's.
#[test]
fn mysql_line_states_and_screen_tokens_match_a_whole_lex_after_edits() {
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    for round in 0..400 {
        let n = 5 + rng.below(40);
        let text = mysqlish(&mut rng, n);
        let d = MODES[round % MODES.len()];
        let mut e = mysql_editor(&text, d);
        for upto in (0..e.lines.len()).step_by(1 + rng.below(3)) {
            e.ensure_states(upto);
        }
        assert_eq!(e.states[..e.valid], states_in(&text, d)[..e.valid], "round {round}: {text:?}");
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
            let ins = mysqlish(&mut rng, n);
            e.replace_range(a, b, &ins);
            let t = e.text();
            let want = states_in(&t, d);
            e.ensure_states(usize::MAX);
            assert_eq!(e.states[..e.valid], want[..], "round {round}: {t:?}");
            let first = rng.below(e.lines.len());
            let last = first + 1 + rng.below(4);
            let (base, region, state) = e.region_text(first, last);
            let (whole, part) = (kinds_in(&t, d, LexState::default()), kinds_in(&region, d, state));
            let from = e.line_start(first) - base;
            assert_eq!(
                part[from..],
                whole[base + from..base + part.len()],
                "round {round}: {t:?} lines {first}..{last}"
            );
        }
    }
}

/// MySQL: the statement under the cursor and the completion segment, found in lines around the
/// cursor, are the whole text's (a `DELIMITER` far above the cursor included).
#[test]
fn mysql_statement_under_the_cursor_is_the_one_in_the_whole_text() {
    let mut rng = Rng(0x0DDB_A11C_0FFE_E123);
    for round in 0..200 {
        let n = 10 + rng.below(60);
        let text = mysqlish(&mut rng, n);
        let d = MODES[round % MODES.len()];
        let mut e = mysql_editor(&text, d);
        e.region = 1 + rng.below(3);
        let stmts = split_in(&text, d);
        for row in 0..e.lines.len() {
            for col in 0..=e.gcount(row) {
                e.row = row;
                e.col = col;
                let off = e.offset();
                let want =
                    statement_at(&stmts, off).map(|i| (stmts[i].start, stmts[i].end, stmts[i].body(&text).to_string()));
                assert_eq!(e.current_statement(), want, "round {round}: {text:?} at {off}");
                let (base, region, state, c) = e.completion_context();
                let (a, b) = datarig_core::sql::split::segment_at_from(&region, c, d, state);
                assert_eq!((base + a, base + b), segment_at_in(&text, off, d), "round {round}: {text:?} at {off}");
            }
        }
    }
}

/// A `DELIMITER` 5,000 lines above decides what ends the statement under the cursor, and a
/// Visual selection inside the block is split as the whole text is.
#[test]
fn mysql_delimiter_far_above_the_cursor_ends_the_statement() {
    let mut text = String::from("DELIMITER $$\n");
    for i in 0..5000 {
        text.push_str(&format!("-- filler {i}\n"));
    }
    text.push_str("CREATE PROCEDURE p() BEGIN\n  SELECT 1;\n  SELECT 2;\nEND$$\nDELIMITER ;\nSELECT 3;\n");
    let mut e = mysql_editor(&text, MYSQL);
    e.row = 5002;
    e.col = 4;
    let (_, _, body) = e.current_statement().expect("a statement");
    assert_eq!(body, "CREATE PROCEDURE p() BEGIN\n  SELECT 1;\n  SELECT 2;\nEND");
    e.row = 5006;
    let (_, _, body) = e.current_statement().expect("a statement");
    assert_eq!(body, "SELECT 3");
    // A selection of the procedure's lines is one statement.
    e.row = 5001;
    e.col = 0;
    crate::widgets::editor::tests::typ(&mut e, "Vjjj");
    let (stmts, _) = e.run_statements();
    assert_eq!(stmts, ["CREATE PROCEDURE p() BEGIN\n  SELECT 1;\n  SELECT 2;\nEND"]);
}

/// MySQL text on screen: the `DELIMITER` line as a keyword, an executable comment's opening and
/// closing in their own style with the code inside highlighted as code, `"…"` as a string,
/// backtick names as quoted names, `#` comments as comments (also on lines lexed from a state
/// below the `DELIMITER`).
#[test]
fn mysql_text_is_highlighted_as_mysql() {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    let text = "DELIMITER //\nSELECT 1 /*!80000 + 1 */, \"s\", `q` # c\n//";
    let mut e = mysql_editor(text, MYSQL);
    let area = Rect::new(0, 0, 60, 4);
    let mut buf = Buffer::empty(area);
    e.render(area, &mut buf, None);
    let th = crate::theme::cur();
    let x0 = e.gutter as u16;
    let fg_at = |line: usize, col: usize| buf[(x0 + col as u16, line as u16)].fg;
    let line1 = e.lines[1].clone();
    let col = |needle: &str| line1.find(needle).expect(needle);
    assert_eq!(fg_at(0, 0), th.syn_keyword.fg.unwrap(), "DELIMITER");
    assert_eq!(fg_at(1, col("/*!")), th.syn_exec_comment.fg.unwrap());
    assert_eq!(fg_at(1, col("*/")), th.syn_exec_comment.fg.unwrap());
    assert_eq!(fg_at(1, col("+ 1")), th.syn_operator.fg.unwrap(), "code inside");
    assert_eq!(fg_at(1, col("1 */")), th.syn_number.fg.unwrap(), "code inside");
    assert_eq!(fg_at(1, col("\"s\"")), th.syn_string.fg.unwrap());
    assert_eq!(fg_at(1, col("`q`")), th.syn_quoted_ident.fg.unwrap());
    assert_eq!(fg_at(1, col("# c")), th.syn_comment.fg.unwrap());
    assert_ne!(th.syn_exec_comment.fg, th.syn_comment.fg);
}
