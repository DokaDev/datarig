//! The tab bar above the editor: each tab is `<n> <document> [RO] [<mark>] ×`. The number carries
//! the tab's connection: drawn in the profile's color while its session is connected (prod and
//! dev stay apart without a chip; the profile's name is on the editor's first line), muted while
//! it is not (the normal idle state: sessions open lazily), and replaced by the status bar's
//! spinner while a statement runs or waits. The document is a console's `console <n>`, a saved
//! query's name or a table tab's `schema.table`, `*` marks a text that could not be saved, `RO`
//! a read-only policy. A mark after it warns, by shape first ([`State`]): `◆` in the warning
//! color while the user's transaction is open, `!` for a lost connection, a failed profile or
//! an aborted transaction; a tab with nothing to warn about has no mark. `×` closes the tab as
//! `Ctrl+W` does. A tab without a connection says so in the mark's place. When the tabs do not
//! fit, the documents' names are shortened first, then the bar scrolls to keep the active tab
//! visible and `‹` / `›` show that more tabs are hidden.

use crate::app::{App, NodeState, SessionState, Tab};
use crate::icons;
use crate::text::{clip, width};
use crate::theme;
use datarig_core::i18n::{Label, Msg};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

/// The warning marks (plain Unicode, not Nerd Font glyphs: the same with icons off), each its
/// own shape, not only another color.
pub const TX_OPEN: &str = "◆";
pub const TROUBLE: &str = "!";
/// The close button.
pub const CLOSE: &str = "×";
/// The tab's text could not be saved (a failed write, or its file changed on disk).
pub const UNSAVED: &str = "*";

/// What a tab's own session is doing, as its number and mark show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// No session (not connected yet, or closed): the number muted, no mark.
    Idle,
    /// Connected and idle: the number in the profile's color, no mark.
    Connected,
    /// A statement runs or waits for its connection: a spinner frame (the accent color) in the
    /// number's place; `◆` stays while it runs in the user's transaction.
    Running,
    /// The user's transaction is open: `◆`, the warning color.
    TxOpen,
    /// The connection was lost or the profile failed to connect, or the transaction is aborted
    /// (only ROLLBACK works): `!`, the warm color (never red: profiles use red for prod).
    Trouble,
}

/// The state of tab `tab`'s session.
pub fn state(app: &App, tab: &Tab) -> State {
    let e = &tab.exec;
    let failed = tab.profile.is_some_and(|p| app.conns.state(p) == NodeState::Failed);
    if e.state == SessionState::Lost || (e.user_tx() && e.tx_aborted) || (failed && e.session.is_none()) {
        State::Trouble
    } else if app.tab_busy(tab.id) {
        State::Running
    } else if e.user_tx() {
        State::TxOpen
    } else if e.session.is_some() {
        State::Connected
    } else {
        State::Idle
    }
}

/// Whether tab `tab`'s own session is connected (its number in the profile's color).
fn connected(tab: &Tab) -> bool {
    tab.exec.session.is_some() && tab.exec.state != SessionState::Lost
}

/// The warning mark and color of `state`, if any (`user_tx`: the user's transaction is open,
/// which a running statement does not hide).
fn warning_mark(state: State, user_tx: bool) -> Option<(&'static str, Color)> {
    let th = theme::cur();
    match state {
        State::TxOpen => Some((TX_OPEN, th.warning)),
        State::Running if user_tx => Some((TX_OPEN, th.warning)),
        State::Trouble => Some((TROUBLE, th.accent_warm)),
        _ => None,
    }
}

/// The name of the document a tab shows: a saved query's name, a console's
/// `console <n>`, a table tab's `schema.table`, a DDL tab's object (`database.` before either in
/// another database than the profile's own).
pub(crate) fn document_name(app: &App, tab: &Tab) -> String {
    if let Some(path) = tab.script() {
        return datarig_core::scripts::display_name(path, false).to_string();
    }
    let object = tab.doc.table.as_ref().map(|t| t.label()).or_else(|| tab.doc.ddl.as_ref().map(|d| d.label()));
    if let Some(name) = object {
        return match app.other_database(tab) {
            Some(db) => format!("{db}.{name}"),
            None => name,
        };
    }
    app.i18n.msg(&Msg::TabConsole { n: tab.doc.console_no.to_string() }).to_string()
}

