//! The connection profile form: a large centered dialog with section tabs (Basic /
//! Advanced), the password storage selector, the pickers of color, icon and folder, and the
//! inline test-connection result line.

use crate::app::action::Action;
use crate::app::profiles::{BUTTONS, DRIVERS, FIELDS, Field, Section, secret_field, source_choice};
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
    // Through a tunnel: one line per stage (the SSH hop, then the database).
    if let Some(stages) = app.test_lines() {
        let frame = app.conn_test.as_ref().map(|t| (t.started.elapsed().as_millis() / 100) as usize).unwrap_or(0);
        for (i, (text, level)) in stages.iter().take(lines).enumerate() {
            let (spin, color) = match level {
                Level::Info => (format!("{} ", SPINNER[frame % SPINNER.len()]), theme::FG),
                l => (String::new(), level_color(*l)),
            };
            let text = crate::text::clip(&format!("{spin}{text}"), w);
            put(buf, x, y + i as u16, &text, w, Style::new().fg(color).bg(theme::SURFACE));
        }
        return;
    }
    if let Some((text, level)) = app.test_status() {
        let (spin, color) = match level {
            Level::Info => {
                let frame =
                    app.conn_test.as_ref().map(|t| (t.started.elapsed().as_millis() / 100) as usize).unwrap_or(0);
                (format!("{} ", SPINNER[frame % SPINNER.len()]), theme::FG)
            }
            l => (String::new(), level_color(l)),
        };
        let wrapped = wrap(&format!("{spin}{text}"), w);
        let n = wrapped.len().min(lines);
        for (i, l) in wrapped.iter().take(n).enumerate() {
            // Clip (with …) only the last visible line.
            let l = if i + 1 == n && wrapped.len() > n { format!("{l}…") } else { l.clone() };
            put(buf, x, y + i as u16, &l, w, Style::new().fg(color).bg(theme::SURFACE));
        }
    }
}

/// One `‹ value ›` selector at (x, y); returns the columns used.
fn selector(buf: &mut Buffer, x: u16, y: u16, room: usize, parts: &[(String, Style)], focused: bool) -> u16 {
    let bg = if focused { theme::SELECTION_BG } else { theme::SURFACE_ALT };
    let mut cx = x;
    let mut all = vec![("‹ ".to_string(), Style::new().fg(theme::FG_MUTED))];
    all.extend(parts.iter().cloned());
    all.push((" ›".to_string(), Style::new().fg(theme::FG_MUTED)));
    for (t, st) in all {
        let left = room.saturating_sub((cx - x) as usize);
        cx += put(buf, cx, y, &t, left, st.bg(bg));
    }
    cx - x
}

