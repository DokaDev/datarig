//! The lists to pick from with a typed filter ([`ListPicker`]) as dialogs, each for a field of
//! the profile form: the SSH tunnel and the environment. `Enter` on the field (or a click on its
//! value) lists what it can be and, when the config file can be written, a last entry that
//! makes a new one ("New SSH tunnel…", "New environment…"), which opens its form over the
//! profile form; once that is saved, the profile form comes back with the new one picked
//! (cancelled: as it was).
//!
//! The SSH tunnel's list has no tunnel, the tunnel presets and the profile's own tunnel. The
//! environment's has the default rules, each environment (its color, badge and rules), the
//! name the profile has when no environment has it, and while there is no environment the
//! presets (`dev`, `test`, `staging`, `prod`): a preset picked is written with the profile.

use super::profiles::{EnvChoice, FormKind, SshChoice};
use super::*;
use crate::theme;
use crate::widgets::list_picker::{Entry, ListPicker, Outcome};
use datarig_core::policy::{self, Environment};

/// A list to pick from, by what it picks (a pick goes to the profile form of profile `form`
/// only).
pub enum Picker {
    /// The SSH tunnel.
    Tunnel { form: ProfileId, list: ListPicker<SshChoice> },
    /// The environment.
    Environment { form: ProfileId, list: ListPicker<EnvChoice> },
}

/// `$body` with the list of whichever picker `$p` is, as `$list`.
macro_rules! with_list {
    ($p:expr, $list:ident => $body:expr) => {
        match $p {
            Picker::Tunnel { $list, .. } => $body,
            Picker::Environment { $list, .. } => $body,
        }
    };
}

impl Picker {
    /// The SSH tunnel's list.
    pub fn tunnel(&self) -> Option<&ListPicker<SshChoice>> {
        match self {
            Picker::Tunnel { list, .. } => Some(list),
            Picker::Environment { .. } => None,
        }
    }

    /// The environment's list.
    pub fn environment(&self) -> Option<&ListPicker<EnvChoice>> {
        match self {
            Picker::Environment { list, .. } => Some(list),
            Picker::Tunnel { .. } => None,
        }
    }

    /// Its title.
    pub fn title(&self) -> Label {
        match self {
            Picker::Tunnel { .. } => Label::FormFieldSshEnabled,
            Picker::Environment { .. } => Label::FormFieldEnvironment,
        }
    }

    /// It came on top again ([`ListPicker::rearm`]).
    pub fn rearm(&mut self) {
        with_list!(self, list => list.rearm())
    }

    /// The pointer moved to (x, y) ([`ListPicker::pointer`]).
    fn pointer(&mut self, x: u16, y: u16) -> bool {
        with_list!(self, list => list.pointer(x, y))
    }

