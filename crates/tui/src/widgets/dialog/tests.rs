use super::*;
use datarig_i18n::{I18n, Lang};

/// The bottom row of a modal box `w` wide with the preview's keys, in `lang`.
fn bottom(w: u16, lang: Lang) -> (Buffer, u16) {
    let mut buf = Buffer::empty(Rect::new(0, 0, w + 4, 5));
    let i18n = I18n { lang };
    modal(Rect::new(2, 0, w, 5), &i18n.label(Label::DialogButtonCancel), &i18n.label(Label::EditPreviewKeys), &mut buf);
    (buf, 4)
}

#[test]
fn the_keys_keep_a_border_cell_by_each_corner_and_are_cut_when_too_long() {
    for lang in [Lang::En, Lang::Ko] {
        for w in [40u16, 120] {
            let (buf, y) = bottom(w, lang);
            let (left, right) = (2, 2 + w - 1);
            // As the terminal shows it: the cell a wide character covers is skipped.
            let mut row = String::new();
            let mut x = left;
            while x <= right {
                row.push_str(buf[(x, y)].symbol());
                x += crate::text::width(buf[(x, y)].symbol()).max(1) as u16;
            }
            assert_eq!(buf[(left, y)].symbol(), "\u{2570}", "{lang:?} {w}: {row}");
            assert_eq!(buf[(right, y)].symbol(), "\u{256F}", "{lang:?} {w}: {row}");
            assert_eq!(buf[(right - 1, y)].symbol(), "\u{2500}", "{lang:?} {w}: {row}");
            assert_eq!(buf[(left + 1, y)].symbol(), "\u{2500}", "{lang:?} {w}: {row}");
            let keys = Label::EditPreviewKeys.text(lang);
            if w == 40 {
                assert!(row.contains('\u{2026}') && !row.contains(keys), "{lang:?} cut: {row}");
            } else {
                assert!(row.contains(keys), "{lang:?} whole: {row}");
            }
        }
    }
}