/// The profile form: a large centered dialog with its section tabs (Basic / Advanced), the
/// fields of the current section, the buttons and the test-connection line. Returns the
/// hardware cursor (the focused text input).
pub(crate) fn draw_profile_form(app: &mut App, area: Rect, buf: &mut Buffer) -> Option<(u16, u16)> {
    let pick_key = app.key_for(Action::PickKeyFile, Ctx::ProfileForm);
    let i18n = &app.i18n;
    let profiles = &app.profiles;
    let icons_on = app.icons_on();
    let keychain_error = app
        .secrets
        .unavailable
        .as_ref()
        .map(|f| i18n.msg(&crate::app::fault_reason(f)).to_string())
        .unwrap_or_default();
    let form = app.overlays.form_mut()?;
    form.key_button = Rect::default();
    let title = match &form.original_name {
        Some(n) => i18n.msg(&Msg::FormTitleEdit { name: n.clone() }),
        None => i18n.label(Label::FormTitleNew),
    };
    let footer = i18n.label(Label::FormKeys);
    let w = area.width.saturating_sub(4).min(100);
    let rect = centered(area, w, 19);
    let inner = modal(rect, &title, &footer, buf);
    let iw = inner.width as usize;
    let label_w = FIELDS.iter().map(|f| width(&i18n.label(f.label()))).max().unwrap_or(8) + 2;
    let editing = form.editing;
    let errors = form.errors(|n| profiles.iter().enumerate().any(|(i, p)| p.name == n && Some(i) != editing));
    let dim = Style::new().fg(theme::FG_DIM).bg(theme::SURFACE);
    let mut cursor = None;
    // Inputs leave room for their hints (28 columns) and are at most 34 wide.
    let input_w = (iw.saturating_sub(label_w + 4 + 28) as u16).clamp(16, 34);
    let label_style = |focused: bool| {
        if focused {
            Style::new().fg(theme::ACCENT).bg(theme::SURFACE).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme::FG_MUTED).bg(theme::SURFACE)
        }
    };
    let right = inner.x + inner.width;
    let room_from = |x: u16| right.saturating_sub(x + 1) as usize;

    // Section tabs.
    let mut x = inner.x + 1;
    for sec in Section::ALL {
        let style = if sec == form.section {
            Style::new().fg(theme::BG).bg(theme::ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme::FG_MUTED).bg(theme::SURFACE_ALT)
        };
        x += put(buf, x, inner.y, &format!(" {} ", i18n.label(sec.label())), room_from(x), style) + 1;
    }
    let hint = i18n.label(Label::FormSectionsHint);
    let hx = right.saturating_sub(width(&hint) as u16 + 1).max(x + 1);
    put(buf, hx, inner.y, &hint, room_from(hx), dim);

    let rows: Vec<Option<Field>> = match form.section {
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
        Section::Advanced => {
            vec![
                Some(Field::SslMode),
                Some(Field::StatementCache),
                Some(Field::Policy),
                Some(Field::Color),
                Some(Field::Icon),
                Some(Field::Folder),
            ]
        }
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
        let label = match f {
            Field::SshSecret if form.ssh_auth == SshAuth::Password => Label::FormFieldSshPassword,
            f => f.label(),
        };
        put(buf, inner.x + 1, y, &i18n.label(label), label_w, label_style(focused));
        let field_bg = if focused { theme::SELECTION_BG } else { theme::SURFACE_ALT };
        let text = |t: &str| (t.to_string(), Style::new().fg(theme::FG));
        let after = match f {
            Field::Driver => {
                let mut cx = x;
                for (i, (_, name)) in DRIVERS.iter().enumerate() {
                    let (t, style) = if i == form.driver {
                        (format!("‹{name}›"), Style::new().fg(theme::FG).bg(field_bg).add_modifier(Modifier::BOLD))
                    } else if form.drivers_enabled[i] {
                        (format!(" {name} "), Style::new().fg(theme::FG_MUTED).bg(theme::SURFACE))
                    } else {
                        (format!(" {name} "), dim.add_modifier(Modifier::CROSSED_OUT))
                    };
                    cx += put(buf, cx, y, &t, room_from(cx), style);
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
                let used = selector(buf, x, y, room_from(x), &[text(SSL_MODES[form.sslmode])], focused);
                put(buf, x + used + 2, y, &i18n.label(Label::FormSslmodeHint), room_from(x + used + 2), dim);
                continue;
            }
            Field::SshEnabled => {
                let (state, hint) = match form.ssh_enabled {
                    true => (Label::FormSshOn, Label::FormSshOnHint),
                    false => (Label::FormSshOff, Label::FormSshOffHint),
                };
                let used = selector(buf, x, y, room_from(x), &[text(&i18n.label(state))], focused);
                let hint = format!("{} · {}", i18n.label(Label::FormSslmodeHint), i18n.label(hint));
                put(buf, x + used + 2, y, &hint, room_from(x + used + 2), dim);
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
                for (name, chosen) in names {
                    let text = if chosen { format!("‹{name}›") } else { format!(" {name} ") };
                    let style = if chosen && focused {
                        Style::new().fg(theme::BG).bg(theme::ACCENT).add_modifier(Modifier::BOLD)
                    } else if chosen {
                        Style::new().fg(theme::FG).bg(theme::SURFACE_ALT).add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().fg(theme::FG_MUTED).bg(theme::SURFACE)
                    };
                    cx += put(buf, cx, y, &text, room_from(cx), style);
                }
                continue;
            }
            Field::StatementCache => {
                let state = if form.statement_cache { Label::FormCacheOn } else { Label::FormCacheOff };
                let used = selector(buf, x, y, room_from(x), &[text(&i18n.label(state))], focused);
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
                x + selector(buf, x, y, room_from(x), &parts, focused)
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
                x + selector(buf, x, y, room_from(x), &[text(&format!("{cell}{name}"))], focused)
            }
            Field::Folder => {
                let name = match &form.folder {
                    Some(f) => format!("{f}/"),
                    None => i18n.label(Label::ChooserFolderTop).to_string(),
                };
                x + selector(buf, x, y, room_from(x), &[text(&name)], focused)
            }
            Field::Source => {
                let mut cx = x;
                for kind in SourceKind::ALL {
                    let name = i18n.label(source_choice(kind));
                    // The chosen one in ‹ › (like the SSL mode), so it shows without colors too.
                    let text = if kind == form.source { format!("‹{name}›") } else { format!(" {name} ") };
                    let style = if kind == form.source && focused {
                        Style::new().fg(theme::BG).bg(theme::ACCENT).add_modifier(Modifier::BOLD)
                    } else if kind == form.source {
                        Style::new().fg(theme::FG).bg(theme::SURFACE_ALT).add_modifier(Modifier::BOLD)
                    } else if kind == SourceKind::Keychain && !form.keychain_ok {
                        dim.add_modifier(Modifier::CROSSED_OUT)
                    } else {
                        Style::new().fg(theme::FG_MUTED).bg(theme::SURFACE)
                    };
                    cx += put(buf, cx, y, &text, room_from(cx), style);
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
                    Style::new().fg(theme::FG).bg(field_bg),
                    focused,
                    mask,
                    None,
                );
                if focused {
                    cursor = Some((cx, y));
                }
                if f == Field::SshKeyFile {
                    // The key file picker's button (`Ctrl+O` on the form too).
                    let bx = x + input_w + 1;
                    let used =
                        put(buf, bx, y, "[…]", room_from(bx), Style::new().fg(theme::ACCENT).bg(theme::SURFACE_ALT));
                    form.key_button = Rect::new(bx, y, used, 1);
                    bx + used - 1
                } else {
                    x + input_w
                }
            }
        };
        let after = after + 2;
        let room = room_from(after);
        if let Some((_, e)) = errors.iter().find(|(ef, _)| *ef == f) {
            put(buf, after, y, &i18n.label(e.label()), room, Style::new().fg(theme::ERROR).bg(theme::SURFACE));
        } else {
            let hint = match (f, form.source) {
                (Field::SshKeyFile, _) => Some(Msg::FormSshKeyHint { key: pick_key.clone() }),
                (Field::Password, SourceKind::File) => Some(Label::FormPasswordFile.into()),
                (Field::Password, _) if form.keychain_ok => Some(Label::FormPasswordKeychain.into()),
                (Field::Password, _) => Some(Label::FormPasswordPrompt.into()),
                (Field::Command, _) => Some(Label::FormCommandHint.into()),
                (Field::Env, _) => Some(Label::FormEnvHint.into()),
                (Field::Policy, _) => Some(Label::FormPolicyHint.into()),
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
    // What the key file picker said about the file it picked, below the fields.
    if form.section == Section::Ssh
        && let Some(note) = form.ssh_key_note.as_ref().filter(|_| form.ssh_fields().contains(&Field::SshKeyFile))
    {
        let text = i18n.msg(note);
        let style = Style::new().fg(theme::WARNING).bg(theme::SURFACE);
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
                Style::new().fg(theme::WARNING).bg(theme::SURFACE),
            );
        }
        // DSN (full width) + inline problem
        let y = inner.y + 11;
        let focused = form.focus == Field::Dsn;
        put(buf, inner.x + 1, y, &i18n.label(Field::Dsn.label()), label_w, label_style(focused));
        let x = inner.x + 1 + label_w as u16;
        let field_bg = if focused { theme::SELECTION_BG } else { theme::SURFACE_ALT };
        let hide = datarig_core::profile::dsn::secret_span(form.dsn.text());
        let dsn_w = inner.width.saturating_sub(label_w as u16 + 2);
        let cx = form.dsn.render(
            Rect::new(x, y, dsn_w, 1),
            buf,
            Style::new().fg(theme::FG).bg(field_bg),
            focused,
            false,
            hide,
        );
        if focused {
            cursor = Some((cx, y));
        }
        if let Some(p) = &form.dsn_problem {
            let detail = i18n.msg(&p.message());
            put(buf, x, y + 1, &detail, dsn_w as usize, Style::new().fg(theme::ERROR).bg(theme::SURFACE));
        }
    }
    // Buttons
    let y = inner.y + 14;
    let mut x = inner.x + 1 + label_w as u16;
    for b in BUTTONS {
        let style = if form.focus == b {
            Style::new().fg(theme::BG).bg(theme::ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme::FG).bg(theme::SURFACE_ALT)
        };
        let text = format!(" {} ", i18n.label(b.label()));
        x += put(buf, x, y, &text, room_from(x), style) + 2;
    }
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
