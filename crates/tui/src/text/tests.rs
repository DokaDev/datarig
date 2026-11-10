use super::*;

const SAMPLES: &[&str] = &[
    "陳大文",
    "大文🐘",
    "臺北市信義區信義路五段7號, 台北101大樓 10樓",
    "👨‍👩‍👧‍👦 family",
    "🔥🚀✨🎉",
    "沖縄県那覇市おもろまち 242 🏝️",
    "絵文字の幅テスト 🇯🇵🇹🇼🇺🇸 ✅❌⚠️",
    "東京都渋谷区道玄坂1-2-3",
    "é combining",
    "e\u{301}e\u{301}e\u{301}",
    "Plain ASCII User",
    "",
];

#[test]
fn widths_follow_spec() {
    assert_eq!(width("👨‍👩‍👧‍👦"), 2);
    assert_eq!(width("🇰🇷"), 2);
    assert_eq!(width("e\u{301}"), 1);
    assert_eq!(width("🏝️"), 2);
    assert_eq!(width("⚠️"), 2);
    assert_eq!(width("漢字"), 4);
    assert_eq!(width("…"), 1); // ambiguous -> 1
    assert_eq!(width("→↵"), 2);
}

#[test]
fn fit_is_always_exact_width() {
    for s in SAMPLES {
        for w in 0..30 {
            let f = fit(s, w, Align::Left);
            assert_eq!(width(&f), w, "fit({s:?}, {w}) = {f:?}");
            let f = fit(s, w, Align::Right);
            assert_eq!(width(&f), w, "fit right({s:?}, {w}) = {f:?}");
        }
    }
}

#[test]
fn truncation_never_splits_graphemes() {
    for s in SAMPLES {
        let gs: Vec<&str> = s.graphemes(true).collect();
        for w in 1..20 {
            let f = fit(s, w, Align::Left);
            if width(s) <= w {
                continue;
            }
            let body = f.trim_end_matches(' ');
            let body = body.strip_suffix('…').expect("ends with ellipsis");
            // body must be a prefix made of whole graphemes
            let bgs: Vec<&str> = body.graphemes(true).collect();
            assert_eq!(&gs[..bgs.len()], &bgs[..], "split grapheme in {s:?} at {w}");
        }
    }
}

#[test]
fn wide_char_leaves_space_pad() {
    // 漢(2) 字(2) + … (1) = 5, one column left over -> space
    assert_eq!(fit("漢字漢字", 6, Align::Left), "漢字… ");
    assert_eq!(fit("漢字漢字", 5, Align::Left), "漢字…");
    assert_eq!(fit("👨‍👩‍👧‍👦 family", 3, Align::Left), "👨‍👩‍👧‍👦…");
    assert_eq!(fit("👨‍👩‍👧‍👦 family", 2, Align::Left), "… ");
    assert_eq!(fit("12", 5, Align::Right), "   12");
}

#[test]
fn sanitize_replaces_controls() {
    assert_eq!(sanitize_cell("a\nb\tc\r\nd"), "a↵b→c↵d");
}

#[test]
fn wrap_respects_width() {
    let lines = wrap("あいうえおかき", 5);
    assert_eq!(lines, vec!["あい", "うえ", "おか", "き"]);
    for l in wrap("長い 紹介文です。 Long bio 🇯🇵🇯🇵 text", 7) {
        assert!(width(&l) <= 7);
    }
    assert_eq!(wrap("a\nb", 10), vec!["a", "b"]);
}

#[test]
fn wrap_words_keeps_words_whole() {
    assert_eq!(wrap_words("OS不 可以用 權限 的 確認一下", 12), ["OS不 可以用", "權限 的", "確認一下"]);
    assert_eq!(wrap_words("a verylongword b", 5), ["a", "veryl", "ongwo", "rd b"]);
    assert_eq!(wrap_words("", 5), [""]);
}

#[test]
fn counts_and_sizes_read_short() {
    let counts =
        [0, 7, 999, 1_000, 4_249, 9_960, 11_000, 10_999, 999_499, 999_500, 3_140_000, 12_000_000, 2_000_000_000];
    assert_eq!(
        counts.map(human_count),
        ["0", "7", "999", "1k", "4.2k", "10k", "11k", "11k", "999k", "1M", "3.1M", "12M", "2B"]
    );
    let sizes = [0, 512, 1023, 1024, 8192, 4_404_019, 19_505_152, 5 << 30, 3 << 40];
    assert_eq!(sizes.map(human_bytes), ["0 B", "512 B", "1023 B", "1 KB", "8 KB", "4.2 MB", "19 MB", "5 GB", "3 TB"]);
}

#[test]
fn clip_middle_keeps_the_end() {
    let lock = "(structure unavailable: the table is locked by another session (try again))";
    assert_eq!(clip_middle(lock, 200), lock);
    let short = clip_middle(lock, 50);
    assert_eq!((width(&short), short.as_str()), (50, "(structure unavailable: the table is… (try again))"));
    let trigger = "t  BEFORE INSERT OR UPDATE OF name, \"Mixed Col\" · FOR EACH ROW · shop.touch()";
    assert_eq!(clip_middle(trigger, 50), "t  BEFORE INSERT OR UPDATE OF name… · shop.touch()");
    // No short end to keep: cut at the end.
    let long = format!("{} ({})", "a".repeat(40), "b".repeat(40));
    assert_eq!(clip_middle(&long, 30), clip(&long, 30));
    // Wide graphemes: never over the width.
    let wide = "(構造を読めません: 別のセッションがテーブルをロックしています (再試行してください))";
    let tail = width(&wide[wide.rfind(" (").unwrap()..]);
    for w in 1..width(wide) {
        let c = clip_middle(wide, w);
        assert!(width(&c) <= w && (w * 2 / 3 < tail || c.ends_with("))")), "{w}: {c}");
    }
}