/// One part of a tab's label: its text, style and what it is.
#[derive(Clone, Debug)]
struct Part {
    text: String,
    style: Style,
    kind: PartKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PartKind {
    Plain,
    /// The document's name (shortened first when the tabs do not fit).
    Document,
    /// The close button.
    Close,
}

/// One tab label, `doc_max` columns at most for the document's name.
fn label(app: &App, index: usize, tab: &Tab, active: bool, doc_max: usize) -> Vec<Part> {
    let th = theme::cur();
    let bg = if active { th.surface_alt } else { th.bg };
    let part = |text: String, style: Style| Part { text, style, kind: PartKind::Plain };
    let profile = tab.profile.and_then(|id| app.profiles.iter().find(|p| p.id == id));
    let state = profile.map(|_| state(app, tab));
    // The number in the profile's color once connected: which connection, at a glance; muted
    // while it is not; the spinner in its place (as wide) while a statement runs.
    let n = (index + 1).to_string();
    let (n, color) = match (profile, state) {
        (_, Some(State::Running)) => {
            let frame = tab
                .exec
                .running
                .map_or(crate::widgets::SPINNER[0], |r| crate::widgets::spinner_at(r.started, app.now()));
            (format!("{frame:<w$}", w = n.len()), th.accent)
        }
        (Some(p), _) if connected(tab) => (n, theme::profile_color(p.display_color())),
        _ => (n, th.fg_muted),
    };
    let mut num = Style::new().fg(color).bg(bg).add_modifier(Modifier::BOLD);
    if !active {
        num = num.remove_modifier(Modifier::BOLD);
    }
    let mark = |c: Color| Style::new().fg(c).bg(bg);
    let mut doc = Style::new().fg(if active { th.fg } else { th.fg_muted }).bg(bg);
    if active {
        doc = doc.add_modifier(Modifier::BOLD);
    }
    let mut parts = vec![part(format!(" {n} "), num)];
    let mut name = document_name(app, tab);
    if tab.is_table() && app.icons_on() {
        name = format!("{} {name}", icons::TABLE);
    }
    // A DDL tab says so, with its icon or in words.
    if tab.is_ddl() {
        name = match app.icons_on() {
            true => format!("{} {name}", icons::DDL),
            false => app.i18n.msg(&Msg::TabDdl { name }).to_string(),
        };
    }
    parts.push(Part { text: clip(&name, doc_max.max(1)), style: doc, kind: PartKind::Document });
    if tab.doc.conflict || tab.doc.save_error.is_some() {
        parts.push(part(format!(" {UNSAVED}"), mark(th.warning)));
    }
    match profile {
        Some(p) => {
            // A read-only policy, in words (policies have no color).
            if app.read_only(p.id) {
                let ro = Style::new().fg(th.fg).bg(bg).add_modifier(Modifier::BOLD);
                parts.push(part(format!(" {}", app.i18n.label(Label::TabReadOnly)), ro));
            }
            if let Some((m, c)) = state.and_then(|s| warning_mark(s, tab.exec.user_tx())) {
                parts.push(part(format!(" {m}"), mark(c)));
            }
        }
        None => {
            let l = if tab.doc.recovered { Label::TabRecovered } else { Label::TabUnbound };
            parts.push(part(format!(" {}", app.i18n.label(l)), mark(th.fg_dim)));
        }
    }
    parts.push(part(" ".to_string(), mark(th.fg_dim)));
    parts.push(Part {
        text: CLOSE.to_string(),
        style: mark(if active { th.fg_muted } else { th.fg_dim }),
        kind: PartKind::Close,
    });
    parts.push(part(" ".to_string(), mark(th.fg_dim)));
    parts
}

fn label_width(parts: &[Part]) -> usize {
    parts.iter().map(|p| width(&p.text)).sum()
}

/// The tabs `first..=last` to show so that the active one is visible, within `room` columns.
fn window(widths: &[usize], active: usize, room: usize) -> (usize, usize) {
    let (mut first, mut last) = (active, active);
    let mut used = widths[active];
    loop {
        let right = last + 1 < widths.len() && used + widths[last + 1] <= room;
        if right {
            last += 1;
            used += widths[last];
        }
        let left = first > 0 && used + widths[first - 1] <= room;
        if left {
            first -= 1;
            used += widths[first];
        }
        if !right && !left {
            return (first, last);
        }
    }
}

/// Where a column of the tab bar leads when clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabHit {
    /// The tab at this index.
    Tab(usize),
    /// A scroll mark (`‹` / `›`): the nearest hidden tab on its side, by index.
    More(usize),
    /// The close button (`×`) of the tab at this index.
    Close(usize),
}

