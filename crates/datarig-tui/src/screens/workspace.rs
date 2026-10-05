//! The workspace: the explorer on the left; the tabs with the editor and results on the right,
//! or the welcome panel while there is no profile; the status bar at the bottom.

use crate::app::action::Action;
use crate::app::tabs::ResultView;
use crate::app::{App, Focus, Layout, Paging, Results};
use crate::keymap::Ctx;
use crate::text::{clip, wrap, wrap_words};
use crate::theme;
use crate::widgets::explorer::draw_explorer;
use crate::widgets::popup::draw_popup;
use crate::widgets::statusbar::draw_status;
use crate::widgets::tabbar::draw_tab_bar;
use crate::widgets::{panel, panel_with_status, put};
use datarig_core::i18n::fmt_count;
use datarig_core::i18n::{Label, Msg};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout as RLayout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;

/// Explorer, editor, results and status bar. Returns where the hardware cursor belongs.
pub(crate) fn draw_workspace(f: &mut Frame, app: &mut App) -> Option<(u16, u16)> {
    let th = theme::cur();
    let area = f.area();
    // A second instance says so on a line of its own at the top.
    let banner = u16::from(app.read_only);
    let rows = RLayout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(banner), Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    if app.read_only {
        let r = rows[0];
        f.buffer_mut().set_style(r, Style::new().bg(th.warning).fg(th.bg));
        let text = match app.workspace_newer {
            Some(version) => app.i18n.msg(&Msg::WorkspaceNewer { version: version.to_string() }),
            None => app.i18n.label(Label::WorkspaceReadOnly),
        };
        let style = Style::new().fg(th.bg).bg(th.warning).add_modifier(Modifier::BOLD);
        let mark = crate::icons::warning(app.icons_on());
        put(f.buffer_mut(), r.x + 1, r.y, &format!("{mark} {text}"), r.width.saturating_sub(2) as usize, style);
    }
    let rows = [rows[1], rows[2]];
    let tree_w = (area.width * 25 / 100).clamp(24, 40);
    let cols = RLayout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(tree_w), Constraint::Min(1)])
        .split(rows[0]);

    // Explorer
    let tb = panel(&app.i18n.label(Label::PaneTreeTitle), app.focus == Focus::Tree, cols[0].width);
    let inner = tb.inner(cols[0]);
    tb.render(cols[0], f.buffer_mut());
    let focused = app.focus == Focus::Tree;
    let filter_cursor = draw_explorer(app, inner, f.buffer_mut(), focused);

    if app.profiles.is_empty() {
        app.layout = Layout { tree: cols[0], editor: cols[1], results: Rect::default(), ..Layout::default() };
        draw_welcome(app, cols[1], f.buffer_mut());
        draw_status(app, rows[1], f.buffer_mut());
        return filter_cursor;
    }
    if app.tabs.is_empty() {
        app.layout = Layout { tree: cols[0], editor: cols[1], results: Rect::default(), ..Layout::default() };
        draw_empty(app, cols[1], f.buffer_mut());
        draw_status(app, rows[1], f.buffer_mut());
        return filter_cursor;
    }

    let bar_and_panes = RLayout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(cols[1]);
    let body = bar_and_panes[1];
    app.layout = Layout { tree: cols[0], body, ..Layout::default() };

    app.layout.tab_bar = bar_and_panes[0];
    app.tab_hits = draw_tab_bar(app, bar_and_panes[0], f.buffer_mut());

    // A table tab: its results at full height. A query tab: the editor, and below it the
    // results pane once it ran, sized, hidden or maximised per tab.
    let (editor_area, results_area) = if app.tab().is_table() {
        (None, Some(body))
    } else if !app.results_shown() {
        (Some(body), None)
    } else if app.tab().pane.maximized {
        (None, Some(body))
    } else {
        let rows = crate::app::pane::results_rows(body.height, app.tab().pane.share);
        let editor = Rect { height: body.height - rows, ..body };
        let results = Rect { y: body.y + body.height - rows, height: rows, ..body };
        app.layout.divider = Rect { height: 1, ..results };
        (Some(editor), Some(results))
    };
    let cursor = match editor_area {
        Some(area) => draw_editor(app, area, f.buffer_mut()),
        None => (0, 0),
    };
    app.layout.editor_cursor = cursor;
    if let Some(area) = results_area {
        draw_results_pane(app, area, f.buffer_mut());
    }

    draw_status(app, rows[1], f.buffer_mut());

    if editor_area.is_some() && app.tab().popup.is_some() && app.focus == Focus::Editor && !app.modal_open() {
        draw_popup(app, cursor, area, f.buffer_mut());
    }
    // Real terminal cursor at the focused text input so IME preedit appears in place.
    if app.focus == Focus::Tree {
        filter_cursor
    } else {
        (app.focus == Focus::Editor && editor_area.is_some()).then_some(cursor)
    }
}

