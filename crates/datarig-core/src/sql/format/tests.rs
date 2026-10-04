use super::*;

const OPTS: Options = Options { case: KeywordCase::Preserve, indent: 4 };

fn fmt(src: &str) -> Result<String, Refused> {
    format(src, OPTS, "").map(|(_, text)| text)
}

/// What the formatter promises, checked on its own: the same tokens (a keyword's case apart),
/// operator characters together or apart as they were, strings separated by a line break or not
/// as they were.
fn assert_layout_only(src: &str, out: &str) {
    let sig = |s: &str| lex(s).into_iter().filter(|t| t.kind != Tok::Whitespace).collect::<Vec<_>>();
    let (a, b) = (sig(src), sig(out));
    assert_eq!(a.len(), b.len(), "token count\n{src}\n---\n{out}");
    for (i, (x, y)) in a.iter().zip(&b).enumerate() {
        assert_eq!(x.kind, y.kind, "{src}\n---\n{out}");
        let (tx, ty) = (x.text(src), y.text(out));
        if x.kind == Tok::Keyword {
            assert!(tx.eq_ignore_ascii_case(ty), "{tx} / {ty}");
        } else {
            assert_eq!(tx, ty, "{src}\n---\n{out}");
        }
        if i > 0 {
            let (gx, gy) = (&src[a[i - 1].end..x.start], &out[b[i - 1].end..y.start]);
            if a[i - 1].kind == Tok::Op && x.kind == Tok::Op {
                assert_eq!(gx.is_empty(), gy.is_empty(), "operator characters {tx:?}\n{src}\n---\n{out}");
            }
            if a[i - 1].kind == Tok::Str && x.kind == Tok::Str {
                assert_eq!(gx.contains('\n'), gy.contains('\n'), "string continuation\n{src}\n---\n{out}");
            }
        }
    }
}

#[test]
fn a_messy_query_is_laid_out() {
    let src = "select a,b,count(*) n from shop.orders o join shop.users u on u.id=o.user_id \
               where o.status='paid' and x::int>=3 group by 1,2 order by 3 desc limit 10;";
    let want = "select\n    a,\n    b,\n    count(*) n\nfrom\n    shop.orders o\n    join shop.users u on u.id = o.user_id\n\
                where\n    o.status = 'paid'\n    and x::int >= 3\ngroup by\n    1,\n    2\norder by\n    3 desc\nlimit\n    10;";
    assert_eq!(fmt(src).unwrap(), want);
    assert_layout_only(src, want);
    // Formatting it again changes nothing.
    assert_eq!(fmt(want).unwrap(), want);
}

/// Keywords take the asked case; names, strings and comments never change.
#[test]
fn keyword_case() {
    let src = "Select Count(*), \"Mixed\", 'Text' From Foo -- Comment\nWhere x In (1)";
    let upper = format(src, Options { case: KeywordCase::Upper, indent: 2 }, "").unwrap().1;
    assert_eq!(upper, "SELECT\n  Count(*),\n  \"Mixed\",\n  'Text'\nFROM\n  Foo -- Comment\nWHERE\n  x IN (1)");
    let lower = format(src, Options { case: KeywordCase::Lower, indent: 2 }, "").unwrap().1;
    assert_eq!(lower, "select\n  Count(*),\n  \"Mixed\",\n  'Text'\nfrom\n  Foo -- Comment\nwhere\n  x in (1)");
    let kept = format(src, Options { case: KeywordCase::Preserve, indent: 2 }, "").unwrap().1;
    assert!(kept.starts_with("Select\n  Count(*),") && kept.contains("\nFrom\n  Foo -- Comment\nWhere\n"), "{kept}");
}

/// Lines after the first start at the column the text starts at.
#[test]
fn continuation_lines_get_the_indent() {
    let (span, out) = format("  select a from t\n", OPTS, "  ").unwrap();
    assert_eq!(span, 2..17, "the blanks around stay");
    assert_eq!(out, "select\n      a\n  from\n      t");
}

/// A dollar-quoted body is never touched, while the code around it is laid out.
#[test]
fn dollar_bodies_stay_as_written() {
    let body = "$fn$\n  SELECT  1 ;   -- inside\n  select 'x'\n$fn$";
    let src = format!("create function f() returns int language sql as {body}; select 1,2");
    let out = fmt(&src).unwrap();
    assert!(out.contains(body), "{out}");
    assert_layout_only(&src, &out);
    assert!(out.ends_with("select\n    1,\n    2"), "{out}");
}

/// What `sqlformat` gets wrong is refused, and the error says where.
#[test]
fn changes_beyond_layout_are_refused() {
    // `U&'…'` would become the operator `&` between a column `U` and a string.
    assert_eq!(fmt("SELECT U&'d\\0061t' FROM t"), Err(Refused::Changed { at: 8 }));
    // Two strings separated by a line break are one string; joined on a line they are an error.
    assert!(fmt("select 'a'\n'b'").is_err());
    // A psql command would come apart.
    assert!(fmt("SELECT 1 \\gset").is_err());
}

