//! Acting on a session from the Monitor tab's Sessions view: `c` cancels the selected
//! session's statement, `X` ends the session. What may be offered and how it is asked is the
//! core's ([`core_act::decide`]): a refusal is said and nothing is sent; a cancel asks one line
//! on the status line (only `y` sends), anything that warns opens a dialog where `Enter` keeps
//! the session as it is and only `y` sends. The row of one of this app's query tabs that runs a
//! statement is cancelled by that tab's own cancel instead, nothing sent by the monitor.
//!
//! What is sent carries the session and its statement as the row showed them; one asked about
//! before the monitor reconnected is dropped, never sent. Sent is not done: the server's answer
//! is said as it is, and reads of the session 1, 3 and 10 seconds after the answer tell what
//! became of it, whether the tab is shown, paused or hidden. Every action stays in the tab's
//! action list (`L`) for the tab's life, in memory only.

use super::*;
use crate::app::edit::{AskAction, StatusAsk};
use crate::app::overlay::{Confirm, ConfirmAction};
use crate::app::tabs::{OwnAsk, SessionState as TabState};
use crate::widgets::monitor::age;
use datarig_core::driver::{DbError, OwnSession};
use datarig_core::monitor::action::{self as core_act, Denied, NotSent, Stale, Via};
use datarig_core::monitor::{
    ActionKind, ActionOutcome, ActionTarget, Ask, Confirmation, Decision, FollowUp, Need, Refusal, Warning,
};

/// When the reads that follow an action up are due, counted from its answer.
const FOLLOW_UPS: [Duration; 3] = [Duration::from_secs(1), Duration::from_secs(3), Duration::from_secs(10)];

/// The most characters of a statement's first line the dialog shows.
const EXCERPT: usize = 80;

/// The most characters of it the action list shows (last on its line).
const LIST_EXCERPT: usize = 32;

/// `text` cut to `n` characters, `…` at the cut.
fn cut(text: &str, n: usize) -> String {
    match text.chars().count() > n {
        true => format!("{}…", text.chars().take(n - 1).collect::<String>()),
        false => text.to_string(),
    }
}

/// What the dialog and the action list say of the row an action is on, as it was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowInfo {
    pub id: i64,
    pub user: String,
    pub database: String,
    /// The first line of its statement, cut to [`EXCERPT`] characters.
    pub statement: String,
}

impl RowInfo {
    fn of(r: &SessionRow) -> Self {
        let line = r.query.as_ref().and_then(|q| q.text.lines().map(str::trim).find(|l| !l.is_empty()));
        let line = line.unwrap_or_default();
        let statement = cut(line, EXCERPT);
        let or_unknown = |s: &Option<String>| s.clone().unwrap_or_else(|| "?".into());
        Self { id: r.key.id, user: or_unknown(&r.user), database: or_unknown(&r.database), statement }
    }
}

/// An action asked about and not sent yet, read on the monitor connection of `generation`.
#[derive(Clone, Debug)]
pub struct Asked {
    generation: u64,
    kind: ActionKind,
    target: ActionTarget,
    row: RowInfo,
}

/// What `y` does to a status-line question of a Monitor tab.
#[derive(Debug)]
pub enum MonitorAsk {
    /// Send `pending` on Monitor tab `tab`'s connection.
    Send { tab: TabId, pending: Asked },
}

impl MonitorAsk {
    /// The Monitor tab and its connection's generation the question was asked on.
    fn asked_on(&self) -> (TabId, u64) {
        match self {
            MonitorAsk::Send { tab, pending } => (*tab, pending.generation),
        }
    }
}

/// An action of a Monitor tab, from the moment it is sent: what it was on, what the server
/// answered and what the reads after it showed.
#[derive(Clone, Debug)]
pub struct ActionEntry {
    pub kind: ActionKind,
    pub target: ActionTarget,
    pub row: RowInfo,
    /// When it was sent (the app's clock).
    pub at: Instant,
    request: u64,
    /// The server's answer as said (`None`: not answered yet).
    pub answer: Option<Notice>,
    /// What the last read after the answer showed.
    pub confirmation: Option<Confirmation>,
    /// Why the reads after it tell nothing (a failed read, no connection).
    pub note: Option<Msg>,
    /// The reads still due, soonest first.
    follow: Vec<Instant>,
    /// The read in flight.
    follow_request: Option<u64>,
}