/// The editor pane of a query tab: the connection bar (or the unbound banner) on its first
/// line, then the text. Returns where the cursor is.
fn draw_editor(app: &mut App, area: Rect, buf: &mut Buffer) -> (u16, u16) {
    // A saved query by its path in the folder of saved queries.
    let name = match app.tab().script() {
        Some(p) => datarig_core::scripts::stem_path(p).to_string(),
        None => crate::widgets::tabbar::document_name(app, app.tab()),
    };
    let title = app.i18n.msg(&Msg::PaneEditorScript { name });
    let eb = panel(&title, app.focus == Focus::Editor, area.width);
    let mut einner = eb.inner(area);
    eb.render(area, buf);
    if einner.height > 2 {
        let line = Rect { height: 1, ..einner };
        if app.tab().profile.is_none() {
            draw_unbound_banner(app, line, buf);
        } else {
            draw_connection_bar(app, line, buf);
        }
        einner = Rect { y: einner.y + 1, height: einner.height - 1, ..einner };
    }
    app.layout.editor = area;
    app.layout.editor_text = einner;
    let editor = &mut app.tabs.active_mut().editor;
    // The statement a run would take (the selection instead, while there is one: the editor
    // marks it itself).
    let stmt = match editor.mode {
        crate::widgets::editor::Mode::Visual => None,
        _ => editor.current_statement().map(|(start, end, _)| (start, end)),
    };
    editor.render(einner, buf, stmt)
}

/// The results pane: the title with the paging state on its right, the grid, and the
/// inspector on the right of the grid when it is shown as a panel.
fn draw_results_pane(app: &mut App, area: Rect, buf: &mut Buffer) {
    let (title, status) = results_title(app);
    let status = status.as_ref().map(|(texts, style)| (texts.as_slice(), *style));
    let panel_w = app.inspector_shown().then(|| crate::widgets::inspector::width_for(area.width)).flatten();
    let (grid_area, detail_area) = match panel_w {
        Some(w) => (Rect { width: area.width - w, ..area }, Rect { x: area.x + area.width - w, width: w, ..area }),
        None => (area, Rect::default()),
    };
    app.layout.results = grid_area;
    app.layout.detail = detail_area;
    // Where the previous and next page marks are, for the mouse: at both ends of the right
    // side of the top border, when it is drawn with them.
    (app.layout.page_prev, app.layout.page_next) = (Rect::default(), Rect::default());
    if let Some((texts, _)) = status {
        let (_, shown) = crate::widgets::title_and_status(&title, texts, grid_area.width);
        let (prev, next) = crate::icons::page_arrows(app.icons_on());
        if let Some(s) = shown.filter(|s| s.starts_with(prev) && s.ends_with(next)) {
            let w = crate::text::width(&s) as u16 + 2;
            let start = grid_area.x + grid_area.width - 1 - w;
            let (pw, nw) = (crate::text::width(prev) as u16, crate::text::width(next) as u16);
            app.layout.page_prev = Rect { x: start + 1, y: grid_area.y, width: pw, height: 1 };
            app.layout.page_next = Rect { x: start + w - 1 - nw, y: grid_area.y, width: nw, height: 1 };
        }
    }
    let rb = panel_with_status(&title, status, app.focus == Focus::Results, grid_area.width);
    let rinner = rb.inner(grid_area);
    rb.render(grid_area, buf);
    // A query tab that ran: the result tabs of its last run and its Messages on the first line.
    let strip = app.strip_shown() && rinner.height > 2;
    app.strip_hits.clear();
    let content = if strip {
        let line = Rect { height: 1, ..rinner };
        app.layout.strip = line;
        draw_strip(app, line, buf);
        Rect { y: rinner.y + 1, height: rinner.height - 1, ..rinner }
    } else {
        app.layout.strip = Rect::default();
        rinner
    };
    if app.tab().exec.view == ResultView::Messages && !app.tab().is_table() {
        draw_messages(app, content, buf);
    } else {
        draw_results(app, content, buf);
    }
    if panel_w.is_some() {
        crate::widgets::inspector::draw_inspector(app, detail_area, buf);
    }
}

