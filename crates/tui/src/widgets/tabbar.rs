//! The tab bar above the editor: each tab is `<n> <document> [RO] [<mark>] ×`. The number carries
//! the tab's connection: drawn in the profile's color while its session is connected (prod and
//! dev stay apart without a chip; the profile's name is on the editor's first line), muted while
//! it is not (the normal idle state: sessions open lazily), and replaced by the status bar's
//! spinner while a statement runs or waits. The document is a console's `console <n>`, a saved
//! query's name or a table tab's `schema.table` (in the color of its connection's environment
//! when that has one, muted on an inactive tab), `*` marks a text that could not be saved, the
//! environment's badge (`PROD`) follows as a chip of its color, and the lock (`RO` with icons
//! off) in the read-only mark's blue marks read-only rules. A mark after it warns, by shape
//! first ([`State`]): `◆` in the warning color while the user's transaction is open, `!` for a
//! lost connection, a failed profile or an aborted transaction; a tab with nothing to warn
//! about has no mark. `×` closes the tab as `Ctrl+W` does. A tab without a connection says so
//! in the mark's place. When the tabs do not fit, the documents' names are shortened first,
//! then the bar scrolls to keep the active tab visible and `‹` / `›` show that more tabs are
//! hidden. The active tab is on the theme's fill for it (`tab_active`), the others on the bar's
//! background. A tab dragged by the mouse (`app::tab_drag`) is drawn in the theme's selection
//! style where the pointer holds it, over its neighbours, which leave a gap where it would land
//! (the bar then scrolls to keep that place visible); tabs it moves slide to their places.

use crate::app::tab_drag;
use crate::app::{App, NodeState, SessionState, Tab, TabId};
use crate::icons;
use crate::text::{clip, grapheme_width, width};
use crate::theme;
use datarig_core::i18n::{Label, Msg};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use unicode_segmentation::UnicodeSegmentation;

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

/// The number of tab `index` as the tab bar shows it, and its color: the profile's color once
/// its session is connected (which connection, at a glance), muted while it is not, the spinner
/// in its place (as wide) while a statement runs.
pub(crate) fn number(app: &App, index: usize, tab: &Tab) -> (String, Color) {
    let th = theme::cur();
    let profile = tab.profile.and_then(|id| app.profiles.iter().find(|p| p.id == id));
    let state = profile.map(|_| state(app, tab));
    let n = (index + 1).to_string();
    match (profile, state) {
        (_, Some(State::Running)) => {
            let frame = tab
                .exec
                .running
                .map_or(crate::widgets::SPINNER[0], |r| crate::widgets::spinner_at(r.started, app.now()));
            (format!("{frame:<w$}", w = n.len()), th.accent)
        }
        (Some(p), _) if connected(tab) => (n, theme::profile_color(p.display_color())),
        _ => (n, th.fg_muted),
    }
}

/// The marks after a tab's name, each with a blank before it, on background `bg`: `*` (its
/// text could not be saved), then on a profile its environment's badge, `RO` and the warning
/// mark of its session.
pub(crate) fn marks(app: &App, tab: &Tab, bg: Color) -> Vec<(String, Style)> {
    let th = theme::cur();
    let mark = |c: Color| Style::new().fg(c).bg(bg);
    let mut out = Vec::new();
    if tab.doc.conflict || tab.doc.save_error.is_some() {
        out.push((format!(" {UNSAVED}"), mark(th.warning)));
    }
    if let Some(p) = tab.profile.and_then(|id| app.profiles.iter().find(|p| p.id == id)) {
        // The environment's badge, a chip of its color.
        if let Some(chip) = app.profile_environment(p.id).and_then(App::badge_chip) {
            out.push((" ".to_string(), mark(th.fg)));
            out.push(chip);
        }
        // Read-only (the effective rules): the lock, or `RO` with icons off, in the read-only
        // mark's color.
        if app.read_only(p.id) {
            let ro =
                Style::new().fg(crate::app::environments::read_only_color(&th)).bg(bg).add_modifier(Modifier::BOLD);
            let text = if app.icons_on() {
                icons::READ_ONLY.to_string()
            } else {
                app.i18n.label(Label::TabReadOnly).to_string()
            };
            out.push((format!(" {text}"), ro));
        }
        if let Some((m, c)) = warning_mark(state(app, tab), tab.exec.user_tx()) {
            out.push((format!(" {m}"), mark(c)));
        }
    }
    out
}

