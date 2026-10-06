//! The plan of a statement, in the results pane's Plan tab.
//!
//! `query.explain` and `query.explain_analyze` run the statement under the cursor (or the one
//! statement selected) as `EXPLAIN (FORMAT JSON)` or `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)`
//! through the normal path of a run: the same read-only refusal, the same confirmation of
//! what may do harm, and the driver's rollback of what an `EXPLAIN ANALYZE` ran. Any result of
//! one row with one `json` column named `QUERY PLAN` that reads as a plan is shown as one, so
//! an `EXPLAIN (FORMAT JSON …)` typed by hand is too.
//!
//! A text `EXPLAIN` (any other format) stays rows, with a line under them that offers
//! `results.view_as_plan`: the statement that produced those rows (not the editor's text) is
//! asked again with `FORMAT JSON` and its other options kept, through the same path of a run.
//! Never by itself: with `ANALYZE` the statement runs again, so it asks first; a result whose
//! tab moved to another connection or database since is refused.
//!
//! A plan belongs to the run it came from, like its rows: a later run that delivers rows
//! replaces it; one that delivers none leaves it (from an earlier run). Its views are drawn
//! from the plan as it was read; switching views, the raw text and a copy never ask the server
//! again.

use super::*;
use datarig_core::sql::plan::{self, Plan};

/// How a plan is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanView {
    /// The node tree with its numbers per node (the default).
    Tree,
    /// Cards of what to look at first (the slowest node, the worst estimate, disk reads, the
    /// times) above a compact tree.
    Summary,
    /// One row per depth, each node as wide as its share of its parent's time, parents on top.
    Icicle,
    /// The icicle upside down: parents at the bottom.
    Flame,
    /// Each node's time from its start to its first and its last row, on one axis.
    Timeline,
    /// The rows each node passed up, as bands as thick as their number (a funnel).
    Rows,
    /// Rectangles whose area is each node's own time.
    Treemap,
    /// Boxes joined by lines, a large plan moved around in.
    Boxes,
    /// The text `psql` shows for `EXPLAIN`, written from the plan.
    Raw,
}

impl PlanView {
    /// Every view, in the order `v` goes through them.
    pub const ALL: [PlanView; 9] = [
        PlanView::Tree,
        PlanView::Summary,
        PlanView::Icicle,
        PlanView::Flame,
        PlanView::Timeline,
        PlanView::Rows,
        PlanView::Treemap,
        PlanView::Boxes,
        PlanView::Raw,
    ];

    pub fn label(self) -> Label {
        match self {
            PlanView::Tree => Label::PlanViewTree,
            PlanView::Summary => Label::PlanViewSummary,
            PlanView::Icicle => Label::PlanViewIcicle,
            PlanView::Flame => Label::PlanViewFlame,
            PlanView::Timeline => Label::PlanViewTimeline,
            PlanView::Rows => Label::PlanViewRows,
            PlanView::Treemap => Label::PlanViewTreemap,
            PlanView::Boxes => Label::PlanViewBoxes,
            PlanView::Raw => Label::PlanViewRaw,
        }
    }

    /// The view `delta` places away, around the ends.
    pub fn step(self, delta: isize) -> PlanView {
        let n = Self::ALL.len() as isize;
        let at = Self::ALL.iter().position(|v| *v == self).unwrap_or(0) as isize;
        Self::ALL[(at + delta).rem_euclid(n) as usize]
    }

    /// The tree's folds apply (the other views show every node).
    pub fn folds(self) -> bool {
        matches!(self, PlanView::Tree | PlanView::Summary | PlanView::Timeline | PlanView::Rows)
    }
}

/// What the Plan tab acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanAction {
    Down,
    Up,
    Top,
    Bottom,
    PageDown,
    PageUp,
    /// Open the selected node's children (the tree), or go to its first child.
    Expand,
    /// Close them, or go to the parent.
    Collapse,
    /// Show or hide the detail of the selected node.
    Detail,
    /// The next (`true`) or previous view.
    NextView(bool),
    View(PlanView),
    /// Move the view sideways (long lines, wide layouts).
    PanLeft,
    PanRight,
    /// The plan as `psql` shows it, or its JSON, to the clipboard.
    CopyText,
    CopyJson,
}