impl ActionEntry {
    /// It still waits: for its answer, or for the reads that follow it up.
    pub fn open(&self) -> bool {
        self.answer.is_none() || self.follow_request.is_some() || !self.follow.is_empty()
    }
}

impl MonitorTab {
    /// The action that still waits on the session of `key`, if any.
    pub fn action_on(&self, key: &SessionKey) -> Option<&ActionEntry> {
        self.actions.iter().rev().find(|e| e.open() && e.target.key == *key)
    }

    /// The connection went: an answer it owed is lost, the reads it would have sent are not
    /// sent, and what was asked about on it is dropped.
    pub(super) fn actions_disconnected(&mut self) {
        for e in &mut self.actions {
            if e.answer.is_none() {
                e.answer = Some(Notice::new(Label::MonitorActLost, Level::Warning));
            }
            if e.follow_request.is_some() || !e.follow.is_empty() {
                e.follow.clear();
                e.follow_request = None;
                e.confirmation = Some(Confirmation::Unknown);
                e.note = Some(Label::MonitorActNoConnection.into());
            }
        }
        self.asked = None;
    }

    /// When the next read that follows an action up is due.
    pub(super) fn follow_due(&self) -> Option<Instant> {
        self.actions.iter().filter(|e| e.follow_request.is_none()).filter_map(|e| e.follow.first().copied()).min()
    }
}

/// The few words a menu shows next to an action `refusal` refuses.
fn refusal_short(refusal: &Refusal) -> Label {
    match refusal {
        Refusal::ReadOnly => Label::MonitorActOffReadOnly,
        Refusal::MonitorConnection => Label::MonitorActOffMonitor,
        Refusal::Hidden(_) => Label::MonitorActOffHidden,
        Refusal::NoStatement => Label::MonitorActOffNoStatement,
        Refusal::Background => Label::MonitorActOffBackground,
        Refusal::PooledIdle => Label::MonitorActOffPooledIdle,
        Refusal::Untracked => Label::MonitorActOffUntracked,
    }
}

/// What a dialog says of `w`.
fn warning_label(w: Warning) -> Label {
    match w {
        Warning::MySqlMaintenance => Label::MonitorActWarnMaintenance,
        Warning::MySqlMayLeaveHalfDone => Label::MonitorActWarnHalfDone,
        Warning::MySqlTerminateKeepsNonTx => Label::MonitorActWarnKeepsNonTx,
        Warning::PoolerSharedBackend => Label::MonitorActWarnPooler,
        Warning::WeakIdentity => Label::MonitorActWarnWeak,
    }
}

/// What a read after an action on session `id` showed, said.
fn confirmation_msg(c: Confirmation, id: String) -> Msg {
    match c {
        Confirmation::Gone => Msg::MonitorActConfirmGone { id },
        Confirmation::EndedAfterCancel => Msg::MonitorActConfirmEndedAfterCancel { id },
        Confirmation::Interrupted => Msg::MonitorActConfirmInterrupted { id },
        Confirmation::StillRunning => Msg::MonitorActConfirmStillRunning { id },
        Confirmation::StillThere => Msg::MonitorActConfirmStillThere { id },
        Confirmation::Unknown => Msg::MonitorActConfirmUnknown { id },
    }
}

/// After it, nothing more is read: the session went, or its statement ended.
fn settled(c: Confirmation) -> bool {
    matches!(c, Confirmation::Gone | Confirmation::Interrupted | Confirmation::EndedAfterCancel)
}

impl App {
    /// What profile `profile`'s monitor may do: its environment as it connected or as it is
    /// now is read-only (one that cannot be read counts as read-only), or it is not connected.
    fn monitor_read_only(&self, profile: ProfileId) -> bool {
        self.conns.get(profile).and_then(|c| c.resolved.as_ref()).is_none_or(|cfg| self.session_read_only(cfg))
    }

