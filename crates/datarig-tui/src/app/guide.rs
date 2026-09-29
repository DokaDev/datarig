//! Key guide: the which-key popup of the `Space` leader,
//! the keyboard help overlay and the hint line of the status bar. All three are computed from
//! the keymap and the action registry, so remapped keys show up everywhere.

use super::*;
use crate::keymap::{Child, HINTS, LEADER, Target, keys, parse_keys};
use crate::widgets::text_input::InputResult;
use ratatui::layout::Position;

/// The which-key popup appears when no key follows the leader within this time.
pub const WHICH_KEY_DELAY: Duration = Duration::from_millis(300);

/// The which-key popup: the keys that may follow `prefix`.
pub struct WhichKey {
    /// The context the sequence was typed in (whose bindings are listed).
    pub origin: Ctx,
    /// The keys typed so far, starting with the leader.
    pub prefix: Vec<KeyChord>,
}

/// One entry of the which-key popup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WhichKeyItem {
    pub key: String,
    pub label: Localized,
    pub group: bool,
    /// The action can run now (`ActionSpec::when`); groups are always enabled.
    pub enabled: bool,
    /// Shown apart below the other entries: `Space ?`, the keyboard help of this screen.
    pub footer: bool,
}

/// The keyboard help overlay: one list of every context. The sections of the context it was
/// opened from (and its ancestors) come first and start open; the other contexts follow
/// closed. The `/` filter searches every context and opens each section with a match.
pub struct Help {
    /// The context it was opened from.
    pub origin: Ctx,
    /// Open sections (while the filter is empty).
    pub expanded: Vec<Ctx>,
    pub filter: TextInput,
    /// The `/` filter has the keyboard (`overlay.help.filter`).
    pub filtering: bool,
    /// Index among the rows (section headers included).
    pub selected: usize,
    /// First visible row, kept by the renderer.
    pub scroll: usize,
    pub view_h: usize,
    /// Screen area of the rows, kept by the renderer (mouse clicks).
    pub list: Rect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelpRow {
    /// A context's header; `count` entries follow when it is `open`.
    Section {
        ctx: Ctx,
        open: bool,
        count: usize,
    },
    Entry(HelpItem),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelpItem {
    pub action: Action,
    pub label: Localized,
    pub keys: String,
    /// Keys of the action the editor uses instead (`?` in vim Normal).
    pub editor_keys: String,
    pub enabled: bool,
}

fn is_leader(keys: &[KeyChord]) -> bool {
    parse_keys(LEADER).ok().is_some_and(|l| keys.first() == l.first())
}

fn join(keys: &[Vec<KeyChord>]) -> String {
    keys.iter().map(|k| keys::label(k)).collect::<Vec<_>>().join(" / ")
}

impl App {
    /// After a key was resolved: start (or stop) the which-key timer of a leader sequence.
    pub(super) fn track_leader(&mut self, pending: bool) {
        self.leader_at = (pending && is_leader(self.key_state.pending())).then(Instant::now);
    }

    /// A leader sequence that matched nothing: say so (`Esc` just drops the sequence).
    pub(super) fn leader_unbound(&mut self, seq: &[KeyChord]) {
        if seq.len() > 1 && is_leader(seq) && seq.last().is_some_and(|k| k.code != KeyCode::Esc) {
            self.flash(Notice::new(Msg::WhichkeyUnbound { keys: keys::label(seq) }, Level::Warning));
        }
    }

    /// Timer part of the which-key popup (called from `on_tick`).
    pub(super) fn which_key_tick(&mut self, now: Instant) {
        let Some(at) = self.leader_at else { return };
        if now.saturating_duration_since(at) < WHICH_KEY_DELAY {
            return;
        }
        self.leader_at = None;
        let ctx = self.key_context();
        let prefix = self.key_state.pending().to_vec();
        if prefix.is_empty() || !is_leader(&prefix) || self.key_state.ctx() != Some(ctx) {
            return;
        }
        self.key_state.clear();
        self.overlays.push(Overlay::WhichKey(WhichKey { origin: ctx, prefix }));
    }

    /// A key while the which-key popup is open: continue the sequence, `Backspace` one level
    /// up, `Esc` closes. A key that leads nowhere closes the popup with a notice.
    pub(super) fn which_key_key(&mut self, k: KeyChord, repeat: bool) {
        let Some(w) = self.overlays.which_key() else { return };
        if repeat {
            return;
        }
        let (origin, mut seq) = (w.origin, w.prefix.clone());
        match k.code {
            KeyCode::Esc => self.overlays.close(OverlayKind::WhichKey),
            KeyCode::Backspace => {
                seq.pop();
                match self.overlays.which_key_mut() {
                    Some(w) if !seq.is_empty() => w.prefix = seq,
                    _ => self.overlays.close(OverlayKind::WhichKey),
                }
            }
            _ => {
                seq.push(k);
                match self.keymap.resolve_seq(origin, &seq, self.key_env()) {
                    (Some(Target::Action(a)), _) => {
                        // An item that cannot run now does nothing.
                        if (action::spec(a).when)(self) {
                            self.overlays.close(OverlayKind::WhichKey);
                            self.dispatch(a);
                        }
                    }
                    (None, true) => {
                        if let Some(w) = self.overlays.which_key_mut() {
                            w.prefix = seq;
                        }
                    }
                    // The key that opens the command line here (`:`, as the footer says) opens it.
                    _ if self.keymap.resolve_seq(origin, &[k], self.key_env()).0
                        == Some(Target::Action(Action::OpenCommands)) =>
                    {
                        self.overlays.close(OverlayKind::WhichKey);
                        self.dispatch(Action::OpenCommands);
                    }
                    _ => {
                        self.overlays.close(OverlayKind::WhichKey);
                        self.leader_unbound(&seq);
                    }
                }
            }
        }
    }

    /// Title (the keys typed so far and the group's label) and entries of the which-key popup.
    pub fn which_key_items(&self) -> Option<(Localized, Vec<WhichKeyItem>)> {
        let w = self.overlays.which_key()?;
        let env = self.key_env();
        let items = self
            .keymap
            .children(w.origin, &w.prefix, env)
            .into_iter()
            .map(|(k, child)| match child {
                Child::Action(a) => {
                    let spec = action::spec(a);
                    // The help of this screen sits apart in the leader's own popup.
                    let footer = a == Action::Help && w.prefix.len() == 1;
                    let item = WhichKeyItem {
                        key: k.label(),
                        label: self.i18n.label(if footer { Label::WhichkeyHelp } else { spec.label }),
                        group: false,
                        enabled: (spec.when)(self),
                        footer,
                    };
                    (matches!(a, Action::GotoTab(_)), item)
                }
                Child::Group(label) => {
                    let item = WhichKeyItem {
                        key: k.label(),
                        // A group marker around its localized label.
                        label: Localized::verbatim(format!("+{}", label.map_or("…", |l| l.text(self.i18n.lang)))),
                        group: true,
                        enabled: true,
                        footer: false,
                    };
                    (false, item)
                }
            })
            .collect::<Vec<_>>();
        // `Space 1` … `Space 9` take one line: `1…9  Go to tab N`.
        let (goto, items): (Vec<_>, Vec<_>) = items.into_iter().partition(|(g, _)| *g);
        let mut items: Vec<WhichKeyItem> = items.into_iter().map(|(_, i)| i).collect();
        if let (Some((_, first)), Some((_, last))) = (goto.first(), goto.last()) {
            let key = if goto.len() > 1 { format!("{}…{}", first.key, last.key) } else { first.key.clone() };
            let at = items.iter().position(|i| i.key.as_str() > key.as_str()).unwrap_or(items.len());
            let enabled = goto.iter().any(|(_, i)| i.enabled);
            let label = self.i18n.label(Label::WhichkeyGotoTab);
            items.insert(at, WhichKeyItem { key, label, group: false, enabled, footer: false });
        }
        let (mut items, footer): (Vec<_>, Vec<_>) = items.into_iter().partition(|i| !i.footer);
        items.extend(footer);
        // The keys typed so far, then the group's localized label.
        let keys = keys::label(&w.prefix);
        let title = match self.keymap.group_label(w.origin, &w.prefix) {
            Some(l) => Localized::verbatim(format!("{keys} — {}", l.text(self.i18n.lang))),
            None => Localized::verbatim(keys),
        };
        Some((title, items))
    }

    pub(super) fn key_env(&self) -> KeyEnv {
        KeyEnv { selection: self.tab().editor.mode == Mode::Visual }
    }

    /// Open the keyboard help of the current context: its sections open, the other contexts
    /// closed below (`all`: every section open).
    pub(super) fn open_help(&mut self, all: bool) {
        if self.layout.too_small {
            return;
        }
        self.overlays.close(OverlayKind::WhichKey);
        let origin = self.overlays.help().map_or_else(|| self.key_context(), |h| h.origin);
        let expanded = if all { Ctx::ALL.to_vec() } else { origin.chain() };
        if let Some(h) = self.overlays.help_mut() {
            h.expanded = expanded;
            h.scroll = 0;
        } else {
            let t = self.tab_mut();
            t.popup = None;
            t.completion_due = None;
            self.overlays.push(Overlay::Help(Help {
                origin,
                expanded,
                filter: TextInput::default(),
                filtering: false,
                selected: 0,
                scroll: 0,
                view_h: 1,
                list: Rect::default(),
            }));
        }
        self.help_select_first_entry();
    }

    /// The rows of the keyboard help: a header per context, then (when the section is open)
    /// its actions and their keys. The context the help was opened from and its ancestors come
    /// first, with the keys an inner context hides left out; every other context of the
    /// current editor mode follows. Sections without an entry matching the filter are left out.
    pub fn help_rows(&self) -> Vec<HelpRow> {
        let Some(h) = self.overlays.help() else { return Vec::new() };
        let query = h.filter.text().to_string();
        let chain = h.origin.chain();
        let others = Ctx::for_mode(self.editor_mode).into_iter().filter(|c| !chain.contains(c));
        let sections = chain.iter().map(|&c| (c, Some(h.origin))).chain(others.map(|c| (c, None)));
        let mut rows = Vec::new();
        for (ctx, from) in sections {
            let items: Vec<HelpItem> = self
                .keymap
                .section(ctx, from)
                .into_iter()
                // Only the keys this terminal can send (Ctrl+Enter needs the kitty keyboard
                // protocol).
                .map(|mut e| {
                    e.keys.retain(|k| crate::keymap::works(k, self.enhanced_keys));
                    e.editor_keys.retain(|k| crate::keymap::works(k, self.enhanced_keys));
                    e
                })
                .filter(|e| !e.keys.is_empty() || !e.editor_keys.is_empty())
                .map(|e| {
                    let spec = action::spec(e.action);
                    HelpItem {
                        action: e.action,
                        label: self.i18n.label(spec.label),
                        keys: join(&e.keys),
                        editor_keys: join(&e.editor_keys),
                        enabled: (spec.when)(self),
                    }
                })
                .filter(|i| {
                    let spec = action::spec(i.action);
                    [i.label.as_str(), spec.label.text(Lang::En), spec.id, i.keys.as_str()]
                        .iter()
                        .any(|t| action::fuzzy_score(&query, t).is_some())
                })
                .collect();
            if items.is_empty() {
                continue;
            }
            let open = !query.is_empty() || h.expanded.contains(&ctx);
            rows.push(HelpRow::Section { ctx, open, count: items.len() });
            if open {
                rows.extend(items.into_iter().map(HelpRow::Entry));
            }
        }
        rows
    }

    fn help_move(&mut self, d: isize) {
        let n = self.help_rows().len();
        if let Some(h) = self.overlays.help_mut() {
            h.selected = (h.selected as isize + d).clamp(0, n.saturating_sub(1) as isize) as usize;
        }
    }

    /// Select the first action (after opening, or when the filter changed).
    pub(super) fn help_select_first_entry(&mut self) {
        let first = self.help_rows().iter().position(|r| matches!(r, HelpRow::Entry(_))).unwrap_or(0);
        if let Some(h) = self.overlays.help_mut() {
            h.selected = first;
            h.scroll = 0;
        }
    }

    /// Open or close the section `ctx` (`None`: toggle) and select its header.
    fn help_set_open(&mut self, ctx: Ctx, open: Option<bool>) {
        let header = self.help_rows().iter().position(|r| matches!(r, HelpRow::Section { ctx: c, .. } if *c == ctx));
        let Some(h) = self.overlays.help_mut() else { return };
        let is_open = h.expanded.contains(&ctx);
        if open.unwrap_or(!is_open) {
            if !is_open {
                h.expanded.push(ctx);
            }
        } else {
            h.expanded.retain(|c| *c != ctx);
        }
        if let Some(i) = header {
            h.selected = i;
        }
    }

    /// The section the row `i` belongs to.
    fn help_section_of(rows: &[HelpRow], i: usize) -> Option<Ctx> {
        rows.iter().take(i + 1).rev().find_map(|r| if let HelpRow::Section { ctx, .. } = r { Some(*ctx) } else { None })
    }

    /// `Enter` (or a click) on row `i`: a header opens or closes its section; an action runs
    /// when it can run now (otherwise nothing happens).
    fn help_activate(&mut self, i: usize, run: bool) {
        match self.help_rows().into_iter().nth(i) {
            Some(HelpRow::Section { ctx, .. }) => self.help_set_open(ctx, None),
            Some(HelpRow::Entry(item)) if run && item.enabled => {
                self.overlays.close(OverlayKind::Help);
                self.dispatch(item.action);
            }
            _ => {}
        }
    }

    /// Keys of the keyboard help list (`overlay.help`).
    pub(super) fn help_keys(&mut self, keys: &[KeyChord], repeat: bool) {
        for k in keys {
            let Some(h) = self.overlays.help_mut() else { return };
            let page = h.view_h.max(1) as isize;
            let sel = h.selected;
            match k.code {
                KeyCode::Char('j') | KeyCode::Down => self.help_move(1),
                KeyCode::Char('k') | KeyCode::Up => self.help_move(-1),
                KeyCode::PageDown => self.help_move(page),
                KeyCode::PageUp => self.help_move(-page),
                _ if repeat => {}
                KeyCode::Esc | KeyCode::Char('q') => self.overlays.close(OverlayKind::Help),
                KeyCode::Char('/') => h.filtering = true,
                KeyCode::Enter => self.help_activate(sel, true),
                KeyCode::Char('l') | KeyCode::Right => {
                    if let Some(HelpRow::Section { ctx, .. }) = self.help_rows().into_iter().nth(sel) {
                        self.help_set_open(ctx, Some(true));
                    }
                }
                KeyCode::Char('h') | KeyCode::Left => {
                    if let Some(ctx) = Self::help_section_of(&self.help_rows(), sel) {
                        self.help_set_open(ctx, Some(false));
                    }
                }
                _ => {}
            }
        }
    }

    /// Mouse in the keyboard help: a click selects a row (and opens or closes a section), the
    /// wheel moves the selection.
    pub(super) fn help_mouse(&mut self, m: MouseEvent) {
        let Some(h) = self.overlays.help() else { return };
        let (list, scroll) = (h.list, h.scroll);
        match m.kind {
            MouseEventKind::ScrollDown => self.help_move(WHEEL_STEP),
            MouseEventKind::ScrollUp => self.help_move(-WHEEL_STEP),
            MouseEventKind::Down(MouseButton::Left) if list.contains(Position::new(m.column, m.row)) => {
                let i = scroll + usize::from(m.row - list.y);
                if i < self.help_rows().len() {
                    if let Some(h) = self.overlays.help_mut() {
                        h.selected = i;
                    }
                    self.help_activate(i, false);
                }
            }
            _ => {}
        }
    }

    /// Keys of the help's `/` filter (`overlay.help.filter`, text input).
    pub(super) fn help_filter_key(&mut self, key: KeyEvent) {
        let Some(h) = self.overlays.help_mut() else { return };
        match key.code {
            KeyCode::Esc => {
                h.filtering = false;
                h.filter.set("");
                self.help_select_first_entry();
            }
            KeyCode::Enter => h.filtering = false,
            KeyCode::Down => self.help_move(1),
            KeyCode::Up => self.help_move(-1),
            _ => {
                if h.filter.handle_key(&key) == InputResult::Changed {
                    self.help_select_first_entry();
                }
            }
        }
    }

    /// The hint line: the most relevant keys of the current context (key, short label), best
    /// first, then `Space` when the leader works here. The run key is the one the terminal
    /// can send (`Ctrl+Enter` only with the kitty keyboard protocol).
    pub fn hints(&self) -> Vec<(String, Localized)> {
        let ctx = self.key_context();
        let mut wanted: Vec<(&str, Label)> = Vec::new();
        if self.tab_busy(self.tab().id) {
            wanted.push(("query.cancel", Label::HintCancel));
        }
        if let Some((_, list)) = HINTS.iter().find(|(c, _)| *c == ctx) {
            wanted.extend(list.iter().copied());
        }
        let mut out: Vec<(String, Localized)> = wanted
            .into_iter()
            .filter_map(|(id, label)| {
                let spec = action::by_id(id)?;
                if !(spec.when)(self) {
                    return None;
                }
                let k = self.keymap.hint_keys(spec.action, ctx, self.enhanced_keys)?;
                Some((keys::label(&k), self.i18n.label(label)))
            })
            .collect();
        let leader = parse_keys(LEADER).unwrap_or_default();
        if !self.keymap.children(ctx, &leader, self.key_env()).is_empty() {
            out.push((keys::label(&leader), self.i18n.label(Label::HintLeader)));
        }
        out
    }
}