/// A plan as `psql` shows it: its lines, each with its node, and each node's first line.
pub struct RawText {
    pub lines: Vec<(String, Option<usize>)>,
    pub first: Vec<usize>,
    /// The widest line, in columns.
    pub widest: usize,
}

impl RawText {
    /// Node `i`'s own line (after the name of its subplan, when it has one).
    pub fn node_line(&self, plan: &Plan, i: usize) -> usize {
        self.first.get(i).copied().unwrap_or(0) + usize::from(plan.nodes.get(i).is_some_and(|n| n.subplan.is_some()))
    }
}

/// A tab's plan and how it is shown.
pub struct PlanTab {
    pub plan: Arc<Plan>,
    /// The run it came from (its query id) and the statement of that run.
    pub query: u64,
    pub index: usize,
    /// The JSON as the server sent it (copied as it is).
    pub json: Arc<str>,
    pub view: PlanView,
    pub selected: usize,
    /// Nodes whose children the tree hides.
    pub collapsed: Vec<bool>,
    /// The selected node's detail is shown.
    pub detail: bool,
    /// The first line of the view on screen (kept by the renderer so the selection shows).
    pub scroll: usize,
    /// Columns the view is moved to the left.
    pub pan: usize,
    /// Lines of the view on screen at the last frame (a page).
    pub page: usize,
    /// The wheel moved the view away from the selection: it is not brought back into view
    /// until a key moves the selection.
    pub detached: bool,
    /// The `psql` text, written once.
    raw: Option<Arc<RawText>>,
    /// Where the last frame drew each node (the pointer selects them).
    pub hits: Vec<(Rect, usize)>,
    /// Where the last frame drew each view's name.
    pub view_hits: Vec<(Rect, PlanView)>,
}

impl PlanTab {
    pub fn new(plan: Plan, query: u64, index: usize, json: &str) -> Self {
        let n = plan.nodes.len();
        Self {
            plan: Arc::new(plan),
            query,
            index,
            json: json.into(),
            view: PlanView::Tree,
            selected: 0,
            collapsed: vec![false; n],
            detail: false,
            scroll: 0,
            pan: 0,
            page: 1,
            detached: false,
            raw: None,
            hits: Vec::new(),
            view_hits: Vec::new(),
        }
    }

    /// The text `psql` shows, each line with its node.
    pub fn raw(&mut self) -> Arc<RawText> {
        let plan = &self.plan;
        self.raw
            .get_or_insert_with(|| {
                let lines = plan::text::lines(plan);
                let mut first = vec![0; plan.nodes.len()];
                for (at, (_, n)) in lines.iter().enumerate().rev() {
                    if let Some(n) = n {
                        first[*n] = at;
                    }
                }
                let widest = lines.iter().map(|(l, _)| crate::text::width(l)).max().unwrap_or(0);
                Arc::new(RawText { lines, first, widest })
            })
            .clone()
    }

