//! The connection profile form: a large centered dialog with section tabs (Basic /
//! Advanced), the password storage selector, the pickers of color, icon and folder, and the
//! inline test-connection result line. The SSH section's first row picks the tunnel: none, a
//! tunnel preset (shown, not edited, here), or one of this profile only (its fields, and a
//! button that makes a preset of them). The tunnel form is the same dialog with the SSH
//! section alone, the preset's name first.

use crate::app::action::Action;
use crate::app::profiles::{
    BUTTONS, DRIVERS, FIELDS, Field, FormHit, FormKind, Section, SshChoice, secret_field, source_choice,
};
use crate::app::{App, Level};
use crate::keymap::Ctx;
use crate::text::{width, wrap};
use crate::theme;
use crate::widgets::dialog::{centered, modal};
use crate::widgets::statusbar::level_color;
use crate::widgets::{SPINNER, put};
use datarig_core::i18n::{Label, Msg};
use datarig_core::profile::SSL_MODES;
use datarig_core::profile::ssh::SshAuth;
use datarig_core::secret::SourceKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

/// Test-connection state, wrapped onto at most `lines` rows.
pub(crate) fn test_line(app: &App, x: u16, y: u16, w: usize, lines: usize, buf: &mut Buffer) {
    let th = theme::cur();
    // Through a tunnel: one line per stage (the SSH hop, then the database).
    if let Some(stages) = app.test_lines() {
        let frame = app.conn_test.as_ref().map(|t| (t.started.elapsed().as_millis() / 100) as usize).unwrap_or(0);
        for (i, (text, level)) in stages.iter().take(lines).enumerate() {
            let (spin, color) = match level {
                Level::Info => (format!("{} ", SPINNER[frame % SPINNER.len()]), th.fg),
                l => (String::new(), level_color(*l)),
            };
            let text = crate::text::clip(&format!("{spin}{text}"), w);
            put(buf, x, y + i as u16, &text, w, Style::new().fg(color).bg(th.surface));
        }
        return;
    }
    if let Some((text, level)) = app.test_status() {
        let (spin, color) = match level {
            Level::Info => {
                let frame =
                    app.conn_test.as_ref().map(|t| (t.started.elapsed().as_millis() / 100) as usize).unwrap_or(0);
                (format!("{} ", SPINNER[frame % SPINNER.len()]), th.fg)
            }
            l => (String::new(), level_color(l)),
        };
        let wrapped = wrap(&format!("{spin}{text}"), w);
        let n = wrapped.len().min(lines);
        for (i, l) in wrapped.iter().take(n).enumerate() {
            // Clip (with …) only the last visible line.
            let l = if i + 1 == n && wrapped.len() > n { format!("{l}…") } else { l.clone() };
            put(buf, x, y + i as u16, &l, w, Style::new().fg(color).bg(th.surface));
        }
    }
}

/// Where a `‹ value ›` selector of field `f` drawn at (x, y), `used` columns wide, is clicked:
/// its `‹`, its value and its `›` (only when `whole`: a selector the box cut off ends in its
/// value).
fn selector_hits(hits: &mut Vec<(Rect, FormHit)>, f: Field, x: u16, y: u16, (used, whole): (u16, bool)) {
    let arrows = if whole { 4 } else { 2 };
    hits.push((Rect::new(x, y, used.min(2), 1), FormHit::Prev(f)));
    hits.push((Rect::new(x + 2, y, used.saturating_sub(arrows), 1), FormHit::Value(f)));
    if whole {
        hits.push((Rect::new((x + used).saturating_sub(2), y, used.min(2), 1), FormHit::Next(f)));
    }
}

/// The underline of the button under the pointer.
fn hovered(style: Style, on: bool) -> Style {
    if on { style.add_modifier(Modifier::UNDERLINED) } else { style }
}