#[test]
fn wrap_exact_keeps_every_character() {
    let sql = "SET search_path = 'a  b', \"x   y\", public, reporting_with_a_long_name";
    for w in 2..=sql.len() {
        let parts = wrap_exact(sql, w);
        assert_eq!(parts.concat(), sql, "{w}");
        assert!(parts.iter().all(|p| width(p) <= w), "{w}: {parts:?}");
    }
    // The blanks where it breaks start the next piece.
    assert_eq!(wrap_exact("SET x = 'a  b'", 10), ["SET x = 'a", "  b'"]);
    assert_eq!(wrap_exact("", 10), [""]);
    let wide = "SET a = '構造を読めません'";
    assert_eq!(wrap_exact(wide, 7).concat(), wide);
    assert!(wrap_exact(wide, 7).iter().all(|p| width(p) <= 7));
}

#[test]
fn slice_cols_shows_a_window_of_columns() {
    assert_eq!(slice_cols("abcdef", 0, 3), "abc");
    assert_eq!(slice_cols("abcdef", 2, 3), "cde");
    assert_eq!(slice_cols("abcdef", 4, 9), "ef", "no padding past the end");
    assert_eq!(slice_cols("abc", 5, 3), "");
    assert_eq!(slice_cols("abc", 1, 0), "");
    // A combining mark stays with its base.
    assert_eq!(slice_cols("e\u{301}xe\u{301}", 1, 2), "xe\u{301}");
}

#[test]
fn slice_cols_blanks_a_wide_character_cut_by_an_edge() {
    // Three wide characters (columns 0-1, 2-3, 4-5).
    let s = "\u{6F22}\u{5B57}\u{AC00}";
    assert_eq!(slice_cols(s, 0, 6), s);
    // The left edge cuts the first one: its right half shows as a blank.
    assert_eq!(slice_cols(s, 1, 5), " \u{5B57}\u{AC00}");
    // The right edge cuts the last one: its left half shows as a blank.
    assert_eq!(slice_cols(s, 0, 5), "\u{6F22}\u{5B57} ");
    // Both edges cut one.
    assert_eq!(slice_cols(s, 1, 4), " \u{5B57} ");
    assert_eq!(slice_cols(s, 3, 1), " ");
    for skip in 0..7 {
        for w in 0..7 {
            let part = slice_cols(s, skip, w);
            assert_eq!(width(&part), w.min(6usize.saturating_sub(skip)), "slice_cols({skip}, {w}) = {part:?}");
        }
    }
}

#[test]
fn control_characters_are_spelled_out_and_a_backslash_doubled_only_before_an_escape() {
    assert_eq!(escape_controls("plain \u{1F418} text"), "plain \u{1F418} text");
    assert_eq!(escape_controls("a\r\nb\tc\u{7}d"), r"a\r\nb\tc\x07d");
    assert_eq!(escape_controls("\u{0}\u{1b}\u{7f}\u{85}\u{9f}\u{2028}"), r"\x00\x1b\x7f\u{85}\u{9f}\u{2028}");
    // A backslash stays one unless what follows would read as an escape.
    assert_eq!(escape_controls(r"a\b\db"), r"a\b\db");
    assert_eq!(escape_controls(r"a\nb \t \x41 \u \\ end\"), r"a\\nb \\t \\x41 \\u \\\ end\");
    assert_eq!(escape_controls("\\\n"), r"\\\n");
}

#[test]
fn direction_and_unseen_characters_are_spelled_out_and_joiners_kept() {
    assert_eq!(escape_controls("a\u{202E}b\u{200B}c\u{FEFF}"), r"a\u{202e}b\u{200b}c\u{feff}");
    let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    assert_eq!(escape_controls(family), family);
}

#[test]
fn an_escaped_text_is_cut_before_an_escape_never_in_it() {
    assert_eq!(clip_escaped(r"abc\x07def", 5), "abc\u{2026}");
    assert_eq!(clip_escaped(r"abc\x07def", 8), "abc\\x07\u{2026}");
    assert_eq!(clip_escaped(r"ab\u{85}cd", 7), "ab\u{2026}");
    assert_eq!(clip_escaped(r"ab\\n", 4), "ab\u{2026}");
    assert_eq!(clip_escaped(r"ab\c", 4), r"ab\c");
    assert_eq!(clip_escaped("short", 9), "short");
}

#[test]
fn whole_parts_never_cut_a_part() {
    let hint = "WHERE  f filter · s sort · Ctrl+R history";
    assert_eq!(whole_parts(hint, 60), hint);
    assert_eq!(whole_parts(hint, width(hint)), hint);
    assert_eq!(whole_parts(hint, width(hint) - 1), "WHERE  f filter · s sort");
    assert_eq!(whole_parts(hint, 24), "WHERE  f filter · s sort");
    assert_eq!(whole_parts(hint, 23), "WHERE  f filter");
    assert_eq!(whole_parts(hint, 15), "WHERE  f filter");
    assert_eq!(whole_parts(hint, 10), "WHERE  f …");
    // Wide (two-column) letters: "WHERE  f <wide> · s <wide> · Ctrl+R <wide>".
    let ko = "WHERE  f \u{d544}\u{d130} \u{b7} s \u{c815}\u{b82c} \u{b7} Ctrl+R \u{ae30}\u{b85d}";
    assert_eq!(whole_parts(ko, width(ko) - 1), "WHERE  f \u{d544}\u{d130} \u{b7} s \u{c815}\u{b82c}");
}