/// The result tab strip: `[Result 1]  Result 3  Messages`, the shown one in
/// brackets, `earlier run` when a later run returned no rows, and on the right what the shown
/// rows' transaction is (uncommitted, ended, rolled back). The transaction comes first. When the
/// tabs do not all fit, the shown one and Messages always stay, with as many of the others
/// around the shown one as fit, `…` where some are left out (`H`/`L` reach them); only when
/// even those two do not fit do the labels shorten (`[1]  Msgs`), and then the shown one stays
/// alone; `earlier run` goes first.
/// Records where each tab is.
fn draw_strip(app: &mut App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    use crate::widgets::grid::TxMark;
    let bg = th.surface;
    buf.set_style(area, Style::new().bg(bg));
    let t = app.tab();
    let active = |i: Option<usize>| match (t.exec.view, i) {
        (ResultView::Messages, None) => true,
        (ResultView::Rows, Some(i)) => t.exec.shown == Some(i) && matches!(t.results, Results::Rows(_)),
        _ => false,
    };
    let tx = match (&t.results, t.exec.view) {
        (Results::Rows(rs), ResultView::Rows) => rs.tx,
        _ => None,
    };
    let tx = tx.map(|mark| {
        let (label, color) = match mark {
            TxMark::InTx(_) => (Label::ResultsTxOpen, th.warning),
            TxMark::Ended => (Label::ResultsTxEnded, th.fg_muted),
            TxMark::RolledBack => (Label::ResultsTxRolledBack, th.error),
        };
        (app.i18n.label(label).to_string(), color)
    });
    let tabs = t.result_tabs();
    let labels = |short: bool| -> Vec<(Option<usize>, String)> {
        let mut v: Vec<(Option<usize>, String)> = tabs
            .iter()
            .map(|&i| {
                let n = (i + 1).to_string();
                (Some(i), if short { n } else { app.i18n.msg(&Msg::ResultsTabResult { n }).to_string() })
            })
            .collect();
        let m = if short { Label::ResultsTabMessagesShort } else { Label::ResultsTabMessages };
        v.push((None, app.i18n.label(m).to_string()));
        v
    };
    let room = (area.width as usize).saturating_sub(1);
    let tx_w = tx.as_ref().map_or(0, |(text, _)| crate::text::width(text) + 2);
    let room = room.saturating_sub(tx_w);
    let full = labels(false);
    let at = full.iter().position(|(i, _)| active(*i));
    let shown = strip_window(&full, at, room)
        .map(|shown| (full.clone(), shown))
        .or_else(|| {
            let short = labels(true);
            strip_window(&short, at, room).map(|shown| (short, shown))
        })
        // Not even that: the shown one alone, short, when it fits (the transaction comes first).
        .unwrap_or_else(|| {
            let short = labels(true);
            let k = at.unwrap_or(short.len() - 1);
            let alone = if crate::text::width(&short[k].1) + 2 <= room { vec![k] } else { Vec::new() };
            (short, alone)
        });
    let (items, shown) = shown;
    let earlier = t.exec.kept_log.is_some().then(|| format!("· {}", app.i18n.label(Label::ResultsTabEarlier)));
    let mut hits = Vec::new();
    let mut x = area.x + 1;
    let end = area.x + area.width - (tx_w as u16).min(area.width);
    let dim = Style::new().fg(th.fg_dim).bg(bg);
    let mut next = 0;
    for k in shown {
        if k > next && x < end {
            put(buf, x, area.y, "…", (end - x) as usize, dim);
            x += 2;
        }
        next = k + 1;
        let (i, label) = &items[k];
        let on = active(*i);
        let text = if on { format!("[{label}]") } else { format!(" {label} ") };
        let style = if on {
            Style::new().fg(th.accent).bg(bg).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(th.fg_muted).bg(bg)
        };
        // Whole labels only (H/L reach the ones left out).
        if x as usize + crate::text::width(&text) > end as usize {
            break;
        }
        let w = put(buf, x, area.y, &text, (end - x) as usize, style);
        hits.push((x, x + w, *i));
        x += w + 1;
    }
    if let Some(e) = earlier.filter(|e| x as usize + crate::text::width(e) <= end as usize) {
        put(buf, x, area.y, &e, (end - x) as usize, dim);
    }
    if let Some((text, color)) = tx {
        let w = crate::text::width(&text) as u16;
        let at = (area.x + area.width).saturating_sub(w + 1).max(area.x + 1);
        let room = (area.x + area.width - at) as usize;
        put(buf, at, area.y, &text, room, Style::new().fg(color).bg(bg).add_modifier(Modifier::BOLD));
    }
    app.strip_hits = hits;
}

/// Which of the strip's `items` (Messages last) to draw in `room` columns: every one when they
/// fit; else the shown one (`at`) and Messages, and the others nearest the shown one while they
/// fit, with room for a `…` where some are left out. `None` when the shown one and Messages do
/// not fit.
fn strip_window(items: &[(Option<usize>, String)], at: Option<usize>, room: usize) -> Option<Vec<usize>> {
    let w = |k: usize| crate::text::width(&items[k].1) + 3;
    let last = items.len().checked_sub(1)?;
    let cost = |shown: &[usize]| {
        let gaps = shown.iter().enumerate().filter(|(n, k)| **k > if *n == 0 { 0 } else { shown[n - 1] + 1 }).count();
        shown.iter().map(|&k| w(k)).sum::<usize>() + 2 * gaps
    };
    let all: Vec<usize> = (0..items.len()).collect();
    if cost(&all) <= room {
        return Some(all);
    }
    // The last one drawn needs no blank after it.
    let room = room + 1;
    let at = at.unwrap_or(0);
    let mut shown = vec![at, last];
    shown.dedup();
    if cost(&shown) > room {
        return None;
    }
    // Nearest the shown one first, alternating left and right; a side stops at the first tab
    // that does not fit.
    let mut sides = [at.checked_sub(1), (at + 1 < last).then_some(at + 1)];
    while sides.iter().any(Option::is_some) {
        for (n, side) in sides.iter_mut().enumerate() {
            let Some(k) = *side else { continue };
            let mut more = shown.clone();
            more.push(k);
            more.sort_unstable();
            if cost(&more) > room {
                *side = None;
                continue;
            }
            shown = more;
            *side = if n == 0 { k.checked_sub(1) } else { Some(k + 1).filter(|k| *k < last) };
        }
    }
    Some(shown)
}

