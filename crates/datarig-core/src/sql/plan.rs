//! Query plans: what a database did (or would do) for one statement, as a tree of nodes, in a
//! model that does not depend on the database ([`Plan`], [`PlanNode`]). PostgreSQL's
//! `EXPLAIN (FORMAT JSON)` is one source ([`pg`]); another database's plan output can be read
//! into the same model. [`text`] writes a plan the way `psql` shows a text `EXPLAIN`, from the
//! model alone: nothing is asked of the server again.
//!
//! **Time and cost.** A plan read with `ANALYZE` (and timing) has measured times: each node's
//! total time ([`PlanNode::total_ms`], its subtree's) and self time ([`PlanNode::self_ms`],
//! the total less its children's), worked out by the source (a source knows how its loops and
//! parallel workers count). Without them only the planner's estimated costs are there, and
//! the views weigh nodes by cost ([`Measure::Cost`]): never presented as time.
//!
//! **Hot nodes and misestimates.** A node is hot when its self time is at least [`HOT`] of the
//! statement's execution time (by cost: of the plan's total cost). Its row estimate is off when
//! the estimated and actual rows per loop differ by a factor of [`MISESTIMATE`] or more.

pub mod explain;
pub mod json;
pub mod pg;
pub mod text;

/// A node whose self time (or cost) is at least this share of the whole is hot.
pub const HOT: f64 = 0.2;

/// Estimated and actual rows this many times apart (either way) are a misestimate.
pub const MISESTIMATE: f64 = 10.0;

/// At most this many nodes are read from one plan.
pub const MAX_NODES: usize = 100_000;

/// A plan: its nodes in depth-first order (a parent before its children, children in the order
/// the source lists them) and what the statement as a whole took.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub nodes: Vec<PlanNode>,
    /// The plan was measured (`ANALYZE`): nodes have actual rows and loops.
    pub analyzed: bool,
    /// The measurement has times (not `ANALYZE` with timing off).
    pub timed: bool,
    pub planning_ms: Option<f64>,
    pub execution_ms: Option<f64>,
    /// Buffers the planning used, when the source reports them.
    pub planning_buffers: Option<Buffers>,
    pub triggers: Vec<TriggerRun>,
    /// What the statement as a whole reported after the tree (settings, planning, triggers,
    /// JIT, times), as lines of the source's text form: a line and the lines indented under it.
    pub footer: Vec<(String, Vec<String>)>,
    /// Plans of the same statement after the first (a rule adds statements): only the first
    /// is read.
    pub more_plans: usize,
    /// What [`Plan::measure`] and [`Plan::total`] say, worked out once by [`Plan::finish`]
    /// (the views ask for them for every node they draw).
    measured: Option<(Measure, f64)>,
}

/// One node of a plan.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlanNode {
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// The root is at depth 0.
    pub depth: usize,
    /// The operation as the source's text names it: `Hash Left Join`, `Parallel Seq Scan`,
    /// `Finalize HashAggregate`.
    pub op: String,
    /// What it works on, as its text says it: `on orders o`, `using orders_pkey on orders o`.
    pub target: Option<String>,
    /// How it hangs under its parent, as the source says (`Outer`, `Inner`, `InitPlan`,
    /// `SubPlan`, `Member`, `Subquery`).
    pub relationship: Option<String>,
    /// The name of a subplan (`InitPlan 1`, `SubPlan 2`, `CTE totals`).
    pub subplan: Option<String>,
    /// Estimated startup and total cost.
    pub cost: Option<(f64, f64)>,
    /// Estimated rows per loop and their width in bytes.
    pub plan_rows: Option<f64>,
    pub plan_width: Option<f64>,
    /// Measured (`ANALYZE`).
    pub actual: Option<Actual>,
    /// Buffers, the node's subtree included (as the source counts them).
    pub buffers: Option<Buffers>,
    /// Rows a filter, a join filter or an index recheck removed, every loop together.
    pub removed: Option<f64>,
    /// Parallel workers planned and launched (a node that gathers them).
    pub workers: Option<(u64, Option<u64>)>,
    /// Everything else the node says, as its text form says it, in that order
    /// (`Filter`, `(amount > 10)`).
    pub properties: Vec<(String, String)>,
    /// Measured time of the node's subtree, every loop together, in milliseconds.
    pub total_ms: Option<f64>,
    /// [`PlanNode::total_ms`] less its children's: the node's own share.
    pub self_ms: Option<f64>,
    /// Estimated total cost less its children's.
    pub self_cost: Option<f64>,
    /// Its parent may stop reading it before its end (a limit, the inner side of a semi join):
    /// fewer rows than estimated is then no misestimate.
    pub early_stop: bool,
    /// Its time is counted inside other nodes that read it (a CTE inside its scans), not in its
    /// parent's.
    pub elsewhere: bool,
}