/// The corpus: every text either formats with only its layout changed, or is refused.
#[test]
fn the_formatter_never_changes_tokens() {
    let corpus = [
        "SELECT 1 -- one\n, 2 /* two */ FROM t",
        "SELECT /* a /* nested */ comment */ 1",
        "select 1 -- no line break after the comment",
        "-- only a comment",
        "CREATE FUNCTION f() RETURNS int AS $$ SELECT  1 ;  $$ LANGUAGE sql",
        "DO $do$ BEGIN RAISE NOTICE 'x'; END $do$",
        "SELECT E'a\\'b\\n', e'\\\\', B'101', X'ff', N'n' FROM t",
        "SELECT 'it''s', 'x' 'y', \"Mixed Col\", \"a\"\"b\" FROM \"Order Lines\"",
        "SELECT '\u{d55c}\u{ae00}' AS \"\u{c774}\u{b984}\", \u{d14c}\u{c774}\u{be14}.x FROM \u{d14c}\u{c774}\u{be14}",
        "SELECT j->>'k', j#>'{a}', j @> '{}', a<=b, a<>b, a!=b, a||b, x ~* 'y', a=>1 FROM t",
        "SELECT a - -1, a--1\n, 2",
        "SELECT :foo, :'bar', :\"baz\" FROM t",
        "SELECT 1 \\gset\n\\echo :x",
        "SELECT $1, $2::text, arr[1], t.*, (x).y, 1.5e-3, .5 FROM t",
        "WITH RECURSIVE x(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM x WHERE n<10) SELECT * FROM x",
        "INSERT INTO t (a,b) VALUES (1,2),(3,4) ON CONFLICT (a) DO UPDATE SET b=excluded.b RETURNING *",
        "UPDATE t SET a = a + 1 WHERE id IN (SELECT id FROM u WHERE flag) RETURNING id",
        "SELECT CASE WHEN a THEN 'x' ELSE 'y' END, CAST(x AS numeric(10,2)), interval '1 day' FROM t",
        "SELECT count(*) FILTER (WHERE x) OVER (PARTITION BY y ORDER BY z ROWS BETWEEN 1 PRECEDING AND CURRENT ROW)",
        "COPY t TO STDOUT WITH (FORMAT csv)",
        "select 'unterminated",
        "select \"unterminated",
        "select /* unterminated",
        "SELECT 1; SELECT 2;; SELECT 3",
        "SELECT U&\"d\\0061t\" UESCAPE '!' FROM t",
        "SELECT * FROM t WHERE a LIKE '%x%' ESCAPE '\\' AND b SIMILAR TO 'y'",
        "SELECT x#y, x # y, @x, |/x, ||/x, !!x",
        "SELECT 'a'\n  'b' AS joined",
        "SELECT\t1\r\nFROM\tt",
    ];
    let mut formatted = 0;
    for src in corpus {
        for case in [KeywordCase::Preserve, KeywordCase::Upper, KeywordCase::Lower] {
            for indent in ["", "    "] {
                if let Ok((span, out)) = format(src, Options { case, indent: 2 }, indent) {
                    assert_layout_only(&src[span], &out);
                    formatted += 1;
                }
            }
        }
    }
    assert!(formatted > corpus.len(), "most of the corpus formats: {formatted}");
}

/// Random texts made of tricky pieces: whatever is not refused only changed its layout.
#[test]
fn random_texts_only_change_their_layout() {
    let pieces = [
        "select",
        "SELECT",
        "from",
        "where",
        "and",
        "or",
        "(",
        ")",
        ",",
        ";",
        "a",
        "b.c",
        "\"Q x\"",
        "'s'",
        "E'\\''",
        "$$ x ; y $$",
        "$t$a$t$",
        "--c\n",
        "/*c*/",
        "-",
        "+",
        "<",
        "=",
        ">",
        "::",
        ":",
        "|",
        "&",
        "@",
        "#",
        "~",
        "!",
        "*",
        "/",
        "1",
        "2.5",
        "$1",
        "U",
        "\\",
        "\n",
        " ",
        "\t",
        "join",
        "on",
        "case",
        "when",
        "end",
        "\u{d55c}",
        "'\u{ae00}'",
    ];
    let mut seed: u64 = 0x5eed;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) as usize
    };
    for _ in 0..3000 {
        let n = 1 + next() % 24;
        let src: String = (0..n)
            .map(|_| {
                let p = pieces[next() % pieces.len()];
                if next() % 3 == 0 { format!("{p} ") } else { p.to_string() }
            })
            .collect();
        if let Ok((span, out)) = format(&src, OPTS, "") {
            assert_layout_only(&src[span], &out);
        }
    }
}