/// The Messages of the last run: each statement, its number, its first line
/// and what it did (and how long it took); then the app's notes about the run.
fn draw_messages(app: &mut App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    use crate::app::runlog::StatementOutcome as O;
    buf.set_style(area, th.base());
    let log = &app.tab().exec.run;
    let mut lines: Vec<Vec<(String, Style)>> = Vec::new();
    let num_w = log.len().to_string().len();
    let sql_w = (area.width as usize / 2).clamp(12, 60);
    for (i, s) in log.statements.iter().enumerate() {
        let (text, color) = match &s.outcome {
            O::Waiting => (app.i18n.label(Label::MessagesWaiting).to_string(), th.fg_dim),
            O::Running => (app.i18n.label(Label::MessagesRunning).to_string(), th.accent),
            O::Rows { count, more: false } => {
                (app.i18n.msg(&Msg::MessagesRows { count: *count }).to_string(), th.success)
            }
            O::Rows { count, more: true } => {
                (app.i18n.msg(&Msg::MessagesRowsMore { count: *count }).to_string(), th.success)
            }
            O::Affected(n) => (app.i18n.msg(&Msg::MessagesAffected { count: *n }).to_string(), th.success),
            O::Command(tag) => (tag.clone(), th.success),
            O::Failed(e) => (app.i18n.msg(&Msg::QueryError { error: e.clone() }).to_string(), th.error),
            O::Cancelled => (app.i18n.label(Label::QueryCancelled).to_string(), th.warning),
            O::NotRun => (app.i18n.label(Label::MessagesNotRun).to_string(), th.fg_dim),
        };
        let elapsed = s.elapsed.map(|d| format!(" · {}", datarig_core::i18n::fmt_elapsed(d))).unwrap_or_default();
        let sql = crate::app::runlog::excerpt(&s.sql, sql_w);
        lines.push(vec![
            (format!("{:>num_w$}  ", i + 1), Style::new().fg(th.fg_muted).bg(th.bg)),
            (
                format!("{}  ", crate::text::fit(&sql, sql_w, crate::text::Align::Left)),
                Style::new().fg(th.fg).bg(th.bg),
            ),
            (format!("{text}{elapsed}"), Style::new().fg(color).bg(th.bg)),
        ]);
    }
    if log.is_empty() {
        lines.push(vec![(app.i18n.label(Label::MessagesEmpty).to_string(), Style::new().fg(th.fg_dim).bg(th.bg))]);
    }
    // A note is shown whole (why a statement failed and how to run it, a reason
    // the status bar cuts): wrapped at words, its later lines under its text. The app's own
    // notices (not about a run: launch warnings, why a database could not be opened) follow.
    let note_w = (area.width as usize).saturating_sub(4);
    for n in log.notes.iter().chain(&app.notices) {
        let color = crate::widgets::statusbar::level_color(n.level);
        for (i, l) in crate::text::wrap_words(&n.render(&app.i18n), note_w).into_iter().enumerate() {
            let text = format!("{} {l}", if i == 0 { "·" } else { " " });
            lines.push(vec![(text, Style::new().fg(color).bg(th.bg))]);
        }
    }
    let h = area.height as usize;
    let t = app.tabs.active_mut();
    let top = t.exec.messages_scroll.min(lines.len().saturating_sub(h));
    t.exec.messages_scroll = top;
    for (row, parts) in lines.iter().skip(top).take(h).enumerate() {
        let y = area.y + row as u16;
        let mut x = area.x + 1;
        let end = area.x + area.width;
        for (text, style) in parts {
            if x >= end {
                break;
            }
            x += put(buf, x, y, text, (end - x) as usize, *style);
        }
    }
}

/// The style of the profile's name on the editor's first line: its color, bold.
fn name_style(p: &datarig_core::profile::ConnectionConfig, bg: ratatui::style::Color) -> Style {
    Style::new().fg(theme::profile_color(p.display_color())).bg(bg).add_modifier(Modifier::BOLD)
}