    /// The nodes the view lists, in order: in the tree, not those under a closed node.
    pub fn visible(&self) -> Vec<usize> {
        let nodes = &self.plan.nodes;
        if !self.view.folds() {
            return (0..nodes.len()).collect();
        }
        let mut out = Vec::with_capacity(nodes.len());
        let mut i = 0;
        while i < nodes.len() {
            out.push(i);
            if self.collapsed.get(i).copied().unwrap_or(false) {
                // Skip its subtree: the nodes after it that are deeper.
                let d = nodes[i].depth;
                i += 1;
                while i < nodes.len() && nodes[i].depth > d {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        out
    }

    /// Select node `i`, opening the closed nodes above it.
    pub fn select(&mut self, i: usize) {
        if i >= self.plan.nodes.len() {
            return;
        }
        let mut p = self.plan.nodes[i].parent;
        while let Some(at) = p {
            self.collapsed[at] = false;
            p = self.plan.nodes[at].parent;
        }
        self.selected = i;
    }

    fn act(&mut self, a: PlanAction) {
        // A key brings the selection back into view; moving the view sideways keeps it where
        // it was put.
        self.detached = matches!(a, PlanAction::PanLeft | PlanAction::PanRight);
        let order = self.visible();
        let at = order.iter().position(|i| *i == self.selected).unwrap_or(0);
        let page = self.page.max(1);
        let to = |k: usize| order.get(k.min(order.len().saturating_sub(1))).copied().unwrap_or(0);
        let n = &self.plan.nodes[self.selected];
        // A node selected where every node shows opens the closed nodes above it (`select`).
        match a {
            PlanAction::Down => self.select(to(at + 1)),
            PlanAction::Up => self.select(to(at.saturating_sub(1))),
            PlanAction::Top => self.select(to(0)),
            PlanAction::Bottom => self.select(to(usize::MAX)),
            PlanAction::PageDown => self.select(to(at + page)),
            PlanAction::PageUp => self.select(to(at.saturating_sub(page))),
            PlanAction::Expand if self.view.folds() && self.collapsed[self.selected] => {
                self.collapsed[self.selected] = false
            }
            PlanAction::Expand => {
                if let Some(&c) = n.children.first() {
                    self.select(c);
                }
            }
            PlanAction::Collapse if self.view.folds() && !n.children.is_empty() && !self.collapsed[self.selected] => {
                self.collapsed[self.selected] = true
            }
            PlanAction::Collapse => {
                if let Some(p) = n.parent {
                    self.select(p);
                }
            }
            PlanAction::Detail => self.detail = !self.detail,
            PlanAction::NextView(next) => self.set_view(self.view.step(if next { 1 } else { -1 })),
            PlanAction::View(v) => self.set_view(v),
            PlanAction::PanLeft => self.pan = self.pan.saturating_sub(8),
            PlanAction::PanRight => self.pan += 8,
            PlanAction::CopyText | PlanAction::CopyJson => {}
        }
    }

    fn set_view(&mut self, v: PlanView) {
        if v != self.view {
            self.view = v;
            self.scroll = 0;
            self.pan = 0;
        }
    }
}

/// An `EXPLAIN` waiting for its confirmation to run again as JSON, and the result it
/// was asked from: it runs only while the tab still shows that result, on the same binding.
pub struct AsPlan {
    tab: TabId,
    binding: u64,
    generation: u64,
    query: u64,
    index: Option<usize>,
    /// The statement of the result, and what runs.
    sql: String,
    json: String,
    /// It has `ANALYZE` (what its question says).
    analyze: bool,
    /// The allowlist's question to the server (`DbCommand::CheckRepeat`) this waits for
    /// instead of the user's answer.
    check: Option<u64>,
}

impl AsPlan {
    /// Its tab still shows the result it was asked from, on the same binding and session.
    fn holds(&self, app: &App) -> bool {
        app.tab().id == self.tab
            && app.tabs.get(self.tab).is_some_and(|t| {
                t.binding == self.binding
                    && t.exec.generation == self.generation
                    && t.exec.session.is_some()
                    && t.exec.query_id == self.query
                    && t.exec.shown == self.index
                    && t.exec.view == super::tabs::ResultView::Rows
                    && t.shown_sql() == self.sql
            })
    }
}

/// [`App::text_plan`] of one result: the tab, its run, the result tab, whether its rows are
/// from an earlier run, and the length of its statement name the result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextPlan {
    key: (TabId, u64, Option<usize>, bool, usize),
    explain: bool,
    rewritable: bool,
}

/// What a refusal of `results.view_as_plan` says.
fn not_json(e: plan::explain::NotJson) -> Notice {
    use plan::explain::NotJson;
    match e {
        NotJson::NotExplain => Notice::new(Label::PlanAsPlanNotExplain, Level::Info),
        NotJson::AlreadyJson => Notice::new(Label::PlanAsPlanAlreadyJson, Level::Info),
        NotJson::Several => Notice::new(Label::PlanAsPlanSeveral, Level::Warning),
        NotJson::Unreadable => Notice::new(Label::PlanAsPlanUnreadable, Level::Warning),
    }
}

/// The plan a result is, when it is one: one row of one `json` column the source names as its
/// plan (PostgreSQL: `QUERY PLAN`), complete, whose text reads as a plan. Its JSON comes too.
pub(super) fn plan_of(
    columns: &[datarig_core::driver::ColumnMeta],
    rows: &[Vec<datarig_core::driver::Cell>],
    more: bool,
) -> Option<(Plan, String)> {
    let [c] = columns else { return None };
    let [row] = rows else { return None };
    if more || !plan::pg::is_plan_column(&c.name, &c.type_name) {
        return None;
    }
    let json = row.first()?.as_deref()?;
    plan::pg::parse(json).ok().map(|p| (p, json.to_string()))
}

impl App {
    /// `query.explain` / `query.explain_analyze`: the statement under the cursor, or the one
    /// statement selected, as `EXPLAIN (… FORMAT JSON)`, run like any statement.
    pub(super) fn explain(&mut self, analyze: bool) {
        let (stmts, spans) = self.tab_mut().editor.run_statements();
        let stmt = match &stmts[..] {
            [] => return self.flash(Notice::new(Label::QueryNoStatement, Level::Warning)),
            [one] => one.trim().to_string(),
            more => {
                let count = more.len() as u64;
                return self.flash(Notice::new(Msg::PlanOneStatement { count }, Level::Warning));
            }
        };
        if plan::is_explain(&stmt) {
            let key = self.key_for(Action::RunStatement, Ctx::VimNormal);
            return self.flash(Notice::new(Msg::PlanAlreadyExplain { key }, Level::Warning));
        }
        if self.tab_busy(self.tab().id) {
            return self.flash_busy();
        }
        let sql = plan::explain_sql(&stmt, analyze);
        self.tab_mut().editor.stage_run(std::slice::from_ref(&sql), spans);
        self.run(vec![sql]);
    }

    /// What the active tab's shown rows are for `results.view_as_plan`, worked out once per
    /// result (the hint asks every frame): their statement is an `EXPLAIN` that is not a plan
    /// yet, and the lexer can ask it again as JSON.
    fn text_plan(&self) -> TextPlan {
        let t = self.tab();
        let key = (t.id, t.exec.query_id, t.exec.shown, t.exec.kept_log.is_some(), t.shown_sql().len());
        if let Some(c) = self.text_plan_cache.get().filter(|c| c.key == key) {
            return c;
        }
        let sql = t.shown_sql();
        let explain = t.exec.shown.is_some()
            && t.exec.plan.as_ref().is_none_or(|p| Some(p.index) != t.exec.shown)
            && plan::is_explain(sql);
        // Rows of an earlier run (statements ran since) or of a run of several are not asked
        // again: no offer.
        let alone = t.exec.kept_log.is_none() && !t.rows_log().several();
        let rewritable = explain && alone && plan::explain::json_text(sql).is_ok();
        let c = TextPlan { key, explain, rewritable };
        self.text_plan_cache.set(Some(c));
        c
    }

    /// The active tab shows the rows of an `EXPLAIN` that are not a plan (a text plan):
    /// `results.view_as_plan` acts on them.
    pub fn text_plan_shown(&self) -> bool {
        if self.tabs.is_empty() {
            return false;
        }
        let t = self.tab();
        t.exec.view == super::tabs::ResultView::Rows
            && matches!(t.results, Results::Rows(_))
            && self.text_plan().explain
    }

    /// The key context where the focus is (the results pane's own there).
    fn focus_ctx(&self) -> Ctx {
        match self.focus {
            Focus::Results if self.plan_shown() => Ctx::Plan,
            Focus::Results => Ctx::Grid,
            _ => self.key_context(),
        }
    }

    /// The key bound to `a` where the focus is, if any: never one of another context (in
    /// vim's Insert mode the leader keys would type text).
    fn key_here(&self, a: Action) -> Option<String> {
        self.keymap.hint_keys(a, self.focus_ctx(), self.enhanced_keys).map(|k| crate::keymap::keys::label(&k))
    }

    /// [`App::key_here`], else the key that opens the command line there, where `a` is found
    /// by its name.
    fn key_or_commands(&self, a: Action) -> String {
        self.key_here(a).unwrap_or_else(|| self.key_for(Action::OpenCommands, self.focus_ctx()))
    }

    /// The line under a text plan that offers to view it as one, when it can be asked again,
    /// with the key that does it where the focus is.
    pub fn text_plan_hint(&self) -> Option<String> {
        if !self.text_plan_shown() || !self.text_plan().rewritable {
            return None;
        }
        let msg = match self.key_here(Action::ExplainAsPlan) {
            Some(key) => Msg::PlanAsPlanHint { key },
            None => Msg::PlanAsPlanHintCommands { key: self.key_for(Action::OpenCommands, self.focus_ctx()) },
        };
        Some(self.i18n.msg(&msg).to_string())
    }

    /// `results.view_as_plan` is not available: why, in the words that help (the text plan is
    /// in another result tab or the pane is hidden, the rows are a plan already).
    pub(super) fn explain_as_plan_unavailable(&mut self) {
        use super::tabs::ResultView;
        let t = self.tab();
        let log = t.rows_log();
        let is_plan = |i: usize| t.exec.plan.as_ref().is_some_and(|p| p.index == i);
        let text_plan = t
            .result_tabs()
            .into_iter()
            .any(|i| !is_plan(i) && log.statements.get(i).is_some_and(|s| plan::is_explain(&s.sql)));
        let notice = if text_plan && !self.results_shown() {
            let key = self.key_or_commands(Action::Panel(super::action::PanelAction::Toggle));
            Notice::new(Msg::PlanAsPlanShowFirst { key }, Level::Info)
        } else if text_plan {
            let key = match (self.key_here(Action::ResultTab(false)), self.key_here(Action::ResultTab(true))) {
                (Some(prev), Some(next)) => format!("{prev}/{next}"),
                _ => self.key_or_commands(Action::ResultTab(true)),
            };
            Notice::new(Msg::PlanAsPlanShowFirst { key }, Level::Info)
        } else if t.exec.view == ResultView::Rows && t.exec.shown.is_some_and(is_plan) {
            Notice::new(Label::PlanAsPlanAlreadyJson, Level::Info)
        } else {
            Notice::new(Label::PlanAsPlanNotExplain, Level::Info)
        };
        self.flash(notice);
    }

    /// `results.view_as_plan`: the statement of the shown text plan with `FORMAT JSON`, run like
    /// any statement. What the editor holds now does not matter. It is refused unless the tab
    /// is still where those rows were read, on the same session, with nothing run since or
    /// beside it (a setting, a temporary table, a prepared statement would make it another
    /// plan); what runs more than the planner (`ANALYZE`, an `EXECUTE`'s parameters) asks first.
    pub(super) fn explain_as_plan(&mut self) {
        let t = self.tab();
        let (id, sql) = (t.id, t.shown_sql().to_string());
        if self.tab_busy(id) || self.pending_as_plan.as_ref().is_some_and(|p| p.tab == id && p.check.is_some()) {
            return self.flash_busy();
        }
        let json = match plan::explain::json(&sql) {
            Ok(j) => j,
            Err(e) => return self.flash(not_json(e)),
        };
        let t = self.tab();
        let log = t.rows_log();
        // Read on another connection, database or schema than the tab's now, or on a session
        // that is gone (what it set went with it).
        if log.binding != t.binding || t.exec.session.is_none() || log.generation != t.exec.generation {
            return self.flash(Notice::new(Label::PlanAsPlanMoved, Level::Warning));
        }
        // Statements ran since (the rows are from an earlier run), or beside it.
        if t.exec.kept_log.is_some() {
            return self.flash(Notice::new(Label::PlanAsPlanSince, Level::Warning));
        }
        if log.several() {
            return self.flash(Notice::new(Label::PlanAsPlanBatch, Level::Warning));
        }
        // A read-only profile refuses it now rather than after the question.
        if let Some(pid) = t.profile
            && let Some(refused) = self.read_only_refusal(id, pid, std::slice::from_ref(&json.sql))
        {
            return self.tab_status(id, refused);
        }
        let p = AsPlan {
            tab: id,
            binding: t.binding,
            generation: t.exec.generation,
            query: t.exec.query_id,
            index: t.exec.shown,
            sql,
            json: json.sql,
            analyze: json.analyze,
            check: None,
        };
        if json.evaluates {
            return self.ask_as_plan(p);
        }
        // Its text is on the allowlist: what only the server can tell (a view, a name a user's
        // function or operator shadows) is asked first; the answer runs it or asks.
        self.query_seq += 1;
        let check = self.query_seq;
        self.send_tab(id, DbCommand::CheckRepeat { id: check, sql: json.statement });
        self.pending_as_plan = Some(AsPlan { check: Some(check), ..p });
    }

    /// Ask whether to run `p` again, saying what runs.
    fn ask_as_plan(&mut self, p: AsPlan) {
        let excerpt = super::runlog::excerpt(&p.json, 60);
        let (title, text) = if p.analyze {
            (Label::PlanAsPlanAnalyzeTitle, Msg::PlanAsPlanAnalyzeText { sql: excerpt })
        } else {
            (Label::PlanAsPlanAgainTitle, Msg::PlanAsPlanAgainText { sql: excerpt })
        };
        self.pending_as_plan = Some(AsPlan { check: None, ..p });
        self.confirm(title, title, Label::PlanAsPlanAnalyzeKeys, ConfirmAction::ExplainAgain);
        if let Some(c) = self.overlays.confirm_mut() {
            c.text = text;
        }
    }

    /// The server answered the allowlist's question `check` for a text plan of tab `tab`: it
    /// runs when the server has nothing against it, else it asks; an answer for a result that
    /// changed meanwhile is dropped.
    pub(super) fn as_plan_checked(
        &mut self,
        tab: TabId,
        check: u64,
        result: Result<(), datarig_core::driver::DbError>,
    ) {
        if !self.pending_as_plan.as_ref().is_some_and(|p| p.tab == tab && p.check == Some(check)) {
            return;
        }
        let Some(p) = self.pending_as_plan.take() else { return };
        if !p.holds(self) {
            return self.tab_status(p.tab, Notice::new(Label::PlanAsPlanStale, Level::Warning));
        }
        match result {
            Ok(()) => self.run_in(p.tab, vec![p.json]),
            Err(_) => self.ask_as_plan(p),
        }
    }

    /// Running the `EXPLAIN` again was confirmed: it runs while its tab still shows the result
    /// it was asked from, on the same binding and session.
    pub(super) fn explain_as_plan_confirmed(&mut self) {
        let Some(p) = self.pending_as_plan.take() else { return };
        if !p.holds(self) {
            return self.tab_status(p.tab, Notice::new(Label::PlanAsPlanStale, Level::Warning));
        }
        self.run_in(p.tab, vec![p.json]);
    }

    /// The active tab shows a plan.
    pub fn plan_shown(&self) -> bool {
        !self.tabs.is_empty()
            && self.tab().exec.view == super::tabs::ResultView::Plan
            && self.tab().exec.plan.is_some()
            && self.results_shown()
    }

    pub(super) fn plan_action(&mut self, a: PlanAction) {
        if let PlanAction::CopyText | PlanAction::CopyJson = a {
            return self.copy_plan(a == PlanAction::CopyJson);
        }
        let t = self.tab_mut();
        let Some(p) = t.exec.plan.as_mut() else { return };
        // Choosing a view shows the plan.
        if matches!(a, PlanAction::View(_) | PlanAction::NextView(_)) {
            t.exec.view = super::tabs::ResultView::Plan;
        }
        p.act(a);
    }

    /// The plan as `psql` shows it (or its JSON) to the clipboard.
    fn copy_plan(&mut self, json: bool) {
        let Some(p) = self.tab_mut().exec.plan.as_mut() else { return };
        let text = match json {
            true => p.json.to_string(),
            false => p.raw().lines.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>().join("\n"),
        };
        let count = text.lines().count() as u64;
        let done = match self.deliver(&text) {
            Ok(m) => {
                let method = self.method_text(m);
                let msg = if json { Msg::PlanCopiedJson { count, method } } else { Msg::PlanCopied { count, method } };
                Notice::new(msg, Level::Success)
            }
            Err(msg) => Notice::new(msg, Level::Error),
        };
        self.show_status(done.clone());
        self.flash(done);
    }

    /// A click on the Plan tab at (x, y): a view's name shows it; a node is selected (a second
    /// click soon after on the same node shows or hides its detail).
    pub(super) fn plan_click(&mut self, x: u16, y: u16, double: bool) {
        let at = |r: &Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        let t = self.tab_mut();
        let Some(p) = t.exec.plan.as_mut() else { return };
        if let Some(v) = p.view_hits.iter().find(|(r, _)| at(r)).map(|h| h.1) {
            return p.set_view(v);
        }
        if let Some(i) = p.hits.iter().find(|(r, _)| at(r)).map(|h| h.1) {
            if double && p.selected == i {
                p.detail = !p.detail;
            }
            p.select(i);
        }
    }

    /// The wheel over the Plan tab: the view scrolls (the selection stays).
    pub(super) fn plan_scroll(&mut self, d: isize, sideways: bool) {
        let Some(p) = self.tab_mut().exec.plan.as_mut() else { return };
        if sideways {
            p.pan = (p.pan as isize + d.signum() * 4).max(0) as usize;
            p.detached = true;
        } else {
            p.scroll = (p.scroll as isize + d).max(0) as usize;
            p.detached = true;
        }
    }
}
