//! The connection profile form: a large centered dialog with section tabs (Basic / SSH /
//! Advanced), the fields with a few fixed values (the driver, the SSL mode, the password's
//! storage, the bastion login and its secret's storage), each picked from a list (`Enter`, or a
//! click on it), the pickers of color, icon and folder, and the inline test-connection result
//! line. The SSH section's first row picks the tunnel from a list too: none, a tunnel preset
//! (shown, not edited, here), or one of this profile only (its fields, and a button that makes
//! a preset of them). The tunnel form is the same dialog with the SSH section alone, the
//! preset's name first. The Advanced section's environment is picked from a list too (its
//! color's dot, its name and badge shown). The environment form is the same dialog with an
//! environment's fields: its name, color, badge and rules. The Startup SQL form has its name,
//! its database and its statements, a list of one statement per row ([`statement_list`]), each
//! row that is not allowed, or that a read-only environment would refuse, saying why under it. A
//! profile's Startup SQL (its environment's, none, or a preset of its database) and an
//! environment's, for each database, are picked from a list; a profile's own startup SQL is the
//! same list (what is not allowed only), its row as tall as the screen leaves room for.

use crate::app::action::Action;
use crate::app::environments::{RuleTone, rules_tones};
use crate::app::profiles::{
    BUTTONS, COMMITS, CONFIRMS, DRIVERS, ENVIRONMENT, EnvChoice, FIELDS, Field, FormHit, FormKind, PAGINGS, SESSION,
    Section, SshChoice, secret_field, source_choice, ssh_auth_choice,
};
use crate::app::{App, Level};
use crate::keymap::Ctx;
use crate::text::{width, wrap};
use crate::theme;
use crate::widgets::dialog::{centered, modal};
use crate::widgets::statement_list::{Note, StatementList};
use crate::widgets::statusbar::level_color;
use crate::widgets::{SPINNER, put};
use datarig_core::i18n::{I18n, Label, Msg};
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