/// The first line of the editor of a tab with a connection: the profile (its icon and name in
/// its color), where it points, its policy (and read-only), and the key that switches the
/// tab to another connection.
fn draw_connection_bar(app: &App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    let Some(p) = app.conn() else { return };
    let bg = th.surface;
    buf.set_style(area, Style::new().bg(bg));
    let (endpoint, database, _) = p.endpoint();
    // Where the tab works: the server's answer once its session connected, else
    // the chosen database, else the profile's; then the chosen schema.
    let t = app.tab();
    let database = match (&t.exec.context, &t.context.database) {
        (Some((db, _)), _) => db.clone(),
        (None, Some(db)) => db.clone(),
        (None, None) => database,
    };
    let mut place = p.name.clone();
    if !database.is_empty() {
        place.push_str(" / ");
        place.push_str(&database);
    }
    // A chosen schema the server does not have on the path is marked, in the
    // warning color, for as long as that is so.
    let missing = app.schema_missing(t);
    let schema = t.context.schema.clone().filter(|s| !s.is_empty()).map(|s| {
        let style =
            if missing { Style::new().fg(th.warning).bg(bg).add_modifier(Modifier::BOLD) } else { name_style(p, bg) };
        (format!(" / {s}{}", if missing { "?" } else { "" }), style)
    });
    let policy = app.i18n.msg(&Msg::StatusPolicy { name: p.policy.clone().unwrap_or_else(|| "default".into()) });
    let name = name_style(p, bg);
    let muted = Style::new().fg(th.fg_muted).bg(bg);
    let mut parts: Vec<(String, Style)> = Vec::new();
    if app.icons_on() {
        parts.push((crate::icons::cell(p, true), name));
    }
    parts.push((place, name));
    parts.extend(schema);
    parts.push((format!("  {endpoint}"), muted));
    parts.push((format!(" · {policy}"), muted));
    if app.read_only(p.id) {
        let ro = Style::new().fg(th.fg).bg(th.surface_alt).add_modifier(Modifier::BOLD);
        parts.push((" ".to_string(), muted));
        parts.push((format!(" {} ", app.i18n.label(Label::StatusReadOnly)), ro));
    }
    let key = app
        .keymap
        .hint_keys(Action::SetTabConnection, Ctx::Nav, app.enhanced_keys)
        .map(|k| crate::keymap::keys::label(&k))
        .unwrap_or_default();
    let switch = app.i18n.msg(&Msg::ConnbarSwitch { key }).to_string();
    let right = crate::text::width(&switch) as u16 + 1;
    let room = area.width.saturating_sub(1);
    // The switch key on the right only when the connection fits whole next to it (narrow
    // panes show the connection; which-key and the help list the key).
    let width_of = |parts: &[(String, Style)]| parts.iter().map(|(t, _)| crate::text::width(t)).sum::<usize>();
    let left_w = width_of(&parts);
    let left_room = if left_w as u16 + right + 2 <= room { room - right - 1 } else { room };
    // Too narrow for all of it: the policy's name goes first, then the endpoint, so where the
    // tab works and the read-only badge stay (the place grew a database and schema).
    let optional = |t: &str| t.starts_with(" · ") || t.starts_with("  ");
    while width_of(&parts) > left_room as usize {
        let Some(i) = parts.iter().rposition(|(t, _)| optional(t)) else { break };
        parts.remove(i);
    }
    let mut x = area.x + 1;
    for (text, style) in &parts {
        let used = x - area.x - 1;
        if used >= left_room {
            break;
        }
        x += put(buf, x, area.y, text, (left_room - used) as usize, *style);
    }
    if left_room < room {
        put(buf, area.x + area.width - right, area.y, &switch, right as usize, muted);
    }
}