/// The longest a document's name is drawn when the tabs do not fit, tried in turn.
const DOC_STEPS: [usize; 4] = [24, 16, 10, 6];

/// Draw the tab bar; returns what each drawn stretch of columns leads to (`[x0, x1)`), exactly
/// as drawn (a clipped tab counts for the columns it got).
pub(crate) fn draw_tab_bar(app: &App, area: Rect, buf: &mut Buffer) -> Vec<(u16, u16, TabHit)> {
    let th = theme::cur();
    let hover = match app.pointer_hover() {
        Some(crate::app::hover::PointerOn::Tab(h)) => Some(h),
        _ => None,
    };
    let mut hits = Vec::new();
    buf.set_style(area, Style::new().bg(th.bg));
    let total = area.width as usize;
    let active = app.tabs.active_index();
    // Full names when they fit; else shorter ones (the number, RO, mark and × stay).
    let make = |max: usize| -> Vec<Vec<Part>> {
        app.tabs.iter().enumerate().map(|(i, t)| label(app, i, t, i == active, max)).collect()
    };
    let mut labels = make(usize::MAX);
    for max in DOC_STEPS {
        if labels.iter().map(|l| label_width(l)).sum::<usize>() <= total {
            break;
        }
        labels = make(max);
    }
    // The active tab alone wider than the bar: its name gives way to the rest of it.
    let rest = label_width(&labels[active])
        - labels[active].iter().filter(|p| p.kind == PartKind::Document).map(|p| width(&p.text)).sum::<usize>();
    if rest + 1 < total && label_width(&labels[active]) > total {
        labels[active] = label(app, active, app.tabs.iter().nth(active).expect("active tab"), true, total - rest);
    }
    let widths: Vec<usize> = labels.iter().map(|l| label_width(l)).collect();
    let (first, last) = if widths.iter().sum::<usize>() <= total {
        (0, labels.len() - 1)
    } else {
        // One column on each side for the scroll marks.
        window(&widths, active, total.saturating_sub(2))
    };
    let arrow = Style::new().fg(th.accent).bg(th.bg).add_modifier(Modifier::BOLD);
    let mark = |h: TabHit| if hover == Some(h) { arrow.patch(crate::widgets::pointer_style()) } else { arrow };
    let mut x = area.x;
    let end = area.x + area.width;
    if first > 0 {
        buf.set_string(x, area.y, "‹", mark(TabHit::More(first - 1)));
        hits.push((x, x + 1, TabHit::More(first - 1)));
        x += 1;
    }
    let stop = if last + 1 < labels.len() { end.saturating_sub(1) } else { end };
    for (i, parts) in labels[first..=last].iter().enumerate() {
        let index = first + i;
        let start = x;
        for p in parts {
            if x >= stop {
                break;
            }
            let s = clip(&p.text, (stop - x) as usize);
            let from = x;
            // Under the pointer: the `×` stands out; the rest of the tab is underlined.
            let style = match (p.kind == PartKind::Close, hover) {
                (true, Some(TabHit::Close(i))) if i == index => p.style.patch(crate::widgets::pointer_style()),
                (false, Some(TabHit::Tab(i))) if i == index => p.style.add_modifier(Modifier::UNDERLINED),
                _ => p.style,
            };
            buf.set_stringn(x, area.y, &s, (stop - x) as usize, style);
            x += width(&s) as u16;
            // The close button has a hit of its own, exactly where it is drawn.
            if p.kind == PartKind::Close && x > from {
                if from > start {
                    hits.push((start, from, TabHit::Tab(index)));
                }
                hits.push((from, x, TabHit::Close(index)));
            }
        }
        let body_from = hits.last().filter(|h| h.2 == TabHit::Close(index)).map_or(start, |h| h.1);
        if x > body_from {
            hits.push((body_from, x, TabHit::Tab(index)));
        }
    }
    if last + 1 < labels.len() {
        buf.set_string(end - 1, area.y, "›", mark(TabHit::More(last + 1)));
        hits.push((end - 1, end, TabHit::More(last + 1)));
    }
    hits
}

#[cfg(test)]
mod tests;