/// What a tab without a profile says in place of its connection: `recovered` (a console found
/// in the state directory) or `no connection`; `None` on a profile.
pub(crate) fn unbound(app: &App, tab: &Tab) -> Option<Label> {
    if tab.profile.is_some_and(|id| app.profiles.iter().any(|p| p.id == id)) {
        return None;
    }
    Some(if tab.doc.recovered { Label::TabRecovered } else { Label::TabUnbound })
}

/// One tab label, `doc_max` columns at most for the document's name, numbered as place
/// `place` (where a dragged tab shows it). The active tab is on the theme's fill for it, its
/// colors made to read there (the name as text, the rest as marks) or replaced by the fill's own
/// text color; another tab under the pointer (its body or its `×`) is on the theme's hover
/// background. The environment's badge keeps its own colors on both.
fn label(app: &App, index: usize, place: usize, tab: &Tab, active: bool, doc_max: usize) -> Vec<Part> {
    let th = theme::cur();
    let hovered = !active
        && matches!(
            app.pointer_hover(),
            Some(crate::app::hover::PointerOn::Tab(TabHit::Tab(i) | TabHit::Close(i))) if i == index
        );
    let bg = if active { th.tab_active_bg() } else { th.bg };
    let part = |text: String, style: Style| Part { text, style, kind: PartKind::Plain };
    let (n, color) = number(app, place, tab);
    let mut num = Style::new().fg(color).bg(bg).add_modifier(Modifier::BOLD);
    if !active {
        num = num.remove_modifier(Modifier::BOLD);
    }
    let mark = |c: Color| Style::new().fg(c).bg(bg);
    // The document's name: in the color of its connection's environment when that has one
    // (muted on an inactive tab), else the body text (muted on an inactive tab); bold on the
    // active tab.
    let env = tab.profile.filter(|id| app.profile(*id).is_some()).and_then(|id| app.name_env_color(id));
    let mut doc = match (env, active) {
        (Some(c), true) => Style::new().fg(th.name_color(Some(c))),
        (Some(c), false) => th.name_muted(th.name_color(Some(c))),
        (None, true) => Style::new().fg(th.fg),
        (None, false) => Style::new().fg(th.fg_muted),
    }
    .bg(bg);
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
    parts.extend(marks(app, tab, bg).into_iter().map(|(text, style)| part(text, style)));
    if let Some(l) = unbound(app, tab) {
        parts.push(part(format!(" {}", app.i18n.label(l)), mark(th.fg_dim)));
    }
    parts.push(part(" ".to_string(), mark(th.fg_dim)));
    parts.push(Part {
        text: CLOSE.to_string(),
        style: mark(if active { th.fg_muted } else { th.fg_dim }),
        kind: PartKind::Close,
    });
    parts.push(part(" ".to_string(), mark(th.fg_dim)));
    if hovered {
        for p in parts.iter_mut().filter(|p| p.style.bg == Some(bg)) {
            p.style = p.style.patch(crate::widgets::hover_style());
        }
    }
    if active {
        for p in parts.iter_mut().filter(|p| p.style.bg == Some(bg)) {
            p.style = match (th.tab_active.fg, p.style.fg) {
                (Some(_), _) => p.style.patch(th.tab_active),
                (None, Some(c)) => {
                    let min = if p.kind == PartKind::Document { 4.5 } else { 3.0 };
                    p.style.fg(theme::Theme::readable_on(c, bg, min))
                }
                (None, None) => p.style,
            };
        }
    }
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

/// The tabs `first..=last` to show while a tab is dragged: from the first one the last frame
/// showed (`shown`), scrolled only as far as tab `anchor` needs, so the bar moves one tab at a
/// time as the pointer goes past an end.
fn drag_window(widths: &[usize], anchor: usize, shown: usize, room: usize) -> (usize, usize) {
    let fill_right = |first: usize| {
        let (mut last, mut used) = (first, widths[first]);
        while last + 1 < widths.len() && used + widths[last + 1] <= room {
            last += 1;
            used += widths[last];
        }
        last
    };
    let fill_left = |last: usize| {
        let (mut first, mut used) = (last, widths[last]);
        while first > 0 && used + widths[first - 1] <= room {
            first -= 1;
            used += widths[first];
        }
        first
    };
    let first = shown.min(widths.len() - 1);
    if anchor < first {
        return (anchor, fill_right(anchor));
    }
    let last = fill_right(first);
    if anchor <= last { (first, last) } else { (fill_left(anchor), anchor) }
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

/// What the tab bar drew last: a dragged tab follows the pointer by it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BarGeom {
    /// The tabs in the order drawn (indexes; a dragged tab at the place it is shown at).
    pub order: Vec<usize>,
    /// Each tab's width, by index.
    pub widths: Vec<u16>,
    /// The places shown: `order[first..=last]`.
    pub first: usize,
    pub last: usize,
    /// The columns tabs are drawn in: `[left, stop)` (the scroll marks are outside).
    pub left: u16,
    pub stop: u16,
    /// Where each shown tab belongs (its first column), by index; a sliding or dragged tab may
    /// be drawn elsewhere for now.
    pub at: Vec<Option<u16>>,
    /// Where each shown tab was drawn (its first column), by index.
    pub drawn: Vec<Option<i32>>,
}

/// One column of the tab bar while its tabs are put together, later ones over earlier ones
/// (a dragged tab over its neighbours).
#[derive(Clone, Debug)]
struct Cell {
    /// Empty for the right half of a wide character.
    symbol: String,
    style: Style,
    hit: TabHit,
    half: Half,
}

/// Which part of a character a [`Cell`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Half {
    /// A character one column wide.
    Whole,
    /// The first column of a wide character (Hangul, CJK).
    Left,
    /// Its second column.
    Right,
}