/// The results pane's title and, on its right in parentheses, where the pages are:
/// `Results · rows 501–1,000` ─── `‹ (page 2 / ? · 23s) ›`. The page and the pages
/// (`?` until the result is complete or counted), then the state of the result's portal: the
/// countdown to its idle close, `in tx` inside the user's transaction (never closed for being
/// idle), `paging` when the policy never closes it, `paging closed`, `fetching stopped`,
/// `counting…`, and the count once known. `‹` and `›` (Nerd Font carets with icons on) are the
/// previous and next page for the mouse. A complete result of one page shows nothing there.
/// Returns the title, and the texts of the right side from the longest to the shortest (the
/// shortest is the state alone, without the marks) with their style.
fn results_title(app: &App) -> (datarig_core::i18n::Localized, Option<(Vec<String>, Style)>) {
    let th = theme::cur();
    let t = app.tab();
    let Results::Rows(rs) = &t.results else { return (app.i18n.label(Label::PaneResultsTitle), None) };
    if t.exec.view == ResultView::Messages && !t.is_table() {
        return (app.i18n.label(Label::PaneResultsTitle), None);
    }
    let size = app.page_size.max(1);
    let len = rs.rows.len();
    let win = t.grid.window(len);
    let paged = rs.more || len > size;
    let title = if paged && !win.is_empty() {
        let (from, to) = (fmt_count(win.start as u64 + 1), fmt_count(win.end as u64));
        app.i18n.msg(&Msg::PaneResultsRange { from, to })
    } else {
        app.i18n.msg(&Msg::PaneResultsRows { count: len as u64 })
    };
    // The portal belongs to the last statement of the rows' run.
    let answer = t.exec.shown.is_some() && t.exec.shown == t.answer_index();
    let paging = if answer { t.exec.paging } else { Paging::None };
    let warn = Style::new().fg(th.warning).bg(th.bg);
    let muted = Style::new().fg(th.fg_muted).bg(th.bg);
    let mut states: Vec<String> = Vec::new();
    let mut style = muted;
    if t.exec.running.is_some_and(|r| r.count) {
        states.push(app.i18n.label(Label::ResultsTitleCounting).to_string());
    }
    match (paging, app.paging_left()) {
        (Paging::Open { in_block: true, .. }, _) => states.push(app.i18n.label(Label::ResultsTitleInTx).to_string()),
        (Paging::Open { .. }, Some(d)) => {
            states.push(crate::app::paging::fmt_left(d));
            style = warn;
        }
        (Paging::Open { .. }, None) => states.push(app.i18n.label(Label::ResultsTitlePaging).to_string()),
        (Paging::ClosedIdle | Paging::Replaced | Paging::Interrupted, _) if rs.more => {
            states.push(app.i18n.label(Label::ResultsTitlePagingClosed).to_string());
            style = warn;
        }
        (Paging::Stopped, _) => {
            states.push(app.i18n.label(Label::ResultsTitlePagingStopped).to_string());
            style = warn;
        }
        _ => {}
    }
    if let (Some(n), true) = (rs.counted, rs.more) {
        let m = if rs.counted_now {
            Msg::ResultsTitleCountedNow { count: n }
        } else {
            Msg::ResultsTitleCounted { count: n }
        };
        states.push(app.i18n.msg(&m).to_string());
    }
    if !paged && states.is_empty() {
        return (title, None);
    }
    let page = (win.start / size + 1).to_string();
    let pages = rs.total().map_or_else(|| "?".to_string(), |n| crate::app::paging::pages(n, size).to_string());
    let (prev, next) = crate::icons::page_arrows(app.icons_on());
    let long = app.i18n.msg(&Msg::ResultsTitlePage { page: page.clone(), pages: pages.clone() });
    let short = app.i18n.msg(&Msg::ResultsTitlePageShort { page, pages });
    let full = std::iter::once(long.to_string()).chain(states.iter().cloned()).collect::<Vec<_>>().join(" · ");
    let brief = std::iter::once(short.to_string()).chain(states.first().cloned()).collect::<Vec<_>>().join(" · ");
    let least = states.first().cloned().unwrap_or_else(|| short.to_string());
    // Narrower: the arrows go before the page number does, then the state before the page
    // number (the line below the rows says a closed portal too).
    let texts = vec![
        format!("{prev} ({full}) {next}"),
        format!("{prev} ({brief}) {next}"),
        format!("({brief})"),
        format!("{prev} ({short}) {next}"),
        format!("({short})"),
        format!("({least})"),
    ];
    (title, Some((texts, style)))
}

/// Launch messages (migration, key map problems, an unknown profile) from line `y` down to
/// `bottom`; returns the next free line.
fn draw_notices(app: &App, x: u16, mut y: u16, w: usize, bottom: u16, buf: &mut Buffer) -> u16 {
    let th = theme::cur();
    for n in &app.notices {
        for l in wrap(&n.render(&app.i18n), w) {
            if y >= bottom {
                return y;
            }
            put(buf, x, y, &l, w, Style::new().fg(crate::widgets::statusbar::level_color(n.level)).bg(th.bg));
            y += 1;
        }
    }
    y
}

/// The first line of the editor of a tab without a connection: why, and the key that picks
/// one (a profile that is not among the profiles says so: the config may be unreadable).
fn draw_unbound_banner(app: &App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    let key = app
        .keymap
        .hint_keys(Action::SetTabConnection, Ctx::Nav, app.enhanced_keys)
        .map(|k| crate::keymap::keys::label(&k))
        .unwrap_or_default();
    let text = if app.tab().doc.kept_profile.is_some() {
        app.i18n.msg(&Msg::BannerUnknownProfile { key })
    } else {
        app.i18n.msg(&Msg::BannerUnbound { key })
    };
    let style = Style::new().fg(th.bg).bg(th.warning).add_modifier(Modifier::BOLD);
    buf.set_style(area, style);
    let mark = crate::icons::warning(app.icons_on());
    put(buf, area.x + 1, area.y, &format!("{mark} {text}"), area.width.saturating_sub(2) as usize, style);
}