/// The background of the button under the pointer.
fn hovered(style: Style, on: bool) -> Style {
    if on { style.patch(crate::widgets::hover_style()) } else { style }
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

/// A value picked from a list (`Enter`, or a click on it) at (x, y): the value and a mark that
/// it drops down, then `hint` dimmed. The value is clicked as field `f`'s.
#[allow(clippy::too_many_arguments)]
fn drop_down(
    buf: &mut Buffer,
    hits: &mut Vec<(Rect, FormHit)>,
    f: Field,
    (x, y): (u16, u16),
    right: u16,
    value: &str,
    hint: &str,
    focused: bool,
) {
    let th = theme::cur();
    let bg = if focused { th.selection } else { Style::new().bg(th.surface_alt) };
    let room = |x: u16| right.saturating_sub(x + 1) as usize;
    let used = put(buf, x, y, value, room(x), Style::new().fg(th.fg).patch(bg));
    let used = used + put(buf, x + used, y, " ▾ ", room(x + used), Style::new().fg(th.fg_muted).patch(bg));
    hits.push((Rect::new(x, y, used, 1), FormHit::Value(f)));
    put(buf, x + used + 2, y, hint, room(x + used + 2), Style::new().fg(th.fg_dim).bg(th.surface));
}

/// A Startup SQL list in `area`: a row per statement (its number, its input, a `×` that removes
/// it) with its note under it, then "+ Add statement" and `hint`. A list longer than the area
/// scrolls (the focused row kept in view unless the wheel moved it), and the lines out of view
/// are counted on its first and last line, at their right end. Returns the hardware cursor (the
/// focused row's).
fn statement_list(
    buf: &mut Buffer,
    hits: &mut Vec<(Rect, FormHit)>,
    list: &mut StatementList,
    area: Rect,
    notes: &[Option<(String, Level)>],
    (i18n, hint): (&I18n, &str),
    (focused, hover): (bool, Option<FormHit>),
) -> Option<(u16, u16)> {
    let th = theme::cur();
    let dim = Style::new().fg(th.fg_dim).bg(th.surface);
    // Each line: a row, the note of a row, or the add line.
    let mut lines: Vec<(usize, bool)> = Vec::new();
    for i in 0..list.row_count() {
        lines.push((i, false));
        if notes.get(i).is_some_and(Option::is_some) {
            lines.push((i, true));
        }
    }
    lines.push((usize::MAX, false));
    // Longer than the area (of three lines at least): its first and last line count the lines
    // out of view.
    let marked = lines.len() > usize::from(area.height) && area.height >= 3;
    let (top, h) = if marked { (1, usize::from(area.height) - 2) } else { (0, usize::from(area.height)) };
    let max_scroll = lines.len().saturating_sub(h);
    if list.follow {
        let first = lines.iter().position(|(i, _)| *i == list.row).unwrap_or(0);
        // The last row keeps the add line in view too.
        let last = match list.row + 1 == list.row_count() {
            true => lines.len() - 1,
            false => lines.iter().rposition(|(i, _)| *i == list.row).unwrap_or(first),
        };
        if first < list.scroll {
            list.scroll = first;
        } else if last >= list.scroll + h {
            list.scroll = last + 1 - h.min(last + 1);
        }
    }
    list.scroll = list.scroll.min(max_scroll);
    let num_w: u16 = 4;
    let input_x = area.x + num_w;
    let input_w = area.width.saturating_sub(num_w + 2);
    let right = area.x + area.width;
    let mut cursor = None;
    for (k, &(i, note)) in lines.iter().skip(list.scroll).take(h).enumerate() {
        let y = area.y + top + k as u16;
        if i == usize::MAX {
            let add = i18n.label(Label::FormStatementAdd);
            let on = hover == Some(FormHit::AddStatement);
            // Under the numbers, so that the hint has room.
            let ax = area.x + 2;
            let style = hovered(Style::new().fg(th.accent).bg(th.surface), on);
            let used = put(buf, ax, y, &add, usize::from(right.saturating_sub(ax)), style);
            hits.push((Rect::new(ax, y, used, 1), FormHit::AddStatement));
            let hx = ax + used + 2;
            put(buf, hx, y, hint, usize::from(right.saturating_sub(hx)), dim);
            continue;
        }
        if note {
            if let Some(Some((text, level))) = notes.get(i) {
                let w = usize::from(right - input_x);
                let text = crate::text::clip(text, w);
                put(buf, input_x, y, &text, w, Style::new().fg(level_color(*level)).bg(th.surface));
            }
            continue;
        }
        let on_row = focused && i == list.row;
        let number = Style::new().fg(if on_row { th.accent } else { th.fg_dim }).bg(th.surface);
        put(buf, area.x, y, &format!("{:>3} ", i + 1), usize::from(num_w), number);
        let field_bg = if on_row { th.selection } else { Style::new().bg(th.surface_alt) };
        let empty = list.text(i).is_empty();
        let Some(input) = list.input_mut(i) else { continue };
        let cx = input.render(
            Rect::new(input_x, y, input_w, 1),
            buf,
            Style::new().fg(th.fg).patch(field_bg),
            on_row,
            false,
            None,
        );
        if on_row {
            cursor = Some((cx, y));
        } else if empty {
            let placeholder = i18n.label(Label::FormStatementPlaceholder);
            put(buf, input_x, y, &placeholder, usize::from(input_w), Style::new().fg(th.fg_dim).patch(field_bg));
        }
        hits.push((Rect::new(input_x, y, input_w, 1), FormHit::Statement(i)));
        let on = hover == Some(FormHit::RemoveStatement(i));
        let bx = input_x + input_w + 1;
        let used = put(buf, bx, y, "×", usize::from(right.saturating_sub(bx)), hovered(dim, on));
        hits.push((Rect::new(bx, y, used, 1), FormHit::RemoveStatement(i)));
    }
    // The lines out of view, at the right end of the first and the last line.
    let below = lines.len().saturating_sub(list.scroll + h);
    for (count, y, up) in [(list.scroll, area.y, true), (below, area.y + area.height - 1, false)] {
        if !marked || count == 0 {
            continue;
        }
        let count = count as u64;
        let m = if up { Msg::SessionViewMoreAbove { count } } else { Msg::SessionViewMoreBelow { count } };
        let text = i18n.msg(&m).to_string();
        let x = right.saturating_sub(width(&text) as u16).max(input_x);
        put(buf, x, y, &text, usize::from(right - x), dim);
    }
    cursor
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
    let environments = app.policies.environments();
    let app_default_rules = app.policies.default_rules();
    let icons_on = app.icons_on();
    let keychain_error = app
        .secrets
        .unavailable
        .as_ref()
        .map(|f| i18n.msg(&crate::app::fault_reason(f)).to_string())
        .unwrap_or_default();
    let now = app.now();
    let top = app.overlays.top().map(|o| o.kind()) == Some(crate::app::overlay::OverlayKind::ProfileForm);
    // A Startup SQL form's profiles and what its list says under each statement (a profile's own
    // list: only what is not allowed), read before the form is borrowed for drawing.
    let (session_users, statement_notes) = match app.overlays.form() {
        Some(f) if f.kind == FormKind::Profile || f.is_session() => {
            let users: Vec<String> = f
                .original_name
                .as_deref()
                .filter(|_| f.is_session())
                .map(|n| app.session_users(n).iter().map(|c| c.name.clone()).collect())
                .unwrap_or_default();
            let list = &f.session_sql;
            let notes: Vec<Option<(String, Level)>> = (0..list.row_count())
                .map(|i| match list.note(i) {
                    Some(Note::Refused(p)) => Some((app.set_problem_text(p), Level::Error)),
                    Some(Note::ReadOnly(b)) if f.is_session() => {
                        let what = i18n.label(crate::app::safety::blocked_label(*b)).to_string();
                        Some((i18n.msg(&Msg::FormStatementReadOnly { what }).to_string(), Level::Warning))
                    }
                    _ => None,
                })
                .collect();
            (users, notes)
        }
        _ => (Vec::new(), Vec::new()),
    };
    // The list's lines: its rows, the notes under them and the line that adds one.
    let list_lines = statement_notes.len() + statement_notes.iter().flatten().count() + 1;
    let session_presets = &app.session_presets;
    let default_session = app.policies.default_rules().session.clone();
    let form = app.overlays.form_mut()?;
    form.press.drawn(top, now);
    let mut hits: Vec<(Rect, FormHit)> = Vec::new();
    let hover = form.hover;
    let tunnel_form = form.is_tunnel();
    let env_form = form.is_environment();
    let session_form = form.is_session();
    let title = match (&form.original_name, tunnel_form, env_form) {
        _ if form.is_default_rules() => i18n.label(Label::EnvFormTitleDefault),
        (Some(n), _, _) if session_form => i18n.msg(&Msg::SessionFormTitleEdit { name: n.clone() }),
        (None, _, _) if session_form => i18n.label(Label::SessionFormTitleNew),
        (Some(n), _, true) => i18n.msg(&Msg::EnvFormTitleEdit { name: n.clone() }),
        (None, _, true) => i18n.label(Label::EnvFormTitleNew),
        (Some(n), false, _) => i18n.msg(&Msg::FormTitleEdit { name: n.clone() }),
        (None, false, _) => i18n.label(Label::FormTitleNew),
        (Some(n), true, _) => i18n.msg(&Msg::TunnelFormTitleEdit { name: n.clone() }),
        (None, true, _) => i18n.label(Label::TunnelFormTitleNew),
    };
    let footer = i18n.label(match (tunnel_form, env_form) {
        _ if session_form => Label::SessionFormKeys,
        (_, true) => Label::EnvFormKeys,
        (true, _) => Label::TunnelFormKeys,
        _ => Label::FormKeys,
    });
    let w = area.width.saturating_sub(4).min(100);
    // An environment's fields, then a blank line and the profiles that use it, then the
    // buttons: below the other forms' rows when there are more of them.
    let env_rows = if env_form { form.fields().iter().filter(|f| !f.is_button()).count() as u16 } else { 0 };
    // A Startup SQL form: its name and database, its statements (at least three lines, as many
    // as they take when the screen has room), a blank line, its profiles.
    let preset_list_h = list_lines.max(3).min(usize::from(area.height.saturating_sub(12)).max(1)) as u16;
    let used_by_row = if session_form { 4 + preset_list_h + 1 } else { (env_rows + 3).max(11) };
    // A profile's Advanced section of many rows (MySQL's) keeps a blank line above the buttons;
    // its own statements take the lines the screen leaves (one at least).
    let adv_fields = match (form.kind, form.section) {
        (FormKind::Profile, Section::Advanced) => form.fields().iter().filter(|f| !f.is_button()).count(),
        _ => 0,
    };
    let list_h = if session_form {
        preset_list_h
    } else {
        list_lines.min(usize::from(area.height).saturating_sub(7 + adv_fields).max(1)) as u16
    };
    let adv_rows = match adv_fields {
        0 => 0,
        n if form.fields().contains(&Field::SessionSql) => n as u16 + list_h - 1,
        n => n as u16,
    };
    let buttons_row = if session_form { used_by_row + 2 } else { (used_by_row + 2).max(14).max(adv_rows + 3) };
    let rect = centered(area, w, buttons_row + 5);
    let inner = modal(rect, &title, &footer, buf);
    let iw = inner.width as usize;
    // The environment form has its own fields (its labels set its column).
    let labelled: &[Field] = if env_form {
        &ENVIRONMENT
    } else if session_form {
        &SESSION
    } else {
        &FIELDS
    };
    let label_w = labelled
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
            let parked = form.parked_preset().map(str::to_string);
            form.errors(|n| {
                crate::app::presets::name_taken(presets, id, original.as_deref(), n)
                    || crate::app::presets::parked_taken(parked.as_deref(), n)
            })
        }
        FormKind::DefaultRules => form.errors(|_| false),
        FormKind::Environment { id, .. } => {
            let original = form.original_name.clone();
            form.errors(|n| crate::app::environments::name_taken(environments, id, original.as_deref(), n))
        }
        FormKind::Session { id, .. } => {
            let original = form.original_name.clone();
            form.errors(|n| crate::app::sessions::name_taken(session_presets, id, original.as_deref(), n))
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

    // Section tabs (a tunnel form and an environment form have one).
    let mut x = inner.x + 1;
    let tab = |sec: Section| match (sec, tunnel_form) {
        _ if session_form => Label::SessionFormSection,
        _ if env_form => Label::EnvFormSection,
        (Section::Ssh, true) => Label::TunnelFormSection,
        (s, _) => s.label(),
    };
    let sections: &[Section] = match (tunnel_form, env_form) {
        _ if session_form => &[Section::Advanced],
        (_, true) => &[Section::Advanced],
        (true, _) => &[Section::Ssh],
        _ => &Section::ALL,
    };
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
    let hint = i18n.label(match (tunnel_form, env_form) {
        _ if session_form => Label::SessionFormHint,
        _ if form.is_default_rules() => Label::EnvFormDefaultHint,
        (_, true) => Label::EnvFormHint,
        (true, _) => Label::TunnelFormHint,
        _ => Label::FormSectionsHint,
    });
    let hx = right.saturating_sub(width(&hint) as u16 + 1).max(x + 1);
    put(buf, hx, inner.y, &hint, room_from(hx), dim);

    let rows: Vec<Option<Field>> = match form.section {
        _ if tunnel_form || env_form || session_form => {
            form.fields().into_iter().filter(|f| !f.is_button()).map(Some).collect()
        }
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
    // The lines a Startup SQL list took above the row drawn, past its first.
    let mut taller = 0;
    for (row, f) in rows.iter().enumerate() {
        let y = inner.y + 2 + row as u16 + taller;
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
            Field::SessionSql if !session_form => Label::FormFieldProfileSessionSql,
            f => f.label(),
        };
        put(buf, inner.x + 1, y, &i18n.label(label), label_w, label_style(focused));
        let field_bg = if focused { th.selection } else { Style::new().bg(th.surface_alt) };
        let text = |t: &str| (t.to_string(), Style::new().fg(th.fg));
        let after = match f {
            Field::Driver | Field::SslMode | Field::Source | Field::SshAuth | Field::SshSource | Field::SessionDbms => {
                let value = match f {
                    Field::Driver => DRIVERS[form.driver].1.to_string(),
                    Field::SessionDbms => form.session_dbms.display().to_string(),
                    Field::SslMode => form.tls_mode().name(form.scheme()).to_string(),
                    Field::Source => i18n.label(source_choice(form.source)).to_string(),
                    Field::SshSource => i18n.label(source_choice(form.ssh_source)).to_string(),
                    _ => i18n.label(ssh_auth_choice(form.ssh_auth)).to_string(),
                };
                let hint = i18n.label(Label::FormPickerHint);
                drop_down(buf, &mut hits, f, (x, y), right, &value, &hint, focused);
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
                // A list to open (`Enter`), not values to step through: the value and a mark
                // that it drops down.
                let used = put(buf, x, y, &value, room_from(x), Style::new().fg(th.fg).patch(field_bg));
                let used = used
                    + put(buf, x + used, y, " ▾ ", room_from(x + used), Style::new().fg(th.fg_muted).patch(field_bg));
                hits.push((Rect::new(x, y, used, 1), FormHit::Value(f)));
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
                let hint = format!("{} · {hint}", i18n.label(Label::FormSshListHint));
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
                let (dot, name) = match &form.color {
                    Some(c) => (("● ".to_string(), Style::new().fg(color)), c.clone()),
                    // An environment without a color has none.
                    None if env_form => (
                        ("○ ".to_string(), Style::new().fg(th.fg_muted)),
                        i18n.label(Label::ChooserColorNone).to_string(),
                    ),
                    None => {
                        (("● ".to_string(), Style::new().fg(color)), i18n.label(Label::ChooserColorAuto).to_string())
                    }
                };
                let parts = [dot, text(&name)];
                let drawn = selector(buf, x, y, room_from(x), &parts, focused);
                selector_hits(&mut hits, f, x, y, drawn);
                let used = drawn.0;
                x + used
            }
            Field::Environment => {
                // A list to open (`Enter`), as the SSH tunnel: the value and a mark that it drops
                // down, then what its rules are.
                let choice = form.env_choice();
                let picked = form.picked_environment().cloned();
                let mut parts: Vec<(String, Style)> = Vec::new();
                let rules = match (&choice, &picked) {
                    (EnvChoice::Default, _) => {
                        parts.push((i18n.label(Label::EnvPickerDefault).to_string(), Style::new().fg(th.fg)));
                        rules_tones(i18n, app_default_rules, icons_on)
                    }
                    (EnvChoice::Named(name) | EnvChoice::Preset(name), Some(e)) => {
                        // Its chip in its color (circles with icons off).
                        let (on, off) = if icons_on {
                            (crate::icons::ENVIRONMENT_CHIP, crate::icons::ENVIRONMENT_CHIP)
                        } else {
                            ("●", "○")
                        };
                        let dot = e.display_color().map_or((format!("{off} "), th.fg_muted), |pc| {
                            (format!("{on} "), theme::profile_color(pc))
                        });
                        parts.push((dot.0, Style::new().fg(dot.1)));
                        let shown = match choice {
                            EnvChoice::Preset(_) => i18n.msg(&Msg::FormEnvPresetNew { name: name.clone() }).to_string(),
                            _ => name.clone(),
                        };
                        parts.push((shown, Style::new().fg(th.fg)));
                        if let Some(chip) = crate::app::App::badge_chip(e) {
                            parts.push((" ".to_string(), Style::new()));
                            parts.push(chip);
                        }
                        rules_tones(i18n, &e.policy, icons_on)
                    }
                    (EnvChoice::Named(name) | EnvChoice::Preset(name), None) => {
                        let missing = i18n.msg(&Msg::FormEnvMissing { name: name.clone() }).to_string();
                        parts.push((missing, Style::new().fg(th.error)));
                        vec![(i18n.label(Label::FormEnvMissingHint).to_string(), RuleTone::Dim)]
                    }
                };
                let mut used = 0;
                for (t, st) in &parts {
                    let st = if st.bg.is_some() { *st } else { st.patch(field_bg) };
                    used += put(buf, x + used, y, t, room_from(x + used), st);
                }
                used += put(buf, x + used, y, " ▾ ", room_from(x + used), Style::new().fg(th.fg_muted).patch(field_bg));
                hits.push((Rect::new(x, y, used, 1), FormHit::Value(f)));
                let hx = x + used + 2;
                // What it is, then its rules, each part in its tone.
                let what = match &choice {
                    EnvChoice::Preset(_) => i18n.label(Label::FormEnvPresetNewHint),
                    _ => i18n.label(Label::FormSshListHint),
                };
                let mut hx = hx + put(buf, hx, y, &format!("{what} · "), room_from(hx), dim);
                for (t, tone) in &rules {
                    hx += put(buf, hx, y, t, room_from(hx), dim.patch(tone.style()));
                }
                continue;
            }
            Field::EnvReadOnly => {
                let state = if form.env_read_only { Label::FormCacheOn } else { Label::FormCacheOff };
                let drawn = selector(buf, x, y, room_from(x), &[text(&i18n.label(state))], focused);
                selector_hits(&mut hits, f, x, y, drawn);
                let used = drawn.0;
                let hint =
                    format!("{} · {}", i18n.label(Label::FormSslmodeHint), i18n.label(Label::FormEnvReadOnlyHint));
                put(buf, x + used + 2, y, &hint, room_from(x + used + 2), dim);
                continue;
            }
            Field::EnvConfirm | Field::EnvPaging | Field::EnvCommit => {
                let names: Vec<(String, bool)> = match f {
                    Field::EnvCommit => COMMITS
                        .iter()
                        .map(|c| {
                            let l = if c.manual() { Label::FormEnvCommitManual } else { Label::FormEnvCommitAuto };
                            (i18n.label(l).to_string(), *c == form.env_commit)
                        })
                        .collect(),
                    Field::EnvConfirm => CONFIRMS
                        .iter()
                        .map(|c| {
                            let l =
                                if c.writes() { Label::FormEnvConfirmWrites } else { Label::FormEnvConfirmDestructive };
                            (i18n.label(l).to_string(), *c == form.env_confirm)
                        })
                        .collect(),
                    _ => PAGINGS
                        .iter()
                        .map(|p| {
                            let l = match p {
                                datarig_core::driver::PagingMode::Hold => Label::FormEnvPagingHold,
                                datarig_core::driver::PagingMode::NoHold => Label::FormEnvPagingNoHold,
                            };
                            (i18n.label(l).to_string(), *p == form.env_paging)
                        })
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
                let hint = i18n.label(match f {
                    Field::EnvPaging => Label::FormEnvPagingHint,
                    Field::EnvCommit => Label::FormEnvCommitHint,
                    _ => Label::FormSslmodeHint,
                });
                put(buf, cx + 2, y, &hint, room_from(cx + 2), dim);
                continue;
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
            // A session preset, picked from a list: its name (marked when no preset has it, or
            // when it is for another database), a profile's environment's with its name, none.
            Field::SessionPreset | Field::EnvSessionPg | Field::EnvSessionMy => {
                use crate::app::profiles::SessionChoice;
                let dbms = form.session_field_dbms(f);
                let choice = match f {
                    Field::SessionPreset => form.session_choice(),
                    _ => dbms.map_or(SessionChoice::Off, |d| form.env_session_choice(d)),
                };
                let (value, bad) = match &choice {
                    SessionChoice::Environment => {
                        let rules = form.picked_environment().map_or(&default_session, |e| &e.policy.session);
                        match dbms.and_then(|d| rules.get(d)) {
                            Some(name) => {
                                (i18n.msg(&Msg::PickerSessionEnvironment { name: name.to_string() }).to_string(), false)
                            }
                            None => (i18n.label(Label::PickerSessionEnvironmentNone).to_string(), false),
                        }
                    }
                    SessionChoice::Off => (i18n.label(Label::PickerSessionNone).to_string(), false),
                    SessionChoice::Named(name) => match form.session_presets.iter().find(|p| p.name == *name) {
                        Some(p) if Some(p.dbms) == dbms => (name.clone(), false),
                        Some(p) => (
                            i18n.msg(&Msg::FormSessionWrongDbms {
                                name: name.clone(),
                                dbms: p.dbms.display().to_string(),
                            })
                            .to_string(),
                            true,
                        ),
                        None => (i18n.msg(&Msg::FormSessionMissing { name: name.clone() }).to_string(), true),
                    },
                };
                let style = if bad { Style::new().fg(th.error) } else { Style::new().fg(th.fg) };
                let used = put(buf, x, y, &value, room_from(x), style.patch(field_bg));
                let used = used
                    + put(buf, x + used, y, " ▾ ", room_from(x + used), Style::new().fg(th.fg_muted).patch(field_bg));
                hits.push((Rect::new(x, y, used, 1), FormHit::Value(f)));
                x + used
            }
            // The statements, one per row, with what is wrong under each.
            Field::SessionSql => {
                let area = Rect::new(x, y, inner.width.saturating_sub(label_w as u16 + 2), list_h);
                hits.push((Rect::new(inner.x, y, inner.width, list_h), FormHit::Field(f)));
                let hint = i18n.label(match session_form {
                    true => Label::FormSessionSqlHint,
                    false => Label::FormProfileSessionSqlHint,
                });
                let drawn = statement_list(
                    buf,
                    &mut hits,
                    &mut form.session_sql,
                    area,
                    &statement_notes,
                    (i18n, &hint),
                    (focused, hover),
                );
                if drawn.is_some() {
                    cursor = drawn;
                }
                taller += list_h - 1;
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
                (Field::EnvBadge, _) => Some(Label::FormEnvBadgeHint.into()),
                (Field::EnvIdle, _) => Some(Label::FormEnvIdleHint.into()),
                (Field::EnvTxWarning, _) => Some(Label::FormEnvTxIdleWarningHint.into()),
                (Field::EnvTxRollback, _) => Some(Label::FormEnvTxIdleRollbackHint.into()),
                (Field::EnvSpill, _) => Some(Label::FormEnvSpillHint.into()),
                (Field::EnvIdleDisconnect, _) => Some(Label::FormEnvIdleDisconnectHint.into()),
                (Field::EnvLockWait, _) => Some(Label::FormEnvLockWaitHint.into()),
                (Field::SessionPreset | Field::EnvSessionPg | Field::EnvSessionMy, _) => {
                    Some(Label::FormSshListHint.into())
                }
                (Field::ServerKey, _) => Some(Label::FormServerKeyHint.into()),
                (Field::TlsCa, _) => Some(Label::FormTlsCaHint.into()),
                (Field::TlsCert, _) => Some(Label::FormTlsCertHint.into()),
                (Field::TlsKey, _) => Some(Label::FormTlsKeyHint.into()),
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
    // A connection without a tunnel to another machine, with TLS off: not encrypted.
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
    // An environment form's profiles, below its fields (the default rules': the profiles
    // without an environment).
    let named = match form.kind {
        FormKind::Environment { editing: true, .. } => form.original_name.clone(),
        FormKind::DefaultRules => Some(datarig_core::policy::DEFAULT.to_string()),
        _ => None,
    };
    if let Some(name) = named.as_ref() {
        let default = form.is_default_rules();
        let users: Vec<String> = profiles
            .iter()
            .filter(|p| match p.policy.as_deref() {
                None => default,
                Some(n) => n == name.as_str(),
            })
            .map(|p| p.name.clone())
            .collect();
        if !users.is_empty() {
            let y = inner.y + used_by_row;
            put(buf, inner.x + 1, y, &i18n.label(Label::FormFieldTunnelUsedBy), label_w, label_style(false));
            put(buf, inner.x + 1 + label_w as u16, y, &users.join(", "), iw.saturating_sub(label_w + 2), dim);
        }
    }
    // A session preset form's profiles, below its fields.
    if session_form && !session_users.is_empty() {
        let y = inner.y + used_by_row;
        put(buf, inner.x + 1, y, &i18n.label(Label::FormFieldTunnelUsedBy), label_w, label_style(false));
        put(buf, inner.x + 1 + label_w as u16, y, &session_users.join(", "), iw.saturating_sub(label_w + 2), dim);
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
    let y = inner.y + buttons_row;
    let mut x = inner.x + 1 + label_w as u16;
    // An environment form has nothing to test.
    for b in BUTTONS.into_iter().filter(|b| !(env_form && *b == Field::Test)) {
        let on = hover == Some(FormHit::Button(b));
        let style = if form.focus == b {
            Style::new().fg(th.bg).bg(th.accent).add_modifier(Modifier::BOLD)
        } else if on {
            Style::new().fg(th.accent).bg(th.surface_alt)
        } else {
            Style::new().fg(th.fg).bg(th.surface_alt)
        };
        let text = format!(" {} ", i18n.label(b.label()));
        let used = put(buf, x, y, &text, room_from(x), hovered(style, on && form.focus != b));
        hits.push((Rect::new(x, y, used, 1), FormHit::Button(b)));
        x += used + 2;
    }
    // Only what is inside the box can be clicked (a small screen cuts it).
    hits.retain_mut(|(r, _)| {
        *r = r.intersection(inner);
        !r.is_empty()
    });
    form.hits = hits;
    if !env_form {
        test_line(app, inner.x + 1, inner.y + buttons_row + 1, iw.saturating_sub(2), 2, buf);
    }
    cursor
}