/// One `‹ value ›` selector at (x, y); returns the columns used and whether all of it fit.
fn selector(buf: &mut Buffer, x: u16, y: u16, room: usize, parts: &[(String, Style)], focused: bool) -> (u16, bool) {
    let th = theme::cur();
    let bg = if focused { th.selection } else { Style::new().bg(th.surface_alt) };
    let mut cx = x;
    let mut all = vec![("‹ ".to_string(), Style::new().fg(th.fg_muted))];
    all.extend(parts.iter().cloned());
    all.push((" ›".to_string(), Style::new().fg(th.fg_muted)));
    let mut full = 0;
    for (t, st) in all {
        let left = room.saturating_sub((cx - x) as usize);
        cx += put(buf, cx, y, &t, left, st.patch(bg));
        full += width(&t);
    }
    (cx - x, usize::from(cx - x) == full)
}

/// The profile form: a large centered dialog with its section tabs (Basic / Advanced), the
/// fields of the current section, the buttons and the test-connection line. Returns the
/// hardware cursor (the focused text input).
pub(crate) fn draw_profile_form(app: &mut App, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let th = theme::cur();
    let pick_key = app.key_for(Action::PickKeyFile, Ctx::ProfileForm);
    let save_as_key = app.key_for(Action::SaveAsTunnel, Ctx::ProfileForm);
    let i18n = &app.i18n;
    let profiles = &app.profiles;
    let presets = &app.presets;
    let icons_on = app.icons_on();
    let keychain_error = app
        .secrets
        .unavailable
        .as_ref()
        .map(|f| i18n.msg(&crate::app::fault_reason(f)).to_string())
        .unwrap_or_default();
    let now = app.now();
    let top = app.overlays.top().map(|o| o.kind()) == Some(crate::app::overlay::OverlayKind::ProfileForm);
    let form = app.overlays.form_mut()?;
    form.press.drawn(top, now);
    let mut hits: Vec<(Rect, FormHit)> = Vec::new();
    let hover = form.hover;
    let tunnel_form = form.is_tunnel();
    let title = match (&form.original_name, tunnel_form) {
        (Some(n), false) => i18n.msg(&Msg::FormTitleEdit { name: n.clone() }),
        (None, false) => i18n.label(Label::FormTitleNew),
        (Some(n), true) => i18n.msg(&Msg::TunnelFormTitleEdit { name: n.clone() }),
        (None, true) => i18n.label(Label::TunnelFormTitleNew),
    };
    let footer = i18n.label(if tunnel_form { Label::TunnelFormKeys } else { Label::FormKeys });
    let w = area.width.saturating_sub(4).min(100);
    let rect = centered(area, w, 19);
    let inner = modal(rect, &title, &footer, buf);
    let iw = inner.width as usize;
    let label_w = FIELDS
        .iter()
        .map(|f| f.label())
        .chain([Label::FormFieldTunnelUsedBy])
        .map(|l| width(&i18n.label(l)))
        .max()
        .unwrap_or(8)
        + 2;
    let editing = form.editing;
    let errors = match form.kind {
        FormKind::Tunnel { id, .. } => {
            let original = form.original_name.clone();
            form.errors(|n| crate::app::presets::name_taken(presets, id, original.as_deref(), n))
        }
        FormKind::Profile => {
            form.errors(|n| profiles.iter().enumerate().any(|(i, p)| p.name == n && Some(i) != editing))
        }
    };
    let dim = Style::new().fg(th.fg_dim).bg(th.surface);
    let unencrypted = form.unencrypted();
    let mut cursor = None;
    // Inputs leave room for their hints (28 columns) and are at most 34 wide.
    let input_w = (iw.saturating_sub(label_w + 4 + 28) as u16).clamp(16, 34);
    let label_style = |focused: bool| {
        if focused {
            Style::new().fg(th.accent).bg(th.surface).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(th.fg_muted).bg(th.surface)
        }
    };
    let right = inner.x + inner.width;
    let room_from = |x: u16| right.saturating_sub(x + 1) as usize;

    // Section tabs (a tunnel form has one).
    let mut x = inner.x + 1;
    let tab = |sec: Section| match (sec, tunnel_form) {
        (Section::Ssh, true) => Label::TunnelFormSection,
        (s, _) => s.label(),
    };
    let sections: &[Section] = if tunnel_form { &[Section::Ssh] } else { &Section::ALL };
    for &sec in sections {
        let style = if sec == form.section {
            Style::new().fg(th.bg).bg(th.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(th.fg_muted).bg(th.surface_alt)
        };
        let used = put(buf, x, inner.y, &format!(" {} ", i18n.label(tab(sec))), room_from(x), style);
        hits.push((Rect::new(x, inner.y, used, 1), FormHit::Section(sec)));
        x += used + 1;
    }
    let hint = i18n.label(if tunnel_form { Label::TunnelFormHint } else { Label::FormSectionsHint });
    let hx = right.saturating_sub(width(&hint) as u16 + 1).max(x + 1);
    put(buf, hx, inner.y, &hint, room_from(hx), dim);

    let rows: Vec<Option<Field>> = match form.section {
        _ if tunnel_form => form.fields().into_iter().filter(|f| !f.is_button()).map(Some).collect(),
        Section::Basic => vec![
            Some(Field::Driver),
            Some(Field::Name),
            Some(Field::Host),
            Some(Field::Port),
            Some(Field::User),
            Some(Field::Source),
            secret_field(form.source),
            Some(Field::Database),
        ],
        Section::Ssh => form.ssh_fields().into_iter().map(Some).collect(),
        // The SSL mode and the statement cache are PostgreSQL's only, the server key file and key
        // retrieval MySQL's (`ProfileForm::fields`).
        Section::Advanced => form.fields().into_iter().filter(|f| !f.is_button()).map(Some).collect(),
    };
    for (row, f) in rows.iter().enumerate() {
        let y = inner.y + 2 + row as u16;
        let x = inner.x + 1 + label_w as u16;
        let Some(f) = *f else {
            // `prompt`: nothing to enter.
            put(buf, inner.x + 1, y, &i18n.label(Label::FormFieldPassword), label_w, label_style(false));
            put(buf, x, y, &i18n.label(Label::FormPromptHint), iw.saturating_sub(label_w + 2), dim);
            continue;
        };
        let focused = form.focus == f;
        hits.push((Rect::new(inner.x, y, inner.width, 1), FormHit::Field(f)));
        let label = match f {
            Field::SshSecret if form.ssh_auth == SshAuth::Password => Label::FormFieldSshPassword,
            f => f.label(),
        };
        put(buf, inner.x + 1, y, &i18n.label(label), label_w, label_style(focused));
        let field_bg = if focused { th.selection } else { Style::new().bg(th.surface_alt) };
        let text = |t: &str| (t.to_string(), Style::new().fg(th.fg));
        let after = match f {
            Field::Driver => {
                let mut cx = x;
                for (i, (_, name)) in DRIVERS.iter().enumerate() {
                    let (t, style) = if i == form.driver {
                        (format!("‹{name}›"), Style::new().fg(th.fg).patch(field_bg).add_modifier(Modifier::BOLD))
                    } else if form.drivers_enabled[i] {
                        (format!(" {name} "), Style::new().fg(th.fg_muted).bg(th.surface))
                    } else {
                        (format!(" {name} "), dim.add_modifier(Modifier::CROSSED_OUT))
                    };
                    let used = put(buf, cx, y, &t, room_from(cx), style);
                    hits.push((Rect::new(cx, y, used, 1), FormHit::Choice(f, i)));
                    cx += used;
                }
                if form.drivers_enabled.iter().any(|e| !e) {
                    // The short label where the full one does not fit (80 columns).
                    let hx = cx + 2;
                    let full = i18n.label(Label::FormDriverUnsupported);
                    let hint = if width(&full) <= room_from(hx) {
                        full
                    } else {
                        i18n.label(Label::FormDriverUnsupportedShort)
                    };
                    put(buf, hx, y, &hint, room_from(hx), dim);
                }
                continue;
            }
            Field::SslMode => {
                let drawn = selector(buf, x, y, room_from(x), &[text(SSL_MODES[form.sslmode])], focused);
                selector_hits(&mut hits, f, x, y, drawn);
                let used = drawn.0;
                put(buf, x + used + 2, y, &i18n.label(Label::FormSslmodeHint), room_from(x + used + 2), dim);
                continue;
            }
            Field::SshEnabled => {
                let choice = form.ssh_choice();
                let (value, hint) = match &choice {
                    SshChoice::Off => (i18n.label(Label::FormSshOff).to_string(), i18n.label(Label::FormSshOffHint)),
                    SshChoice::Inline => (i18n.label(Label::FormSshOwn).to_string(), i18n.label(Label::FormSshOnHint)),
                    SshChoice::Preset(name) if form.new_preset_picked() => (
                        i18n.msg(&Msg::FormSshPresetNew { name: name.clone() }).to_string(),
                        i18n.label(Label::FormSshPresetNewHint),
                    ),
                    SshChoice::Preset(name) if form.picked_preset().is_none() => (
                        i18n.msg(&Msg::FormSshPresetMissing { name: name.clone() }).to_string(),
                        i18n.label(Label::FormSshPresetMissingHint),
                    ),
                    SshChoice::Preset(name) => (name.clone(), i18n.label(Label::FormSshPresetHint)),
                };
                let drawn = selector(buf, x, y, room_from(x), &[text(&value)], focused);
                selector_hits(&mut hits, f, x, y, drawn);
                let used = drawn.0;
                let mut hx = x + used + 2;
                if choice == SshChoice::Inline {
                    // "Save as tunnel preset" (its key on the form too).
                    let label = i18n.msg(&Msg::FormSshSaveAsPreset { key: save_as_key.clone() }).to_string();
                    let b = put(
                        buf,
                        hx,
                        y,
                        &format!("[{label}]"),
                        room_from(hx),
                        hovered(Style::new().fg(th.accent).bg(th.surface_alt), hover == Some(FormHit::SaveAsPreset)),
                    );
                    hits.push((Rect::new(hx, y, b, 1), FormHit::SaveAsPreset));
                    hx += b + 2;
                }
                let hint = format!("{} · {hint}", i18n.label(Label::FormSslmodeHint));
                put(buf, hx, y, &hint, room_from(hx), dim);
                // A preset is shown here, not edited: its bastion, its login and who else uses it.
                if let SshChoice::Preset(name) = &choice {
                    let shown = form
                        .picked_preset()
                        .map(|p| p.settings.clone())
                        .or_else(|| form.new_preset_picked().then(|| form.ssh_settings()));
                    let mut lines: Vec<(Label, String, Style)> = Vec::new();
                    match &shown {
                        Some(s) => {
                            lines.push((
                                Label::FormFieldSshHost,
                                crate::app::App::bastion_text(s),
                                Style::new().fg(th.fg),
                            ));
                            let mut login = i18n.label(ssh_auth_choice(s.auth)).to_string();
                            if let (SshAuth::Key, Some(k)) = (s.auth, s.key_file.as_deref()) {
                                login.push_str(&format!(" · {k}"));
                            }
                            lines.push((Label::FormFieldSshAuth, login, Style::new().fg(th.fg)));
                        }
                        None => {
                            let text = i18n.msg(&Msg::FormSshPresetGone { name: name.clone() }).to_string();
                            lines.push((Label::FormFieldSshEnabled, text, Style::new().fg(th.error)));
                        }
                    }
                    let others: Vec<String> = profiles
                        .iter()
                        .filter(|p| p.tunnel.as_deref() == Some(name.as_str()) && p.id != form.profile_id())
                        .map(|p| p.name.clone())
                        .collect();
                    if !others.is_empty() {
                        lines.push((Label::FormFieldTunnelUsedBy, others.join(", "), Style::new().fg(th.fg_muted)));
                    }
                    for (i, (label, value, style)) in lines.into_iter().enumerate() {
                        let ly = y + 1 + i as u16;
                        put(buf, inner.x + 1, ly, &i18n.label(label), label_w, label_style(false));
                        put(buf, x, ly, &value, room_from(x), style.bg(th.surface));
                    }
                }
                continue;
            }
            Field::SshAuth | Field::SshSource => {
                let names: Vec<(String, bool)> = match f {
                    Field::SshAuth => SshAuth::ALL
                        .iter()
                        .map(|a| (i18n.label(ssh_auth_choice(*a)).to_string(), *a == form.ssh_auth))
                        .collect(),
                    _ => SourceKind::ALL
                        .iter()
                        .map(|k| (i18n.label(source_choice(*k)).to_string(), *k == form.ssh_source))
                        .collect(),
                };
                let mut cx = x;
                for (i, (name, chosen)) in names.into_iter().enumerate() {
                    let text = if chosen { format!("‹{name}›") } else { format!(" {name} ") };
                    let style = if chosen && focused {
                        Style::new().fg(th.bg).bg(th.accent).add_modifier(Modifier::BOLD)
                    } else if chosen {
                        Style::new().fg(th.fg).bg(th.surface_alt).add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().fg(th.fg_muted).bg(th.surface)
                    };
                    let used = put(buf, cx, y, &text, room_from(cx), style);
                    hits.push((Rect::new(cx, y, used, 1), FormHit::Choice(f, i)));
                    cx += used;
                }
                continue;
            }
            Field::KeyRetrieval => {
                let state = if form.key_retrieval { Label::FormKeyRetrievalOn } else { Label::FormKeyRetrievalOff };
                let drawn = selector(buf, x, y, room_from(x), &[text(&i18n.label(state))], focused);
                selector_hits(&mut hits, f, x, y, drawn);
                let used = drawn.0;
                let hint =
                    format!("{} · {}", i18n.label(Label::FormSslmodeHint), i18n.label(Label::FormKeyRetrievalHint));
                put(buf, x + used + 2, y, &hint, room_from(x + used + 2), dim);
                continue;
            }
            Field::StatementCache => {
                let state = if form.statement_cache { Label::FormCacheOn } else { Label::FormCacheOff };
                let drawn = selector(buf, x, y, room_from(x), &[text(&i18n.label(state))], focused);
                selector_hits(&mut hits, f, x, y, drawn);
                let used = drawn.0;
                let hint = format!("{} · {}", i18n.label(Label::FormSslmodeHint), i18n.label(Label::FormCacheHint));
                put(buf, x + used + 2, y, &hint, room_from(x + used + 2), dim);
                continue;
            }
            Field::Color => {
                let color = theme::profile_color(form.display_color());
                let name = match &form.color {
                    Some(c) => c.clone(),
                    None => i18n.label(Label::ChooserColorAuto).to_string(),
                };
                let parts = [("● ".to_string(), Style::new().fg(color)), text(&name)];
                let drawn = selector(buf, x, y, room_from(x), &parts, focused);
                selector_hits(&mut hits, f, x, y, drawn);
                let used = drawn.0;
                x + used
            }
            Field::Icon => {
                let name = match &form.icon {
                    Some(i) => i.clone(),
                    None => i18n.label(Label::ChooserIconDriver).to_string(),
                };
                let glyph = form
                    .icon
                    .as_deref()
                    .and_then(crate::icons::by_name)
                    .unwrap_or_else(|| crate::icons::for_driver(DRIVERS[form.driver].0));
                let cell = if icons_on { format!("{glyph} ") } else { String::new() };
                let drawn = selector(buf, x, y, room_from(x), &[text(&format!("{cell}{name}"))], focused);
                selector_hits(&mut hits, f, x, y, drawn);
                let used = drawn.0;
                x + used
            }
            Field::Folder => {
                let name = match &form.folder {
                    Some(f) => format!("{f}/"),
                    None => i18n.label(Label::ChooserFolderTop).to_string(),
                };
                let drawn = selector(buf, x, y, room_from(x), &[text(&name)], focused);
                selector_hits(&mut hits, f, x, y, drawn);
                let used = drawn.0;
                x + used
            }
            Field::Source => {
                let mut cx = x;
                for (i, kind) in SourceKind::ALL.into_iter().enumerate() {
                    let name = i18n.label(source_choice(kind));
                    // The chosen one in ‹ › (like the SSL mode), so it shows without colors too.
                    let text = if kind == form.source { format!("‹{name}›") } else { format!(" {name} ") };
                    let style = if kind == form.source && focused {
                        Style::new().fg(th.bg).bg(th.accent).add_modifier(Modifier::BOLD)
                    } else if kind == form.source {
                        Style::new().fg(th.fg).bg(th.surface_alt).add_modifier(Modifier::BOLD)
                    } else if kind == SourceKind::Keychain && !form.keychain_ok {
                        dim.add_modifier(Modifier::CROSSED_OUT)
                    } else {
                        Style::new().fg(th.fg_muted).bg(th.surface)
                    };
                    let used = put(buf, cx, y, &text, room_from(cx), style);
                    hits.push((Rect::new(cx, y, used, 1), FormHit::Choice(f, i)));
                    cx += used;
                }
                put(buf, cx + 2, y, &i18n.label(Label::FormSslmodeHint), room_from(cx + 2), dim);
                continue;
            }
            _ => {
                let mask = matches!(f, Field::Password | Field::SshSecret);
                let inp = form.input_mut(f)?;
                let cx = inp.render(
                    Rect::new(x, y, input_w, 1),
                    buf,
                    Style::new().fg(th.fg).patch(field_bg),
                    focused,
                    mask,
                    None,
                );
                if focused {
                    cursor = Some((cx, y));
                }
                hits.push((Rect::new(x, y, input_w, 1), FormHit::Input(f)));
                if f == Field::SshKeyFile {
                    // The key file picker's button (`Ctrl+O` on the form too).
                    let bx = x + input_w + 1;
                    let style = hovered(Style::new().fg(th.accent).bg(th.surface_alt), hover == Some(FormHit::KeyFile));
                    let used = put(buf, bx, y, "[…]", room_from(bx), style);
                    hits.push((Rect::new(bx, y, used, 1), FormHit::KeyFile));
                    bx + used - 1
                } else {
                    x + input_w
                }
            }
        };
        let after = after + 2;
        let room = room_from(after);
        if let Some((_, e)) = errors.iter().find(|(ef, _)| *ef == f) {
            put(buf, after, y, &i18n.label(e.label()), room, Style::new().fg(th.error).bg(th.surface));
        } else if f == Field::Host && unencrypted {
            let warning = Style::new().fg(th.warning).bg(th.surface);
            put(buf, after, y, &i18n.label(Label::FormUnencryptedShort), room, warning);
        } else {
            let hint = match (f, form.source) {
                (Field::SshKeyFile, _) => Some(Msg::FormSshKeyHint { key: pick_key.clone() }),
                (Field::Password, SourceKind::File) => Some(Label::FormPasswordFile.into()),
                (Field::Password, _) if form.keychain_ok => Some(Label::FormPasswordKeychain.into()),
                (Field::Password, _) => Some(Label::FormPasswordPrompt.into()),
                (Field::Command, _) => Some(Label::FormCommandHint.into()),
                (Field::Env, _) => Some(Label::FormEnvHint.into()),
                (Field::Policy, _) => Some(Label::FormPolicyHint.into()),
                (Field::ServerKey, _) => Some(Label::FormServerKeyHint.into()),
                (Field::Color | Field::Icon | Field::Folder, _) => Some(Label::FormPickerHint.into()),
                (Field::SshSecret, _) => Some(Label::FormSshSecretHint.into()),
                (Field::SshCommand, _) => Some(Label::FormCommandHint.into()),
                (Field::SshEnv, _) => Some(Label::FormEnvHint.into()),
                (Field::SshKeepalive | Field::SshTimeout, _) => Some(Label::FormSshSecondsHint.into()),
                _ => None,
            };
            if let Some(hint) = hint {
                put(buf, after, y, &i18n.msg(&hint), room, dim);
            }
        }
    }
    // A MySQL connection without a tunnel to another machine: not encrypted.
    if form.section == Section::Ssh && unencrypted {
        let text = i18n.label(Label::FormUnencrypted);
        let style = Style::new().fg(th.warning).bg(th.surface);
        for (i, l) in crate::text::wrap_words(&text, iw.saturating_sub(2)).iter().take(3).enumerate() {
            put(buf, inner.x + 1, inner.y + 4 + i as u16, l, iw.saturating_sub(2), style);
        }
    }
    // The profile named a preset and had its own tunnel on: which one the form keeps.
    if form.section == Section::Ssh && form.ssh_both {
        let text = i18n.label(Label::FormSshBoth);
        let style = Style::new().fg(th.warning).bg(th.surface);
        for (i, l) in wrap(&text, iw.saturating_sub(2)).iter().take(2).enumerate() {
            put(buf, inner.x + 1, inner.y + 12 + i as u16, l, iw.saturating_sub(2), style);
        }
    }
    // A tunnel form's profiles, below its fields.
    if let FormKind::Tunnel { editing: true, .. } = form.kind
        && let Some(name) = form.original_name.as_ref()
        && form.ssh_key_note.is_none()
    {
        let users: Vec<String> =
            profiles.iter().filter(|p| p.tunnel.as_deref() == Some(name.as_str())).map(|p| p.name.clone()).collect();
        if !users.is_empty() {
            let y = inner.y + 13;
            put(buf, inner.x + 1, y, &i18n.label(Label::FormFieldTunnelUsedBy), label_w, label_style(false));
            put(buf, inner.x + 1 + label_w as u16, y, &users.join(", "), iw.saturating_sub(label_w + 2), dim);
        }
    }
    // What the key file picker said about the file it picked, below the fields.
    if form.section == Section::Ssh
        && let Some(note) = form.ssh_key_note.as_ref().filter(|_| form.ssh_fields().contains(&Field::SshKeyFile))
    {
        let text = i18n.msg(note);
        let style = Style::new().fg(th.warning).bg(th.surface);
        for (i, l) in wrap(&text, iw.saturating_sub(2)).iter().take(2).enumerate() {
            put(buf, inner.x + 1, inner.y + 12 + i as u16, l, iw.saturating_sub(2), style);
        }
    }
    if form.section == Section::Basic {
        // Why the keychain cannot be picked.
        if !form.keychain_ok {
            let note = i18n.msg(&Msg::FormSourceKeychainUnavailable { error: keychain_error });
            put(
                buf,
                inner.x + 1,
                inner.y + 10,
                &note,
                iw.saturating_sub(2),
                Style::new().fg(th.warning).bg(th.surface),
            );
        }
        // DSN (full width) + inline problem
        let y = inner.y + 11;
        let focused = form.focus == Field::Dsn;
        hits.push((Rect::new(inner.x, y, inner.width, 1), FormHit::Field(Field::Dsn)));
        put(buf, inner.x + 1, y, &i18n.label(Field::Dsn.label()), label_w, label_style(focused));
        let x = inner.x + 1 + label_w as u16;
        let field_bg = if focused { th.selection } else { Style::new().bg(th.surface_alt) };
        let hide = datarig_core::profile::dsn::secret_span(form.dsn.text());
        let dsn_w = inner.width.saturating_sub(label_w as u16 + 2);
        let cx = form.dsn.render(
            Rect::new(x, y, dsn_w, 1),
            buf,
            Style::new().fg(th.fg).patch(field_bg),
            focused,
            false,
            hide,
        );
        if focused {
            cursor = Some((cx, y));
        }
        hits.push((Rect::new(x, y, dsn_w, 1), FormHit::Input(Field::Dsn)));
        if let Some(p) = &form.dsn_problem {
            let detail = i18n.msg(&p.message());
            put(buf, x, y + 1, &detail, dsn_w as usize, Style::new().fg(th.error).bg(th.surface));
        }
    }
    // Buttons
    let y = inner.y + 14;
    let mut x = inner.x + 1 + label_w as u16;
    for b in BUTTONS {
        let on = hover == Some(FormHit::Button(b));
        let style = if form.focus == b {
            Style::new().fg(th.bg).bg(th.accent).add_modifier(Modifier::BOLD)
        } else if on {
            Style::new().fg(th.accent).bg(th.surface_alt)
        } else {
            Style::new().fg(th.fg).bg(th.surface_alt)
        };
        let text = format!(" {} ", i18n.label(b.label()));
        let used = put(buf, x, y, &text, room_from(x), hovered(style, on));
        hits.push((Rect::new(x, y, used, 1), FormHit::Button(b)));
        x += used + 2;
    }
    // Only what is inside the box can be clicked (a small screen cuts it).
    hits.retain_mut(|(r, _)| {
        *r = r.intersection(inner);
        !r.is_empty()
    });
    form.hits = hits;
    test_line(app, inner.x + 1, inner.y + 15, iw.saturating_sub(2), 2, buf);
    cursor
}

/// The name of a way to log in to the bastion, in its selector.
fn ssh_auth_choice(a: SshAuth) -> Label {
    match a {
        SshAuth::Key => Label::FormSshAuthKey,
        SshAuth::Password => Label::FormSshAuthPassword,
        SshAuth::Agent => Label::FormSshAuthAgent,
        SshAuth::KeyboardInteractive => Label::FormSshAuthKeyboardInteractive,
    }
}