/// What `ANALYZE` measured on a node: averages per loop, as the server reports them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Actual {
    /// Time to the first row and to the last, per loop (`None`: timing was off).
    pub startup_ms: Option<f64>,
    pub total_ms: Option<f64>,
    /// Rows per loop (fractional from PostgreSQL 18 on).
    pub rows: f64,
    /// The source wrote the rows with decimals (PostgreSQL 18).
    pub rows_decimals: bool,
    pub loops: f64,
}

/// Blocks a node read and wrote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Buffers {
    pub shared_hit: u64,
    pub shared_read: u64,
    pub shared_dirtied: u64,
    pub shared_written: u64,
    pub local_hit: u64,
    pub local_read: u64,
    pub local_dirtied: u64,
    pub local_written: u64,
    pub temp_read: u64,
    pub temp_written: u64,
}

impl Buffers {
    /// Blocks found in the cache and blocks read from outside it (shared and local).
    pub fn hits(&self) -> u64 {
        self.shared_hit + self.local_hit
    }

    pub fn reads(&self) -> u64 {
        self.shared_read + self.local_read
    }

    /// The share of blocks found in the cache, when any block was asked for.
    pub fn hit_ratio(&self) -> Option<f64> {
        let all = self.hits() + self.reads();
        (all > 0).then(|| self.hits() as f64 / all as f64)
    }

    pub fn is_empty(&self) -> bool {
        *self == Buffers::default()
    }
}

/// A trigger that fired while the statement ran.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TriggerRun {
    pub name: String,
    pub relation: Option<String>,
    pub ms: f64,
    pub calls: f64,
}

/// What the plan's nodes are weighed by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Measure {
    /// Measured time (`ANALYZE` with timing).
    Time,
    /// The planner's estimated cost: not time.
    Cost,
}

/// How far a node's row estimate is off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowsOff {
    /// `max(estimated / actual, actual / estimated)`, both taken as at least one row.
    pub ratio: f64,
    /// More rows came than the planner thought.
    pub under: bool,
}

/// Why a text is not a plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanError {
    /// Not JSON.
    Json,
    /// JSON, but not a plan of the source.
    NotAPlan,
    /// More than [`MAX_NODES`] nodes.
    TooBig,
}

