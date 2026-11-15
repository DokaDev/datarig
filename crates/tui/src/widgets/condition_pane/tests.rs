use super::*;
use crate::app::filter_dialog::view::tests::dialog;
use crate::app::overlay::Overlay;

/// The text of each row of `buf`.
fn lines(buf: &Buffer) -> Vec<String> {
    let a = buf.area;
    (a.y..a.bottom()).map(|y| (a.x..a.right()).map(|x| buf[(x, y)].symbol()).collect()).collect()
}

/// An app with the dialog of [`dialog`] open, its keys on the operator of `email`'s condition.
fn app() -> App {
    let mut app = App::new(&datarig_core::config::Config::default(), None, datarig_i18n::Lang::En);
    let mut d = dialog();
    d.focus = DialogFocus::Op(0);
    app.overlays.push(Overlay::FilterDialog(Box::new(d)));
    app
}

#[test]
fn a_pane_named_by_its_border_starts_with_its_conditions_in_short_words() {
    let app = app();
    let d = app.filter_dialog().expect("open");
    let label = |l: Label| app.i18n.label(l).to_string();
    let draw = |rect: Rect, opts: PaneOpts| {
        let mut buf = Buffer::empty(Rect::new(0, 0, 90, 12));
        let mut hits = DialogHits::default();
        let out = draw_condition_pane(&app, d, None, rect, opts, &mut buf, &mut hits);
        (lines(&buf), hits, out)
    };

    // As the whole dialog draws it: the name, the `Filter` heading, then the condition.
    let rect = Rect::new(2, 1, 80, 8);
    let (rows, hits, out) = draw(rect, PaneOpts::default());
    assert!(rows[1].contains("email"), "{rows:#?}");
    assert!(rows[3].contains(&label(Label::TableDialogFilter)), "{rows:#?}");
    assert_eq!(hits.ops.first().map(|o| o.0.y), Some(4));
    assert!(rows[8].contains(&format!("\u{2191} {}", label(Label::TableDialogSortAsc))), "{rows:#?}");
    assert_eq!(out.picker_at, Some((4, 5)));

    // Named by its border and compact: the condition on the first row, the short sort words
    // at any width.
    let opts = PaneOpts { title_in_border: true, compact: true };
    let (rows, hits, out) = draw(rect, opts);
    assert!(!rows.iter().any(|r| r.contains("email") || r.contains(&label(Label::TableDialogFilter))), "{rows:#?}");
    assert_eq!(hits.ops.first().map(|o| o.0.y), Some(1));
    assert!(rows[8].contains(&format!("\u{2191} {}", label(Label::TableDialogSortAscShort))), "{rows:#?}");
    assert_eq!(out.picker_at, Some((4, 2)));
    assert_eq!(out.rows_scroll, 0);
}
