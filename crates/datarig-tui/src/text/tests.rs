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
