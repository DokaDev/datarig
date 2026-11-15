use super::*;
use crate::app::filter_dialog::draft::{ColumnInfo, Draft, Undo};
use crate::app::filter_dialog::{ColumnSource, DialogFocus, DialogHit};
use crate::app::tabs::TabId;
use crate::widgets::text_input::TextInput;
use datarig_core::query::{ColumnFacts, ColumnKind, Filter, Query, TableName, Target};
use datarig_core::sql::dialect::Dialect;
use std::collections::HashMap;
use std::time::Instant;

fn column(name: &str) -> ColumnInfo {
    ColumnInfo {
        name: name.into(),
        type_name: "text".into(),
        kind: ColumnKind::Text,
        meta: None,
        enum_known: false,
        primary_key: false,
        foreign_key: false,
        not_null: false,
        invisible: false,
    }
}

fn popup(kind: PopupKind) -> View {
    View::Popup { kind, anchor: Anchor { seg: Some(BarHit::Where), at: Rect::new(3, 1, 10, 1) } }
}

/// A dialog shown as the condition popup of `email`, mid-session: a condition on `email`, a
/// draft changed once, an apply waiting to be sent, the column search open on `ma`, the
/// operator picker open, the pointer over and pressing a part drawn.
pub(crate) fn dialog() -> FilterDialog {
    let target = Target::new(
        Dialect::Postgres,
        vec![ColumnFacts::new("id", ColumnKind::Number), ColumnFacts::new("email", ColumnKind::Text)],
    );
    let text = "email = 'a'".to_string();
    let filter = Some(Filter { recognised: datarig_core::query::recognise(&text, &target), text });
    let q = Query {
        filter,
        limit: Some(5),
        ..Query::table(TableName { database: None, schema: "s".into(), name: "t".into() })
    };
    let draft = Draft::from_query(&q, &target);
    let mut undo = Undo::default();
    undo.record(&Draft::from_query(&Query { limit: None, ..q.clone() }, &target), None);
    let hits = DialogHits { add: Rect::new(1, 1, 4, 1), ..Default::default() };
    let mut press = crate::app::overlay::Press::default();
    press.drawn(true, Instant::now() - crate::app::overlay::ARM_DELAY * 2);
    let down = ratatui::crossterm::event::MouseEventKind::Down(ratatui::crossterm::event::MouseButton::Left);
    press.press(down, Some(DialogHit::Add), Instant::now());
    assert_eq!(press.armed, Some(DialogHit::Add));
    FilterDialog {
        tab: TabId(1),
        table: "s.t".into(),
        dialect: Dialect::Postgres,
        columns: vec![column("id"), column("email")],
        source: ColumnSource::Result,
        loaded: Some(3),
        limit: TextInput::new(&draft.limit),
        applied: Draft::from_query(&Query { limit: None, ..q.clone() }, &target),
        before_last: Some(Draft::from_query(&Query { limit: Some(1), ..q.clone() }, &target)),
        draft,
        undo,
        focus: DialogFocus::Columns,
        cursor: 0,
        scroll: 0,
        search: Some(TextInput::new("ma")),
        picker: Some(1),
        inputs: Vec::new(),
        message: None,
        mark: None,
        sql: None,
        applying: Some(q),
        applying_hidden: Some(vec!["id".into()]),
        view: popup(PopupKind::Condition { col: "email".into() }),
        view_gen: 0,
        hits,
        press,
        hover: Some(DialogHit::Add),
        narrow: None,
        values_scroll: 0,
        rows_scroll: 0,
        listed: 0,
        values_drawn: true,
        list_of: None,
        tally: None,
        counting: None,
        exact: HashMap::new(),
    }
}

/// The session the dialog edits, as the parts every view shares.
fn session(d: &FilterDialog) -> (Draft, usize, Option<Query>, Option<Vec<String>>, Option<Draft>) {
    (d.draft.clone(), d.undo.len(), d.applying.clone(), d.applying_hidden.clone(), d.before_last.clone())
}

/// What was drawn, pointed at and opened is gone.
fn dropped(d: &FilterDialog) {
    assert!(d.hits.at(1, 1).is_none(), "where things were drawn is dropped");
    assert_eq!(d.hover, None);
    assert_eq!(d.press.armed, None);
    assert_eq!(d.press.shown_at, None, "a press waits again before it arms");
    assert_eq!(d.picker, None);
}

#[test]
fn a_change_of_view_keeps_the_session_and_drops_what_was_drawn() {
    let mut d = dialog();
    let kept = session(&d);
    assert!(d.is_popup() && d.confined());
    assert_eq!(d.popup_kind().map(PopupKind::id), Some(PopupKindId::Condition));
    assert_eq!(d.column().map(|c| c.name.as_str()), Some("email"));

    // A popup that picks a column keeps the search.
    d.set_view(popup(PopupKind::AddPick));
    assert_eq!(d.view_gen(), 1);
    assert_eq!(d.popup_kind().map(PopupKind::id), Some(PopupKindId::AddPick));
    assert_eq!(d.search.as_ref().map(TextInput::text), Some("ma"));
    assert_eq!(session(&d), kept);
    dropped(&d);

    // Another popup closes it, the column selected kept.
    d.picker = Some(0);
    d.set_view(popup(PopupKind::Condition { col: "email".into() }));
    assert_eq!(d.view_gen(), 2);
    assert!(d.search.is_none());
    assert_eq!(d.column().map(|c| c.name.as_str()), Some("email"));
    assert_eq!(session(&d), kept);
    dropped(&d);

    // The whole dialog, the same session.
    d.set_view(View::Full);
    assert_eq!(d.view(), &View::Full);
    assert_eq!(d.view_gen(), 3);
    assert!(!d.is_popup() && !d.confined() && d.popup_kind().is_none());
    assert_eq!(session(&d), kept);
    dropped(&d);
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "does not turn into a popup")]
fn the_whole_dialog_does_not_turn_into_a_popup() {
    let mut d = dialog();
    d.set_view(View::Full);
    d.set_view(popup(PopupKind::Order));
}