    /// The query tab of profile `profile` whose own session row `row` is: its session said it
    /// reaches the server directly as that session (never a pooler's, never one not known), and
    /// the row agrees with that session. The id alone is only a hint (behind a load-balanced
    /// endpoint or after a failover the monitor may read another server, whose session of the
    /// same id is someone else's): the row must be a client session of the profile's user (when
    /// the profile names one), in the tab's database, showing the application name the app's
    /// query sessions connect with. A row that shows no application (MySQL's list has none) is
    /// never named.
    pub(crate) fn own_tab(&self, profile: ProfileId, row: &SessionRow) -> Option<TabId> {
        if row.monitor || row.kind != datarig_core::monitor::SessionKind::Client {
            return None;
        }
        let user = self.conns.get(profile).and_then(|c| c.resolved.as_ref()).map(|c| c.user.as_str());
        if user.filter(|u| !u.is_empty()).is_some_and(|u| row.user.as_deref() != Some(u)) {
            return None;
        }
        let application = datarig_core::driver::ConnectOptions::new(self.page_size, SessionRole::Query, &self.instance)
            .application_name;
        if row.application.as_deref() != Some(application.as_str()) {
            return None;
        }
        let ours = |t: &Tab| match t.exec.own {
            OwnAsk::Known(OwnSession::Direct { id }, _) => id == row.key.id,
            _ => false,
        };
        // The database the tab's session works in: as the server said, else as the tab asks.
        let database = |t: &Tab| match &t.exec.context {
            Some((db, _)) => db.clone(),
            None => t.context.database.clone().unwrap_or_else(|| self.own_database(profile)),
        };
        let in_database = |t: &Tab| row.database.as_deref().is_some_and(|d| !d.is_empty() && d == database(t));
        let t = self.tabs.iter().find(|t| {
            t.is_query() && t.profile == Some(profile) && t.exec.session.is_some() && ours(t) && in_database(t)
        })?;
        Some(t.id)
    }