    /// Text pasted into its filter ([`ListPicker::paste`]).
    fn paste(&mut self, text: &str) {
        with_list!(self, list => list.paste(text))
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
        with_list!(self, list => list.draw(area, buf, texts, notes, top, now))
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
                Entry::new(c, label, hint)
            })
            .collect();
        let new = self.config_writable().then(|| i18n.label(Label::FormSshPickerNew).to_string());
        let list = ListPicker::new(entries, new, Some(&f.ssh_choice()));
        let form = f.profile_id();
        self.overlays.push(Overlay::Picker(Picker::Tunnel { form, list }));
    }

    /// Keys of a list to pick from (`overlay.picker`, text input).
    pub(super) fn picker_key(&mut self, key: KeyEvent, repeat: bool) {
        match self.overlays.picker_mut() {
            Some(&mut Picker::Tunnel { form, ref mut list }) => {
                let out = list.key(&key, repeat);
                self.tunnel_picked(form, out);
            }
            Some(&mut Picker::Environment { form, ref mut list }) => {
                let out = list.key(&key, repeat);
                self.environment_picked(form, out);
            }
            None => {}
        }
    }

    /// Text pasted while a list to pick from has the keyboard: into its filter.
    pub(super) fn picker_paste(&mut self, text: &str) {
        if let Some(p) = self.overlays.picker_mut() {
            p.paste(text);
        }
    }

    /// The mouse on a list to pick from: a row is picked on the release over the row pressed.
    pub(super) fn picker_mouse(&mut self, m: MouseEvent) {
        let now = self.now();
        match self.overlays.picker_mut() {
            Some(&mut Picker::Tunnel { form, ref mut list }) => {
                let out = list.mouse(&m, now);
                self.tunnel_picked(form, out);
            }
            Some(&mut Picker::Environment { form, ref mut list }) => {
                let out = list.mouse(&m, now);
                self.environment_picked(form, out);
            }
            None => {}
        }
    }

    /// The pointer over a list to pick from: `true` when the row highlighted changed.
    pub(super) fn picker_hover(&mut self, m: MouseEvent) -> bool {
        self.overlays.picker_mut().is_some_and(|p| p.pointer(m.column, m.row))
    }

    /// The list closed with `out`: whether profile `form`'s form is the dialog on top again (a
    /// pick goes there only).
    fn picker_closed<T: PartialEq>(&mut self, form: ProfileId, out: &Outcome<T>) -> Option<bool> {
        if *out == Outcome::Pending {
            return None;
        }
        self.overlays.close(OverlayKind::Picker);
        Some(matches!(
            self.overlays.top(),
            Some(Overlay::ProfileForm(f)) if f.kind == FormKind::Profile && f.profile_id() == form
        ))
    }

    /// What the tunnel's list of profile `form`'s form did: a pick sets that form's tunnel;
    /// "New SSH tunnel…" puts that form aside under a new tunnel form. Either only while that
    /// form is the dialog on top once the list closed.
    fn tunnel_picked(&mut self, form: ProfileId, out: Outcome<SshChoice>) {
        let Some(on_top) = self.picker_closed(form, &out) else { return };
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

    /// `Enter` on the profile form's environment: the list of what it can be, the current one
    /// selected.
    pub(super) fn open_environment_picker(&mut self) {
        let Some(f) = self.overlays.form().filter(|f| f.kind == FormKind::Profile) else { return };
        let i18n = &self.i18n;
        let icons = self.icons_on();
        let tones = |prefix: Option<Label>, p: &datarig_core::policy::Policy| {
            let mut out: Vec<(String, super::environments::RuleTone)> = prefix
                .map(|l| (format!("{} · ", i18n.label(l)), super::environments::RuleTone::Dim))
                .into_iter()
                .collect();
            out.extend(super::environments::rules_tones(i18n, p, icons));
            out
        };
        let entry = |c: EnvChoice, e: Option<&Environment>, label: String, hint: String| {
            let mut out = Entry::new(c, label, hint);
            // The environment's chip in its color (circles with icons off).
            let (on, off) =
                if icons { (crate::icons::ENVIRONMENT_CHIP, crate::icons::ENVIRONMENT_CHIP) } else { ("●", "○") };
            out.mark = Some(match e.and_then(Environment::display_color) {
                Some(pc) => (format!("{on} "), theme::profile_color(pc)),
                None => (format!("{off} "), theme::cur().fg_muted),
            });
            out.badge = e.and_then(Self::badge_chip);
            out
        };
        let mut entries: Vec<Entry<EnvChoice>> = f
            .env_choices()
            .into_iter()
            .filter_map(|c| {
                Some(match &c {
                    EnvChoice::Default => {
                        let label = i18n.label(Label::EnvPickerDefault).to_string();
                        let hint = self.rules_hint(self.policies.default_rules());
                        let mut e = entry(c, None, label, hint);
                        e.hint_tones = tones(None, self.policies.default_rules());
                        e
                    }
                    EnvChoice::Named(name) => match f.environments.iter().find(|e| e.name == *name) {
                        Some(e) => {
                            let mut out = entry(c.clone(), Some(e), name.clone(), self.rules_hint(&e.policy));
                            out.hint_tones = tones(None, &e.policy);
                            out
                        }
                        None => {
                            let label = i18n.msg(&Msg::FormEnvMissing { name: name.clone() }).to_string();
                            let mut e = entry(c, None, label, i18n.label(Label::FormEnvMissingHint).to_string());
                            e.mark = Some(("✕ ".to_string(), theme::cur().error));
                            e
                        }
                    },
                    // Listed below, while there is no environment.
                    EnvChoice::Preset(_) => return None,
                })
            })
            .collect();
        // No environment yet: the presets, each written with the profile once it is saved.
        if f.environments.is_empty() && self.config_writable() {
            for p in policy::presets() {
                let hint = format!("{} · {}", i18n.label(Label::EnvPickerPreset), self.rules_hint(&p.policy));
                let mut e = entry(EnvChoice::Preset(p.name.clone()), Some(&p), p.name.clone(), hint);
                e.hint_tones = tones(Some(Label::EnvPickerPreset), &p.policy);
                entries.push(e);
            }
        } else if let Some(p) = f.made_environment() {
            // A preset picked before an environment was made here: still the one picked, and
            // still written with the profile.
            let hint = format!("{} · {}", i18n.label(Label::EnvPickerPreset), self.rules_hint(&p.policy));
            let label = i18n.msg(&Msg::FormEnvPresetNew { name: p.name.clone() }).to_string();
            let mut e = entry(EnvChoice::Preset(p.name.clone()), Some(p), label, hint);
            e.hint_tones = tones(Some(Label::EnvPickerPreset), &p.policy);
            entries.push(e);
        }
        let new = self.config_writable().then(|| i18n.label(Label::EnvPickerNew).to_string());
        let list = ListPicker::new(entries, new, Some(&f.env_choice()));
        let form = f.profile_id();
        self.overlays.push(Overlay::Picker(Picker::Environment { form, list }));
    }

    /// What the environment's list of profile `form`'s form did: a pick sets that form's
    /// environment; "New environment…" puts that form aside under a new environment form.
    /// Either only while that form is the dialog on top once the list closed.
    fn environment_picked(&mut self, form: ProfileId, out: Outcome<EnvChoice>) {
        let Some(on_top) = self.picker_closed(form, &out) else { return };
        match out {
            Outcome::Picked(c) if on_top => {
                if let Some(f) = self.overlays.form_mut() {
                    f.set_env_choice(c);
                }
            }
            Outcome::New if on_top => {
                let Some(mut back) = self.overlays.take_form() else { return };
                // As for a new tunnel: a running test stops, its result waits with the form.
                self.cancel_test();
                back.parked_test = self.conn_test.take();
                self.open_environment_form(None, false);
                if let Some(f) = self.overlays.form_mut() {
                    f.back_to = Some(back);
                }
            }
            _ => {}
        }
    }
}