/// The workspace without a tab (profiles exist): how to open one — pick a connection in the
/// explorer or quick connect — with the keys, and the launch messages.
fn draw_empty(app: &mut App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    let block = panel(&app.i18n.label(Label::EmptyTitle), false, area.width);
    let inner = block.inner(area);
    block.render(area, buf);
    let x = inner.x + 2;
    let w = (inner.width as usize).saturating_sub(4);
    let bottom = inner.y + inner.height;
    let mut y = inner.y + 1;
    let keys = |a: Action| {
        app.keymap
            .hint_keys(a, Ctx::Explorer, app.enhanced_keys)
            .map(|k| crate::keymap::keys::label(&k))
            .unwrap_or_default()
    };
    let bold = Style::new().fg(th.fg).bg(th.bg).add_modifier(Modifier::BOLD);
    put(buf, x, y, &app.i18n.label(Label::EmptyHeadline), w, bold);
    y += 1;
    let pick = app.i18n.msg(&Msg::EmptyPick { key: keys(Action::QuickConnect) });
    for l in wrap_words(&pick, w) {
        put(buf, x, y, &l, w, Style::new().fg(th.fg_muted).bg(th.bg));
        y += 1;
    }
    y += 1;
    let entries = [
        (keys(Action::Explorer(crate::app::action::ExplorerAction::Activate)), app.i18n.label(Label::EmptyConnect)),
        (keys(Action::QuickConnect), app.i18n.label(Label::EmptyQuick)),
        (keys(Action::NewTab), app.i18n.label(Label::EmptyNewTab)),
        (keys(Action::ReopenTab), app.i18n.label(Label::EmptyReopen)),
        (keys(Action::NewProfile), app.i18n.label(Label::WelcomeNew)),
        (keys(Action::Help), app.i18n.label(Label::WelcomeHelp)),
        (keys(Action::OpenCommands), app.i18n.label(Label::WelcomeCommands)),
    ];
    let key_w = entries.iter().map(|(k, _)| crate::text::width(k)).max().unwrap_or(0).min(w / 2);
    for (k, label) in &entries {
        if y >= bottom {
            return;
        }
        put(buf, x, y, k, key_w, Style::new().fg(th.accent_warm).bg(th.bg).add_modifier(Modifier::BOLD));
        let lx = x + key_w as u16 + 2;
        put(buf, lx, y, label, w.saturating_sub(key_w + 2), Style::new().fg(th.fg).bg(th.bg));
        y += 1;
    }
    y += 1;
    draw_notices(app, x, y, w, bottom, buf);
}

/// The welcome panel (no profile yet): what datarig is, and the keys that get
/// started (new connection, paste a URL, keyboard help).
fn draw_welcome(app: &mut App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    let block = panel(&app.i18n.label(Label::WelcomeTitle), app.focus != Focus::Tree, area.width);
    let inner = block.inner(area);
    block.render(area, buf);
    let x = inner.x + 2;
    let w = (inner.width as usize).saturating_sub(4);
    let bottom = inner.y + inner.height;
    let mut y = inner.y + 1;
    // The name as text, icons on or off.
    let name = Style::new().fg(th.accent).bg(th.bg).add_modifier(Modifier::BOLD);
    put(buf, x, y, "datarig", w, name);
    y += 1;
    for l in wrap_words(&app.i18n.label(Label::WelcomeTagline), w) {
        put(buf, x, y, &l, w, Style::new().fg(th.fg_muted).bg(th.bg));
        y += 1;
    }
    y += 1;
    put(
        buf,
        x,
        y,
        &app.i18n.label(Label::WelcomeEmpty),
        w,
        Style::new().fg(th.fg).bg(th.bg).add_modifier(Modifier::BOLD),
    );
    y += 2;
    let keys = |a: Action| {
        app.keymap
            .hint_keys(a, Ctx::Welcome, app.enhanced_keys)
            .map(|k| crate::keymap::keys::label(&k))
            .unwrap_or_default()
    };
    let entries = [
        (keys(Action::NewProfile), app.i18n.label(Label::WelcomeNew)),
        (keys(Action::Help), app.i18n.label(Label::WelcomeHelp)),
        (keys(Action::OpenCommands), app.i18n.label(Label::WelcomeCommands)),
        (keys(Action::Quit), app.i18n.label(Label::WelcomeQuit)),
    ];
    let key_w = entries.iter().map(|(k, _)| crate::text::width(k)).max().unwrap_or(0).min(w / 2);
    for (k, label) in &entries {
        if y >= bottom {
            return;
        }
        put(buf, x, y, k, key_w, Style::new().fg(th.accent_warm).bg(th.bg).add_modifier(Modifier::BOLD));
        let lx = x + key_w as u16 + 2;
        put(buf, lx, y, label, w.saturating_sub(key_w + 2), Style::new().fg(th.fg).bg(th.bg));
        y += 1;
    }
    if y < bottom {
        put(buf, x, y, &app.i18n.label(Label::WelcomePaste), w, Style::new().fg(th.fg_muted).bg(th.bg));
    }
    y += 2;
    draw_notices(app, x, y, w, bottom, buf);
}