    /// The visible Monitor tab of profile `profile` is ready: each query tab of the profile
    /// whose session is connected is asked which server session it is, once per session; one
    /// whose answer told nothing is asked again when idle, outside a transaction block, at
    /// least `interval` after the answer. One ask per tab at a time.
    pub(super) fn monitor_ask_own(&mut self, profile: ProfileId, interval: Duration, now: Instant) {
        let due = |t: &Tab| match t.exec.own {
            OwnAsk::NotAsked => true,
            OwnAsk::Known(OwnSession::NotKnown, at) => {
                !t.exec.in_block && t.exec.running.is_none() && now >= at + interval
            }
            _ => false,
        };
        let ids: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|t| t.is_query() && t.profile == Some(profile) && t.exec.state == TabState::Ready && due(t))
            .map(|t| t.id)
            .collect();
        for id in ids {
            let request = self.tabs.next_generation();
            if let Some(t) = self.tabs.get_mut(id)
                && let Some(s) = &t.exec.session
            {
                s.send(DbCommand::ServerSession { request });
                t.exec.own = OwnAsk::Asked(request);
            }
        }
    }

    /// Query tab `id` is about to send a run: while a Monitor tab of its profile is open (shown
    /// or not), its session is asked which server session it is first, unless it said so
    /// already (or cannot say inside a transaction block).
    pub(in crate::app) fn monitor_ask_own_at_run(&mut self, id: TabId) {
        let Some(t) = self.tabs.get(id).filter(|t| t.is_query()) else { return };
        let Some(profile) = t.profile else { return };
        let due = match t.exec.own {
            OwnAsk::NotAsked => true,
            OwnAsk::Known(OwnSession::NotKnown, _) => !t.exec.in_block,
            _ => false,
        };
        if !due || t.exec.session.is_none() || !self.tabs.iter().any(|m| m.is_monitor() && m.profile == Some(profile)) {
            return;
        }
        let request = self.tabs.next_generation();
        if let Some(t) = self.tabs.get_mut(id)
            && let Some(s) = &t.exec.session
        {
            s.send(DbCommand::ServerSession { request });
            t.exec.own = OwnAsk::Asked(request);
        }
    }

    /// What a need of `need` reads as.
    fn need_text(&self, need: &Need) -> String {
        let m = match need {
            Need::Grant(g) => Msg::MonitorNeedGrant { grant: (*g).to_string() },
            Need::Setting(s) => Msg::MonitorNeedSetting { setting: (*s).to_string() },
            Need::Version(v) => Msg::MonitorNeedVersion { version: (*v).to_string() },
        };
        self.i18n.msg(&m).to_string()
    }

    /// Why an action on session `id` is refused, in full.
    fn refusal_msg(&self, refusal: &Refusal, id: i64) -> Msg {
        let id = id.to_string();
        match refusal {
            Refusal::ReadOnly => Label::MonitorActRefuseReadOnly.into(),
            Refusal::MonitorConnection => Label::MonitorActRefuseMonitor.into(),
            Refusal::Hidden(need) => Msg::MonitorActRefuseHidden { need: self.need_text(need) },
            Refusal::NoStatement => Msg::MonitorActRefuseNoStatement { id },
            Refusal::Background => Msg::MonitorActRefuseBackground { id },
            Refusal::PooledIdle => Label::MonitorActRefusePooledIdle.into(),
            Refusal::Untracked => Msg::MonitorActRefuseUntracked { id },
        }
    }

    /// What the server's refusal `d` means, in words.
    fn denied_text(&self, d: &Denied) -> String {
        let m = match d {
            Denied::Grant(g) => Msg::MonitorActWhyGrant { grants: g.join(" or ") },
            Denied::SuperuserTarget => Label::MonitorActWhySuperuser.into(),
            Denied::SystemUserTarget => Label::MonitorActWhySystemUser.into(),
            Denied::GrantOrSystemUser(g) => Msg::MonitorActWhyGrantOrSystemUser { grants: g.join(" or ") },
            Denied::ServiceSession => Label::MonitorActWhyService.into(),
            Denied::ProcedureDenied(p) => Msg::MonitorActWhyProcedure { procedure: (*p).to_string() },
            Denied::RdsLongUserName => Label::MonitorActWhyRdsLongUser.into(),
        };
        self.i18n.msg(&m).to_string()
    }

    /// What `kind` on the active Monitor tab's selected session would do: `Ok` with the
    /// decision, the row, its format and the generation it was read on; `Err` with what is said
    /// when there is nothing to act on.
    fn monitor_decision(&self, kind: ActionKind) -> Result<(Decision, SessionRow, MonitorFormat, u64), Notice> {
        let select = || Notice::new(Label::MonitorActSelect, Level::Info);
        let m = self.monitor_of(self.tab().id).ok_or_else(select)?;
        let profile = self.tab().profile.ok_or_else(select)?;
        // The server pane covers the list: nothing is done on a row the user cannot see.
        if m.view != MonitorView::Sessions || m.server_info.is_some() {
            return Err(select());
        }
        let format = m.format().cloned().ok_or(Notice::new(Label::MonitorActNotReady, Level::Info))?;
        let row = m.selected_row().cloned().ok_or_else(select)?;
        // The list was read on an earlier connection (the server may have restarted since, and
        // given its ids to other sessions): nothing is done on it until it is read again.
        if m.read_on != m.generation {
            return Err(self.monitor_stale());
        }
        let read_only = self.monitor_read_only(profile);
        Ok((core_act::decide(&format, &row, read_only, kind), row, format, m.generation))
    }

    /// Why the action menu shows `kind` dimmed for the selected session (`None`: it may be
    /// asked for, or there is nothing selected to act on).
    pub(crate) fn monitor_action_off(&self, kind: ActionKind) -> Option<super::super::menu::Off> {
        if self.monitor_of(self.tab().id).is_some_and(|m| m.format().is_some() && m.read_on != m.generation) {
            return Some(super::super::menu::Off::new(Label::MonitorActOffStale, self.monitor_stale()));
        }
        match self.monitor_decision(kind) {
            Ok((Decision::Refuse(r), row, _, _)) => {
                let notice = Notice::new(self.refusal_msg(&r, row.key.id), Level::Warning);
                Some(super::super::menu::Off::new(refusal_short(&r), notice))
            }
            _ => None,
        }
    }

    /// What is said of a list read before the monitor reconnected.
    fn monitor_stale(&self) -> Notice {
        let key = self.key_for(Action::Monitor(MonitorAction::ReadNow), Ctx::Monitor);
        Notice::new(Msg::MonitorActStale { key }, Level::Warning)
    }

    /// `c` / `X`: `kind` on the active Monitor tab's selected session, refused (said), asked on
    /// the status line or in a dialog; the question names the tab when the session is one of
    /// this app's own.
    pub(super) fn monitor_act(&mut self, kind: ActionKind) {
        let (decision, row, format, generation) = match self.monitor_decision(kind) {
            Ok(d) => d,
            Err(n) => return self.flash(n),
        };
        let tab = self.tab().id;
        let Some(profile) = self.tab().profile else { return };
        let own = self.own_tab(profile, &row);
        // One of this app's own tabs: the question names it. The cancel is the same guarded one
        // (the server checks the statement); the tab's own cancel is never used here, as it
        // names no statement and could stop whichever one the tab runs by then.
        let own_name = own.and_then(|q| self.tabs.get(q)).map(|q| crate::widgets::tabbar::document_name(self, q));
        let ask = match decision {
            Decision::Refuse(r) => return self.flash(Notice::new(self.refusal_msg(&r, row.key.id), Level::Warning)),
            Decision::Offer(ask) => ask,
        };
        let Some(target) = core_act::target(&format, &row) else { return };
        let pending = Asked { generation, kind, target, row: RowInfo::of(&row) };
        match ask {
            Ask::Line => {
                let time = self.monitor_of(tab).and_then(|m| m.sessions_time());
                let id = row.key.id.to_string();
                let (user, database) = (pending.row.user.clone(), pending.row.database.clone());
                let age = time.and_then(|t| query_age(&row, t)).map(age).unwrap_or_else(|| "?".into());
                let forms = match own_name {
                    Some(tab) => vec![
                        self.i18n.msg(&Msg::MonitorActAskOwn { id, tab: tab.clone() }).to_string(),
                        self.i18n.msg(&Msg::MonitorActAskOwnShort { tab }).to_string(),
                    ],
                    None => vec![
                        self.i18n.msg(&Msg::MonitorActAskCancel { id: id.clone(), user, database, age }).to_string(),
                        self.i18n.msg(&Msg::MonitorActAskCancelShort { id }).to_string(),
                    ],
                };
                self.monitor_ask(forms, MonitorAsk::Send { tab, pending });
            }
            Ask::Modal { warnings } => self.monitor_dialog(tab, pending, &row, &format, &warnings, own),
        }
    }

    /// Ask `forms` (longest first) on the status line; `y` does `ask`.
    fn monitor_ask(&mut self, forms: Vec<String>, ask: MonitorAsk) {
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.key_state.clear();
        self.ask = Some(StatusAsk { forms, tone: Level::Warning, action: AskAction::Monitor(Box::new(ask)) });
    }

    /// The dialog that asks for `pending` (kept on Monitor tab `tab` until answered): the
    /// session, what is known of it, what the action does beyond it, what it warns of, and
    /// whether it is one of this app's tabs (`own`).
    fn monitor_dialog(
        &mut self,
        tab: TabId,
        pending: Asked,
        row: &SessionRow,
        format: &MonitorFormat,
        warnings: &[Warning],
        own: Option<TabId>,
    ) {
        let kind = pending.kind;
        let time = self.monitor_of(tab).and_then(|m| m.sessions_time());
        let mut details: Vec<Msg> = Vec::new();
        if let Some(client) = row.client.clone().filter(|c| !c.is_empty()) {
            details.push(Msg::MonitorActClient { client });
        }
        details.push(Msg::MonitorActState { state: state_text(&self.i18n, row) });
        if let Some(a) = time.and_then(|t| tx_age(row, t)) {
            details.push(Msg::MonitorActTxAge { time: age(a) });
        }
        if !pending.row.statement.is_empty() {
            details.push(Msg::MonitorActStatement { statement: pending.row.statement.clone() });
        }
        // MySQL's warning that it keeps what is not transactional says this too.
        if kind == ActionKind::Terminate && matches!(format, MonitorFormat::Postgres(_)) {
            details.push(Label::MonitorActRollsBack.into());
        }
        details.extend(warnings.iter().map(|w| Msg::from(warning_label(*w))));
        if let Some(t) = own.and_then(|q| self.tabs.get(q)) {
            let tab = crate::widgets::tabbar::document_name(self, t);
            details.push(match kind {
                ActionKind::Cancel => Msg::MonitorActOwnTabCancel { tab },
                ActionKind::Terminate => Msg::MonitorActOwnTab { tab },
            });
        }
        let (title, keys) = match kind {
            ActionKind::Cancel => (Label::MonitorActCancelTitle, Label::MonitorActCancelKeys),
            ActionKind::Terminate => (Label::MonitorActEndTitle, Label::MonitorActEndKeys),
        };
        let text = Msg::MonitorActSession {
            id: pending.row.id.to_string(),
            user: pending.row.user.clone(),
            database: pending.row.database.clone(),
        };
        if let Some(m) = self.monitor_mut(tab) {
            m.asked = Some(pending);
        }
        self.tab_mut().popup = None;
        self.overlays.close(OverlayKind::Commands);
        self.overlays.close(OverlayKind::WhichKey);
        self.key_state.clear();
        self.overlays.push(Overlay::Confirm(Confirm {
            title,
            text,
            details,
            keys,
            action: ConfirmAction::MonitorAction { tab, kind },
            folder: None,
            path: None,
            tx: None,
            buttons: Default::default(),
        }));
    }

    /// `y` in the dialog of Monitor tab `tab`: what it asked about is sent.
    pub(in crate::app) fn monitor_confirmed(&mut self, tab: TabId) {
        match self.monitor_mut(tab).and_then(|m| m.asked.take()) {
            Some(p) => self.monitor_send(tab, p),
            None => self.flash(Notice::new(Label::MonitorActReconnected, Level::Warning)),
        }
    }

    /// `n`, `Esc` or `Enter` in the dialog of Monitor tab `tab`: nothing is sent.
    pub(in crate::app) fn monitor_declined(&mut self, tab: TabId) {
        if let Some(m) = self.monitor_mut(tab) {
            m.asked = None;
        }
    }

    /// The status-line question `a` is still about what is on screen: its Monitor tab active
    /// and on the connection it was asked on, no dialog over it.
    pub(in crate::app) fn monitor_ask_current(&self, a: &MonitorAsk) -> bool {
        let (tab, generation) = a.asked_on();
        !self.tabs.is_empty()
            && !self.modal_open()
            && self.tab().id == tab
            && self.monitor_of(tab).is_some_and(|m| m.generation == generation)
    }

    /// What is said when the question `a` is dropped for not being current.
    pub(in crate::app) fn monitor_ask_moved(&self, a: &MonitorAsk) -> Label {
        let (tab, generation) = a.asked_on();
        match self.monitor_of(tab) {
            Some(m) if m.generation == generation => Label::MonitorActAskMoved,
            _ => Label::MonitorActReconnected,
        }
    }

    /// `y` to the status-line question `a`.
    pub(in crate::app) fn monitor_ask_yes(&mut self, a: MonitorAsk) {
        if !self.monitor_ask_current(&a) {
            let said = self.monitor_ask_moved(&a);
            return self.ask_ended(said);
        }
        match a {
            MonitorAsk::Send { tab, pending } => self.monitor_send(tab, pending),
        }
    }

    /// Send `p` on Monitor tab `tab`'s connection, when it is still the one it was read on and
    /// the profile is not read-only now; the action list takes it.
    fn monitor_send(&mut self, tab: TabId, p: Asked) {
        let Some(profile) = self.tabs.get(tab).and_then(|t| t.profile) else { return };
        if self.monitor_read_only(profile) {
            return self.flash(Notice::new(Label::MonitorActRefuseReadOnly, Level::Warning));
        }
        let now = self.now();
        let Some(m) = self.monitor_mut(tab) else { return };
        if m.generation != p.generation || m.read_on != p.generation || m.format().is_none() {
            return self.flash(Notice::new(Label::MonitorActReconnected, Level::Warning));
        }
        let Some(session) = m.session.as_ref() else {
            return self.flash(Notice::new(Label::MonitorActReconnected, Level::Warning));
        };
        m.seq += 1;
        let request = m.seq;
        session.send(DbCommand::MonitorAction { request, kind: p.kind, target: p.target.clone() });
        m.actions.push(ActionEntry {
            kind: p.kind,
            target: p.target,
            row: p.row,
            at: now,
            request,
            answer: None,
            confirmation: None,
            note: None,
            follow: Vec::new(),
            follow_request: None,
        });
    }

    /// The answer to action `request` of Monitor tab `tab`: said and kept; an answer that says
    /// something was sent plans the reads that follow it up.
    pub(super) fn monitor_action_done(&mut self, tab: TabId, request: u64, outcome: ActionOutcome, now: Instant) {
        let failed = match &outcome {
            ActionOutcome::Failed(e) => self.db_error_text(e),
            _ => String::new(),
        };
        let refused = match &outcome {
            ActionOutcome::NotSent(NotSent::Refused(d)) => self.denied_text(d),
            _ => String::new(),
        };
        let Some(m) = self.monitor_mut(tab) else { return };
        let Some(e) = m.actions.iter_mut().find(|e| e.request == request && e.answer.is_none()) else { return };
        let id = e.row.id.to_string();
        let cancel = e.kind == ActionKind::Cancel;
        let (msg, level, follow): (Msg, Level, bool) = match outcome {
            ActionOutcome::NotSent(why) => {
                let msg = match why {
                    NotSent::Stale(Stale::SessionGone) => Msg::MonitorActGone { id },
                    NotSent::Stale(Stale::SessionReplaced) => Msg::MonitorActReplaced { id },
                    NotSent::Stale(Stale::StatementChanged) => Msg::MonitorActChanged { id },
                    NotSent::Stale(Stale::CannotTell) => Msg::MonitorActCannotTell { id },
                    NotSent::ReadOnly => Label::MonitorActNotSentReadOnly.into(),
                    NotSent::OwnConnection => Label::MonitorActNotSentOwn.into(),
                    NotSent::Refused(_) => Msg::MonitorActRefused { why: refused },
                };
                (msg, Level::Warning, false)
            }
            ActionOutcome::NotDelivered => (Msg::MonitorActNotDelivered { id }, Level::Info, true),
            ActionOutcome::Acknowledged { via } => {
                let msg = match (via, cancel) {
                    (Via::Standard, true) => Msg::MonitorActCancelSent { id },
                    (Via::Standard, false) => Msg::MonitorActEndSent { id },
                    (Via::Procedure(p), true) => Msg::MonitorActCancelSentBy { id, procedure: p.to_string() },
                    (Via::Procedure(p), false) => Msg::MonitorActEndSentBy { id, procedure: p.to_string() },
                };
                (msg, Level::Info, true)
            }
            ActionOutcome::OutcomeUnknown => (Msg::MonitorActUnknown { id }, Level::Info, true),
            ActionOutcome::Failed(_) => (Msg::MonitorActFailed { error: failed }, Level::Error, false),
        };
        let said = Notice::new(msg, level);
        e.answer = Some(said.clone());
        if follow {
            e.follow = FOLLOW_UPS.iter().map(|d| now + *d).collect();
        }
        self.flash(said);
    }

    /// Monitor tab `tab`'s reads that follow its actions up and are due at `now`, one per
    /// action at a time; without a connection to read on they are given up.
    pub(super) fn monitor_follow_ups(&mut self, tab: TabId, now: Instant) {
        let Some(m) = self.monitor_mut(tab) else { return };
        let live = m.format().is_some();
        let MonitorTab { session, seq, actions, .. } = m;
        for e in actions.iter_mut() {
            if e.follow_request.is_some() || e.follow.first().is_none_or(|d| *d > now) {
                continue;
            }
            let Some(s) = session.as_ref().filter(|_| live) else {
                e.follow.clear();
                e.confirmation = Some(Confirmation::Unknown);
                e.note = Some(Label::MonitorActNoConnection.into());
                continue;
            };
            *seq += 1;
            s.send(DbCommand::MonitorFollowUp { request: *seq, target: e.target.clone() });
            e.follow.remove(0);
            e.follow_request = Some(*seq);
        }
    }

    /// The answer to read `request` after an action of Monitor tab `tab`: what it shows is
    /// kept, and said when it changed or when it is the last read.
    pub(super) fn monitor_followed(&mut self, tab: TabId, request: u64, result: Result<FollowUp, DbError>) {
        let error = result.as_ref().err().map(|e| self.db_error_text(e));
        let Some(m) = self.monitor_mut(tab) else { return };
        let Some(e) = m.actions.iter_mut().find(|e| e.follow_request == Some(request)) else { return };
        e.follow_request = None;
        let follow = match result {
            Ok(f) => f,
            Err(_) => {
                e.note = Some(Msg::MonitorActFollowFailed { error: error.unwrap_or_default() });
                return;
            }
        };
        let c = core_act::confirm(e.kind, &e.target, &follow);
        let changed = e.confirmation != Some(c);
        e.confirmation = Some(c);
        e.note = None;
        if settled(c) {
            e.follow.clear();
        }
        let last = e.follow.is_empty();
        let level = if last && !settled(c) { Level::Warning } else { Level::Info };
        let msg = confirmation_msg(c, e.row.id.to_string());
        if changed || last {
            self.flash(Notice::new(msg, level));
        }
    }

    /// The soonest read that follows an action up, of any Monitor tab (hidden ones too).
    pub(super) fn monitor_follow_due(&self) -> Option<Instant> {
        self.tabs.iter().filter_map(|t| t.doc.monitor.as_ref()).filter_map(MonitorTab::follow_due).min()
    }

    /// What the action list says of `e`: when, what, on which session, what the reads after it
    /// showed, the server's answer, and its statement.
    pub fn action_line(&self, e: &ActionEntry, now: Instant) -> String {
        let when = now.saturating_duration_since(e.at).as_micros() as i64;
        let when = self.i18n.msg(&Msg::MonitorAgo { time: age(when) }).to_string();
        let what = self.i18n.label(match e.kind {
            ActionKind::Cancel => Label::MonitorActKindCancel,
            ActionKind::Terminate => Label::MonitorActKindEnd,
        });
        let answer = match &e.answer {
            Some(n) => self.i18n.msg(&n.msg).to_string(),
            None => self.i18n.label(Label::MonitorActSending).to_string(),
        };
        let mut line = format!("{when}  {what} {}  {}@{}", e.row.id, e.row.user, e.row.database);
        // The latest first: what the reads showed, then the answer.
        if let Some(c) = e.confirmation {
            line.push_str("  · ");
            line.push_str(&self.i18n.msg(&confirmation_msg(c, e.row.id.to_string())));
        }
        line.push_str("  · ");
        line.push_str(&answer);
        if let Some(n) = &e.note {
            line.push_str(" · ");
            line.push_str(&self.i18n.msg(n));
        }
        if !e.row.statement.is_empty() {
            line.push_str("  ");
            line.push_str(&cut(&e.row.statement, LIST_EXCERPT));
        }
        line
    }
}
