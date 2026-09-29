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