pub(crate) fn draw_results(app: &mut App, area: Rect, buf: &mut Buffer) {
    let th = theme::cur();
    let msg_line = |buf: &mut Buffer, text: &str, color| {
        for (i, l) in wrap(text, area.width as usize).iter().take(area.height as usize).enumerate() {
            buf.set_stringn(
                area.x + 1,
                area.y + i as u16,
                l,
                area.width.saturating_sub(1) as usize,
                Style::new().fg(color).bg(th.bg),
            );
        }
    };
    let focused = app.focus == Focus::Results;
    // The key marks of the result's columns, from the profile's key cache (none while it is
    // not known).
    let keys = Some(app.tab_keys(app.tab()));
    let marks: Vec<datarig_core::driver::KeyMarks> = match &app.tab().results {
        Results::Rows(rs) => {
            rs.columns.iter().map(|c| keys.map(|k| k.marks(c.meta.origin)).unwrap_or_default()).collect()
        }
        _ => Vec::new(),
    };
    let look = crate::widgets::grid::Look { marks: &marks, icons: app.icons_on() };
    // The portal belongs to the last statement of the rows' run: only its rows page.
    let answer = app.tab().exec.shown.is_some() && app.tab().exec.shown == app.tab().answer_index();
    let next_key = app
        .keymap
        .hint_keys(Action::PageNext, Ctx::Grid, app.enhanced_keys)
        .map(|k| crate::keymap::keys::label(&k))
        .unwrap_or_default();
    let run_again = app.can_run_again();
    let t = app.tabs.active_mut();
    match &mut t.results {
        Results::Empty => {
            // A table tab not run yet (restored): nothing was sent; the run key loads it.
            let t = if t.is_table() {
                app.i18n.msg(&Msg::TableNotLoaded { run: app.run_key().into() })
            } else {
                app.i18n.msg(&Msg::ResultsEmpty { run: app.run_key().into() })
            };
            msg_line(buf, &t, th.fg_dim);
            let lines = wrap(&t, area.width as usize).len() as u16;
            let w = area.width.saturating_sub(1) as usize;
            draw_notices(app, area.x + 1, area.y + lines + 1, w, area.y + area.height, buf);
        }
        // A result whose portal was closed for being idle, or whose fetching stopped at its
        // spill limit, keeps its rows and says so below them.
        Results::Rows(rs)
            if answer
                && ((rs.more && t.exec.paging.closed()) || t.exec.paging == Paging::Stopped)
                && area.height > 3 =>
        {
            let grid = Rect { height: area.height - 1, ..area };
            crate::widgets::grid::render(rs, &mut t.grid, grid, buf, &app.i18n, focused, look);
            let count = rs.rows.len() as u64;
            let text = match t.exec.paging {
                Paging::Stopped => app.i18n.msg(&Msg::ResultsPagingStopped { count }),
                // The next page runs the statement again when it may.
                _ if run_again => app.i18n.msg(&Msg::ResultsPagingClosedRerun { count, key: next_key.clone() }),
                Paging::Replaced => app.i18n.msg(&Msg::ResultsPagingReplaced { count }),
                Paging::Interrupted => app.i18n.msg(&Msg::ResultsPagingInterrupted { count }),
                _ => app.i18n.msg(&Msg::ResultsPagingClosed { count }),
            };
            let y = area.y + area.height - 1;
            buf.set_style(Rect { y, height: 1, ..area }, Style::new().bg(th.surface));
            let w = area.width.saturating_sub(1) as usize;
            buf.set_stringn(area.x + 1, y, clip(&text, w), w, Style::new().fg(th.warning).bg(th.surface));
        }
        Results::Rows(rs) => crate::widgets::grid::render(rs, &mut t.grid, area, buf, &app.i18n, focused, look),
        Results::Message(m) => msg_line(buf, &m.render(&app.i18n), th.success),
        Results::Error(e) => {
            // In a run of several statements: which one failed.
            let log = &t.exec.run;
            let msg = match log.failed().filter(|_| log.several()) {
                Some((i, error)) => Msg::QueryStepFailed {
                    step: (i + 1).to_string(),
                    total: log.len().to_string(),
                    sql: crate::app::runlog::excerpt(&log.statements[i].sql, 40),
                    error: error.to_string(),
                },
                None => Msg::QueryError { error: e.clone() },
            };
            msg_line(buf, &app.i18n.msg(&msg), th.error)
        }
        Results::Cancelled => msg_line(buf, &app.i18n.label(Label::QueryCancelled), th.warning),
    }
    // Rows of the spill file that could not be read are shown as not read, and said so.
    if let Some(fault) = app.tabs.active_mut().grid.read_error.take() {
        app.results_read_failed(&fault);
    }
}
