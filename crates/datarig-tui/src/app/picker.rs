//! The lists to pick from with a typed filter ([`ListPicker`]) as dialogs. The profile form's
//! SSH tunnel is one: `Enter` on the field (or a click on its value) lists no tunnel, the
//! tunnel presets, the profile's own tunnel and, when the config file can be written, "New SSH
//! tunnel…", which opens the tunnel form over the profile form; once that is saved, the profile
//! form comes back with the new preset picked (cancelled: as it was).

use super::profiles::{FormKind, SshChoice};
use super::*;
use crate::widgets::list_picker::{Entry, ListPicker, Outcome};

/// A list to pick from, by what it picks.
pub enum Picker {
    /// The SSH tunnel of the profile form of profile `form` (a pick goes to that form only).
    Tunnel { form: ProfileId, list: ListPicker<SshChoice> },
}

impl Picker {
    /// The SSH tunnel's list.
    pub fn tunnel(&self) -> Option<&ListPicker<SshChoice>> {
        match self {
            Picker::Tunnel { list, .. } => Some(list),
        }
    }

    /// Its title.
    pub fn title(&self) -> Label {
        match self {
            Picker::Tunnel { .. } => Label::FormFieldSshEnabled,
        }
    }

    /// It came on top again ([`ListPicker::rearm`]).
    pub fn rearm(&mut self) {
        match self {
            Picker::Tunnel { list, .. } => list.rearm(),
        }
    }

    /// The pointer moved to (x, y) ([`ListPicker::pointer`]).
    fn pointer(&mut self, x: u16, y: u16) -> bool {
        match self {
            Picker::Tunnel { list, .. } => list.pointer(x, y),
        }
    }

    /// Draw it ([`ListPicker::draw`]).
    pub(crate) fn draw(
        &mut self,
        area: Rect,
        buf: &mut ratatui::buffer::Buffer,
        texts: (&Localized, &Localized),
        notes: (&str, &str),
        top: bool,
        now: Instant,
    ) -> (u16, u16) {
        match self {
            Picker::Tunnel { list, .. } => list.draw(area, buf, texts, notes, top, now),
        }
    }
}

impl App {
    /// `Enter` on the profile form's SSH tunnel: the list of what it can be, the current one
    /// selected.
    pub(super) fn open_tunnel_picker(&mut self) {
        let Some(f) = self.overlays.form().filter(|f| f.kind == FormKind::Profile) else { return };
        let i18n = &self.i18n;
        let entries = f
            .ssh_choices()
            .into_iter()
            .map(|c| {
                let (label, hint) = match &c {
                    SshChoice::Off => {
                        (i18n.label(Label::FormSshOff).to_string(), i18n.label(Label::FormSshOffHint).to_string())
                    }
                    SshChoice::Inline => {
                        let s = f.ssh_settings();
                        let hint = if s.host.trim().is_empty() { String::new() } else { Self::bastion_text(&s) };
                        (i18n.label(Label::FormSshOwn).to_string(), hint)
                    }
                    SshChoice::Preset(name) if f.new_preset.as_ref() == Some(name) => (
                        i18n.msg(&Msg::FormSshPresetNew { name: name.clone() }).to_string(),
                        Self::bastion_text(&f.ssh_settings()),
                    ),
                    SshChoice::Preset(name) => match f.presets.iter().find(|p| p.name == *name) {
                        Some(p) => (name.clone(), Self::bastion_text(&p.settings)),
                        None => {
                            (i18n.msg(&Msg::FormSshPresetMissing { name: name.clone() }).to_string(), String::new())
                        }
                    },
                };
                Entry { value: c, label, hint }
            })
            .collect();
        let new = self.config_writable().then(|| i18n.label(Label::FormSshPickerNew).to_string());
        let list = ListPicker::new(entries, new, Some(&f.ssh_choice()));
        let form = f.profile_id();
        self.overlays.push(Overlay::Picker(Picker::Tunnel { form, list }));
    }

    /// Keys of a list to pick from (`overlay.picker`, text input).
    pub(super) fn picker_key(&mut self, key: KeyEvent, repeat: bool) {
        if let Some(&mut Picker::Tunnel { form, ref mut list }) = self.overlays.picker_mut() {
            let out = list.key(&key, repeat);
            self.tunnel_picked(form, out);
        }
    }

    /// Text pasted while a list to pick from has the keyboard: into its filter.
    pub(super) fn picker_paste(&mut self, text: &str) {
        if let Some(Picker::Tunnel { list, .. }) = self.overlays.picker_mut() {
            list.paste(text);
        }
    }

    /// The mouse on a list to pick from: a row is picked on the release over the row pressed.
    pub(super) fn picker_mouse(&mut self, m: MouseEvent) {
        let now = self.now();
        if let Some(&mut Picker::Tunnel { form, ref mut list }) = self.overlays.picker_mut() {
            let out = list.mouse(&m, now);
            self.tunnel_picked(form, out);
        }
    }

    /// The pointer over a list to pick from: `true` when the row highlighted changed.
    pub(super) fn picker_hover(&mut self, m: MouseEvent) -> bool {
        self.overlays.picker_mut().is_some_and(|p| p.pointer(m.column, m.row))
    }

    /// What the tunnel's list of profile `form`'s form did: a pick sets that form's tunnel;
    /// "New SSH tunnel…" puts that form aside under a new tunnel form. Either only while that
    /// form is the dialog on top once the list closed.
    fn tunnel_picked(&mut self, form: ProfileId, out: Outcome<SshChoice>) {
        if out == Outcome::Pending {
            return;
        }
        self.overlays.close(OverlayKind::Picker);
        let on_top = matches!(
            self.overlays.top(),
            Some(Overlay::ProfileForm(f)) if f.kind == FormKind::Profile && f.profile_id() == form
        );
        match out {
            Outcome::Picked(c) if on_top => {
                if let Some(f) = self.overlays.form_mut() {
                    f.set_ssh_choice(c);
                }
            }
            Outcome::New if on_top => {
                let Some(mut back) = self.overlays.take_form() else { return };
                // A running test would ask its questions over the tunnel form: it stops.
                self.cancel_test();
                back.parked_test = self.conn_test.take();
                self.open_tunnel_form(None, false);
                if let Some(f) = self.overlays.form_mut() {
                    f.back_to = Some(back);
                }
            }
            _ => {}
        }
    }
}
