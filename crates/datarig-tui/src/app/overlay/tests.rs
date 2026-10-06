use super::*;
use crate::app::profiles::ProfileForm;
use crate::widgets::text_input::TextInput;

fn commands() -> Overlay {
    Overlay::Commands(CommandLine {
        input: TextInput::default(),
        items: Vec::new(),
        selected: 0,
        error: None,
        picked: false,
        list: Default::default(),
        offset: 0,
    })
}

fn viewer() -> Overlay {
    Overlay::CellViewer(Viewer { column: "c".into(), text: String::new(), scroll: 0, view_h: 1 })
}

fn kinds(o: &Overlays) -> Vec<OverlayKind> {
    o.iter().map(Overlay::kind).collect()
}

#[test]
fn stack_keeps_rank_order_and_replaces_same_kind() {
    let mut o = Overlays::default();
    assert!(o.is_empty() && !o.modal() && o.top().is_none());
    o.push(viewer());
    assert!(!o.modal(), "the cell viewer does not block the screen");
    o.push(commands());
    o.push(Overlay::ProfileForm(Box::new(ProfileForm::new_profile())));
    assert_eq!(kinds(&o), [OverlayKind::CellViewer, OverlayKind::ProfileForm, OverlayKind::Commands]);
    assert_eq!(o.top().map(Overlay::kind), Some(OverlayKind::Commands), "the command line stays on top");
    assert!(o.modal());
    o.push(commands());
    assert_eq!(kinds(&o).len(), 3, "same kind replaced, not duplicated");
    o.close(OverlayKind::Commands);
    assert_eq!(o.top().map(Overlay::kind), Some(OverlayKind::ProfileForm));
    assert!(o.form_mut().is_some() && o.command_line().is_none() && o.viewer().is_some());
    o.close(OverlayKind::ProfileForm);
    o.close(OverlayKind::CellViewer);
    assert!(o.is_empty());
}

#[test]
fn a_confirmations_buttons_put_the_safe_one_first_and_enter_on_it() {
    let confirm = |action, keys| Confirm {
        title: Label::QuitTitle,
        text: Label::QuitTx.into(),
        details: Vec::new(),
        keys,
        action,
        folder: None,
        path: None,
        buttons: Buttons::default(),
    };
    let (b, enter) = confirm(ConfirmAction::Quit, Label::QuitKeys).buttons();
    assert_eq!(b, [(Label::DialogButtonStay, KeyCode::Char('n')), (Label::DialogButtonQuit, KeyCode::Char('y'))]);
    assert_eq!(enter, Some(0), "Enter stays");
    let (b, enter) = confirm(ConfirmAction::TrustHostKey, Label::SshHostKeyKeys).buttons();
    assert_eq!((b[1].0, enter), (Label::DialogButtonTrust, Some(0)));
    let (b, enter) = confirm(ConfirmAction::Copy, Label::CopyConfirmKeys).buttons();
    assert_eq!((b[1].0, enter), (Label::DialogButtonCopy, Some(1)), "Enter copies: nothing is lost");
    let conflict = ConfirmAction::ScriptConflict(crate::app::TabId(1));
    let (b, enter) = confirm(conflict, Label::ScriptsConflictKeys).buttons();
    let keys: Vec<KeyCode> = b.iter().map(|b| b.1).collect();
    assert_eq!((keys, enter), (vec![KeyCode::Esc, KeyCode::Char('r'), KeyCode::Char('o')], None));
    let (b, _) = confirm(conflict, Label::ScriptsConflictMissingKeys).buttons();
    assert_eq!(b, [(Label::DialogButtonLater, KeyCode::Esc), (Label::DialogButtonSaveAgain, KeyCode::Char('o'))]);
}

#[test]
fn buttons_say_which_one_is_under_the_pointer_and_whether_that_changed() {
    let mut b = Buttons { rects: vec![Rect::new(10, 5, 6, 1), Rect::new(19, 5, 8, 1)], ..Default::default() };
    assert_eq!(b.at(10, 5), Some(0));
    assert_eq!(b.at(16, 5), None, "between them");
    assert_eq!(b.at(26, 5), Some(1));
    assert!(!b.hover(0, 0));
    assert!(b.hover(20, 5));
    assert!(!b.hover(21, 5), "the same button");
    assert!(b.hover(12, 5));
    assert!(b.hover(12, 6), "off them");
    assert_eq!(b.hover, None);
}

#[test]
fn a_button_acts_on_the_release_over_the_one_pressed_once_armed() {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let (down, up) = (MouseEventKind::Down(MouseButton::Left), MouseEventKind::Up(MouseButton::Left));
    let t0 = Instant::now();
    let mut b = Buttons { rects: vec![Rect::new(10, 5, 6, 1), Rect::new(19, 5, 8, 1)], ..Default::default() };
    // Not drawn yet: nothing is armed.
    assert_eq!(b.press(down, 20, 5, t0), None);
    assert_eq!(b.press(up, 20, 5, t0), None);
    b.shown_at = Some(t0);
    let late = t0 + ARM_DELAY;
    assert_eq!(b.press(down, 20, 5, t0 + ARM_DELAY / 2), None);
    assert_eq!(b.press(up, 20, 5, late), None, "pressed within the delay");
    assert_eq!(b.press(down, 20, 5, late), None, "a press only arms");
    assert_eq!(b.press(up, 20, 5, late), Some(1));
    assert_eq!(b.press(down, 20, 5, late), None);
    assert_eq!(b.press(up, 11, 5, late), None, "released over the other button");
    assert_eq!(b.press(up, 20, 5, late), None, "a release disarms");
}