/// Write tab `index`'s label (`parts`) from column `x` into `line` (columns `[x0, stop)`), as
/// [`Buffer::set_stringn`] would: what falls left of `x0` or right of `stop` is cut off.
fn put_tab(line: &mut [Option<Cell>], x0: u16, stop: u16, x: i32, parts: &[Part], index: usize, hover: Option<TabHit>) {
    let th = theme::cur();
    let mut x = x;
    for p in parts {
        if x >= i32::from(stop) {
            break;
        }
        let room = usize::try_from(i32::from(stop) - x).unwrap_or(0);
        let s = clip(&p.text, room);
        // Under the pointer: only the `×` changes, to the theme's error color.
        let style = match (p.kind == PartKind::Close, hover) {
            (true, Some(TabHit::Close(i))) if i == index => p.style.fg(th.error),
            _ => p.style,
        };
        let hit = if p.kind == PartKind::Close { TabHit::Close(index) } else { TabHit::Tab(index) };
        for g in s.graphemes(true) {
            let w = grapheme_width(g);
            if w > 0 && !g.contains(char::is_control) {
                for k in 0..w {
                    let Ok(at) = usize::try_from(x + k as i32 - i32::from(x0)) else { continue };
                    let Some(cell) = line.get_mut(at) else { continue };
                    let (symbol, half) = match (k, w) {
                        (0, 1) => (g.to_string(), Half::Whole),
                        (0, _) => (g.to_string(), Half::Left),
                        _ => (String::new(), Half::Right),
                    };
                    *cell = Some(Cell { symbol, style, hit, half });
                }
            }
            x += w as i32;
        }
    }
}