impl Plan {
    /// The roots: one, unless the source listed several trees as one plan.
    pub fn roots(&self) -> impl Iterator<Item = usize> + '_ {
        self.nodes.iter().enumerate().filter(|(_, n)| n.parent.is_none()).map(|(i, _)| i)
    }

    /// Work out what the whole is (call once the nodes are complete; a source does).
    pub fn finish(&mut self) {
        self.measured = None;
        let measure =
            if self.timed && self.nodes.iter().all(|n| n.self_ms.is_some()) { Measure::Time } else { Measure::Cost };
        self.measured = Some((measure, 0.0));
        let roots: f64 = self.roots().map(|r| self.inclusive(r)).sum();
        let total = match (measure, self.execution_ms) {
            (Measure::Time, Some(ms)) => ms.max(roots),
            _ => roots,
        };
        self.measured = Some((measure, total));
    }

    /// What the nodes are weighed by: time when every node that ran has its self time.
    pub fn measure(&self) -> Measure {
        match self.measured {
            Some((m, _)) => m,
            None if self.timed && self.nodes.iter().all(|n| n.self_ms.is_some()) => Measure::Time,
            None => Measure::Cost,
        }
    }

    /// Node `i`'s own weight: its self time, or by cost its self cost.
    pub fn weight(&self, i: usize) -> f64 {
        let n = &self.nodes[i];
        match self.measure() {
            Measure::Time => n.self_ms.unwrap_or(0.0),
            Measure::Cost => n.self_cost.unwrap_or(0.0),
        }
    }

    /// Node `i`'s subtree weight: its total time, or by cost its total cost.
    pub fn inclusive(&self, i: usize) -> f64 {
        let n = &self.nodes[i];
        match self.measure() {
            Measure::Time => n.total_ms.unwrap_or(0.0),
            Measure::Cost => n.cost.map_or(0.0, |c| c.1),
        }
    }

    /// What a node's share is a share of: the execution time (by cost, the roots' total cost).
    /// Never less than the roots' own total (a source's execution time may miss a part).
    pub fn total(&self) -> f64 {
        if let Some((_, total)) = self.measured.filter(|(_, t)| *t > 0.0) {
            return total;
        }
        let roots: f64 = self.roots().map(|r| self.inclusive(r)).sum();
        match (self.measure(), self.execution_ms) {
            (Measure::Time, Some(ms)) => ms.max(roots),
            _ => roots,
        }
    }

    /// Node `i`'s own weight as a share of [`Plan::total`] (0 to 1).
    pub fn share(&self, i: usize) -> f64 {
        let total = self.total();
        if total > 0.0 { (self.weight(i) / total).clamp(0.0, 1.0) } else { 0.0 }
    }

    /// Node `i` takes at least [`HOT`] of the whole.
    pub fn is_hot(&self, i: usize) -> bool {
        self.total() > 0.0 && self.share(i) >= HOT
    }

    /// How far node `i`'s row estimate was from what came (per loop), once it ran.
    pub fn rows_off(&self, i: usize) -> Option<RowsOff> {
        let n = &self.nodes[i];
        let a = n.actual.filter(|a| a.loops > 0.0)?;
        let est = n.plan_rows?.max(1.0);
        let act = a.rows.max(1.0);
        let off = RowsOff { ratio: (est / act).max(act / est), under: act > est };
        // Stopped early on purpose: fewer rows tell nothing about the estimate.
        (off.under || !n.early_stop).then_some(off)
    }

    /// [`Plan::rows_off`] when it is a misestimate (at least [`MISESTIMATE`] apart).
    pub fn misestimate(&self, i: usize) -> Option<RowsOff> {
        self.rows_off(i).filter(|o| o.ratio >= MISESTIMATE)
    }

    /// Rows node `i` passed up, every loop together: measured, else estimated per loop.
    pub fn rows_out(&self, i: usize) -> Option<f64> {
        let n = &self.nodes[i];
        match n.actual {
            Some(a) => Some(a.rows * a.loops),
            None => n.plan_rows,
        }
    }

    /// The node with the largest own weight (the first of equals).
    pub fn slowest(&self) -> Option<usize> {
        (0..self.nodes.len()).fold(None, |best: Option<usize>, i| match best {
            Some(b) if self.weight(b) >= self.weight(i) => Some(b),
            _ => Some(i),
        })
    }

    /// The node whose row estimate is furthest off, if any is a misestimate.
    pub fn worst_misestimate(&self) -> Option<(usize, RowsOff)> {
        (0..self.nodes.len()).filter_map(|i| self.misestimate(i).map(|o| (i, o))).fold(
            None,
            |best, (i, o)| match best {
                Some((_, b)) if b.ratio >= o.ratio => best,
                _ => Some((i, o)),
            },
        )
    }

    /// The buffers of the whole plan: the roots' (each counts its subtree).
    pub fn buffers(&self) -> Option<Buffers> {
        let mut all: Option<Buffers> = None;
        for r in self.roots() {
            if let Some(b) = self.nodes[r].buffers {
                let a = all.get_or_insert_with(Buffers::default);
                a.shared_hit += b.shared_hit;
                a.shared_read += b.shared_read;
                a.shared_dirtied += b.shared_dirtied;
                a.shared_written += b.shared_written;
                a.local_hit += b.local_hit;
                a.local_read += b.local_read;
                a.local_dirtied += b.local_dirtied;
                a.local_written += b.local_written;
                a.temp_read += b.temp_read;
                a.temp_written += b.temp_written;
            }
        }
        all
    }

    /// The node's name as one line: its operation and what it works on.
    pub fn label(&self, i: usize) -> String {
        let n = &self.nodes[i];
        match &n.target {
            Some(t) => format!("{} {t}", n.op),
            None => n.op.clone(),
        }
    }
}

/// The SQL that asks PostgreSQL for `statement`'s plan as JSON (`analyze`: run it and measure,
/// with buffers). PostgreSQL syntax: another database asks in its own words.
pub fn explain_sql(statement: &str, analyze: bool) -> String {
    let options = if analyze { "ANALYZE, BUFFERS, FORMAT JSON" } else { "FORMAT JSON" };
    format!("EXPLAIN ({options}) {statement}")
}

/// `sql` starts with `EXPLAIN` (comments and blanks before it skipped).
pub fn is_explain(sql: &str) -> bool {
    crate::sql::lexer::lex(sql)
        .into_iter()
        .find(|t| !t.is_trivia())
        .is_some_and(|t| t.text(sql).eq_ignore_ascii_case("EXPLAIN"))
}

#[cfg(test)]
mod tests;