/// A wide character partly covered by another tab is not drawn by halves: what is left of it
/// is a blank of its style.
fn mend(line: &mut [Option<Cell>]) {
    let half = |c: &Option<Cell>| c.as_ref().map(|c| c.half);
    for k in 0..line.len() {
        let orphan = match half(&line[k]) {
            Some(Half::Left) => line.get(k + 1).map(half) != Some(Some(Half::Right)),
            Some(Half::Right) => k == 0 || half(&line[k - 1]) != Some(Half::Left),
            _ => false,
        };
        if let Some(c) = line[k].as_mut().filter(|_| orphan) {
            c.symbol = " ".to_string();
            c.half = Half::Whole;
        }
    }
}

/// Draw the tab bar; returns what each drawn stretch of columns leads to (`[x0, x1)`), exactly
/// as drawn (a clipped tab counts for the columns it got, a dragged tab for those it covers).
/// What it drew is kept for a drag to follow the pointer by ([`BarGeom`]), and the tabs a drag
/// moved slide to their places ([`tab_drag::TabSlide`]).
pub(crate) fn draw_tab_bar(app: &mut App, area: Rect, buf: &mut Buffer) -> Vec<(u16, u16, TabHit)> {
    let th = theme::cur();
    let hover = match app.pointer_hover() {
        Some(crate::app::hover::PointerOn::Tab(h)) => Some(h),
        _ => None,
    };
    let mut hits = Vec::new();
    buf.set_style(area, Style::new().bg(th.bg));
    let total = area.width as usize;
    let n = app.tabs.len();
    let active = app.tabs.active_index();
    // A dragged tab: its place, the place it is shown at among the others, whether the pointer
    // is too far from the bar, the pointer's column and where it holds the tab.
    let drag = app.tab_drag_live().map(|d| (d.from(), d.slot, d.far || d.keys, d.x, d.grab));
    // Moved by keys: drawn at its place (sliding there), over the others.
    let keyed = app.tab_drag_live().filter(|d| d.keys).map(|d| d.from());
    let order = match drag {
        Some((from, slot, ..)) => tab_drag::shown_order(n, from, slot),
        None => (0..n).collect::<Vec<_>>(),
    };
    // The numbers are places: of the order shown.
    let mut place = vec![0; n];
    for (k, i) in order.iter().enumerate() {
        place[*i] = k;
    }
    // Full names when they fit; else shorter ones (the number, RO, mark and × stay).
    let app_ref: &App = app;
    let make = |max: usize| -> Vec<Vec<Part>> {
        app_ref.tabs.iter().enumerate().map(|(i, t)| label(app_ref, i, place[i], t, i == active, max)).collect()
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
        let tab = app_ref.tabs.iter().nth(active).expect("active tab");
        labels[active] = label(app_ref, active, place[active], tab, true, total - rest);
    }
    if let Some((from, ..)) = drag {
        // Lifted: the theme's selection style, but for the environment's badge.
        let fill = th.tab_active_bg();
        for p in labels[from].iter_mut().filter(|p| p.style.bg == Some(fill) || p.style.bg == Some(th.bg)) {
            p.style = p.style.patch(th.selection);
        }
    }
    let ids: Vec<TabId> = app_ref.tabs.iter().map(|t| t.id).collect();
    let widths: Vec<usize> = labels.iter().map(|l| label_width(l)).collect();
    let shown: Vec<usize> = order.iter().map(|i| widths[*i]).collect();
    let (first, last) = if shown.iter().sum::<usize>() <= total {
        (0, n - 1)
    } else {
        // One column on each side for the scroll marks. In view: the active tab, or while one
        // is dragged the place it is shown at (the bar scrolls as it goes past an end).
        let room = total.saturating_sub(2);
        match drag {
            Some((_, slot, ..)) => drag_window(&shown, slot.min(n - 1), app_ref.tab_geom.first, room),
            None => window(&shown, active, room),
        }
    };
    let end = area.x + area.width;
    let left = area.x + u16::from(first > 0);
    let stop = if last + 1 < n { end.saturating_sub(1) } else { end };
    // Where each shown tab belongs.
    let mut at = vec![None; n];
    let mut x = usize::from(left);
    for k in first..=last {
        at[order[k]] = Some(u16::try_from(x).unwrap_or(u16::MAX));
        x += shown[k];
    }
    let tabs_end = i32::try_from(x.min(usize::from(stop))).unwrap_or(i32::MAX);
    // The dragged tab is drawn where the pointer holds it, within the tabs shown.
    let held = drag.filter(|d| !d.2).map(|(from, _, _, px, grab)| {
        let w = i32::try_from(widths[from]).unwrap_or(i32::MAX);
        let x = (i32::from(px) - i32::from(grab)).clamp(i32::from(left), (tabs_end - w).max(i32::from(left)));
        (from, x)
    });
    let places: Vec<(TabId, i32, bool)> = order[first..=last]
        .iter()
        .map(|&i| match held {
            Some((from, x)) if from == i => (ids[i], x, true),
            _ => (ids[i], at[i].map_or(0, i32::from), false),
        })
        .collect();
    let now = app.now();
    let xs = app.tab_slide.place(&places, now);
    let mut drawn = vec![None; n];
    for (k, x) in xs.iter().enumerate() {
        drawn[order[first + k]] = Some(*x);
    }
    app.tab_geom = BarGeom {
        drawn,
        order: order.clone(),
        widths: widths.iter().map(|w| u16::try_from(*w).unwrap_or(u16::MAX)).collect(),
        first,
        last,
        left,
        stop,
        at,
    };
    // Tabs in place first, sliding ones over them, the dragged (or just dropped) one on top.
    let top = held.map(|h| h.0).or(keyed).or_else(|| app.tab_slide.top.and_then(|t| app.tabs.position(t)));
    let mut draw: Vec<usize> = (0..places.len()).collect();
    draw.sort_by_key(|&k| (Some(order[first + k]) == top, app.tab_slide.moving(places[k].0)));
    let mut line: Vec<Option<Cell>> = vec![None; usize::from(stop.saturating_sub(left))];
    for k in draw {
        let i = order[first + k];
        put_tab(&mut line, left, stop, xs[k], &labels[i], i, hover);
    }
    mend(&mut line);
    let arrow = Style::new().fg(th.accent).bg(th.bg).add_modifier(Modifier::BOLD);
    let mark = |h: TabHit| if hover == Some(h) { arrow.patch(crate::widgets::pointer_style()) } else { arrow };
    if first > 0 {
        let h = TabHit::More(order[first - 1]);
        buf.set_string(area.x, area.y, "‹", mark(h));
        hits.push((area.x, area.x + 1, h));
    }
    let mut run: Option<(u16, u16, TabHit)> = None;
    for (k, c) in line.iter().enumerate() {
        let x = left + k as u16;
        let Some(c) = c else {
            hits.extend(run.take());
            continue;
        };
        if c.half == Half::Right {
            // Hidden by the wide character before it, as `Buffer::set_stringn` leaves it.
            buf[(x, area.y)].reset();
        } else {
            buf[(x, area.y)].set_symbol(&c.symbol).set_style(c.style);
        }
        match &mut run {
            Some(r) if r.2 == c.hit => r.1 = x + 1,
            _ => {
                hits.extend(run.take());
                run = Some((x, x + 1, c.hit));
            }
        }
    }
    hits.extend(run);
    if last + 1 < n {
        let h = TabHit::More(order[last + 1]);
        buf.set_string(end - 1, area.y, "›", mark(h));
        hits.push((end - 1, end, h));
    }
    hits
}

#[cfg(test)]
mod tests;
