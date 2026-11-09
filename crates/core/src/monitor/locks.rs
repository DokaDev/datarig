//! Who waits for whom: the lock waits a tick read ([`LockList`]), the metadata lock
//! compatibility of MySQL ([`mdl_conflicts`]), and the blocking tree the Locks view draws
//! ([`Tree`]), root blockers first.
//!
//! Where the server names a waiter's blockers (PostgreSQL `pg_blocking_pids`, MySQL
//! `data_lock_waits`), the tree shows them as such. Where it does not (MySQL metadata locks), the
//! holders of locks on the same object that the waiter's request cannot be granted beside are
//! shown as "holders of incompatible locks", which is not proof that they block it: a waiter may
//! also wait behind a request of higher priority, which is not modelled.

use super::model::{Blocker, SessionKey, SessionKind, SessionList, SessionRow, TEXT_SAME_START_US, Timestamp, Tx};
use std::collections::{HashMap, HashSet};

/// The most lock waits a read holds (the longest first where the server tells since when).
pub const LOCK_WAITS: usize = 200;

/// The most holders a metadata lock wait keeps (a table many sessions read, which an `ALTER`
/// waits for).
pub const HOLDERS_MAX: usize = 50;

/// The most rows a read of the locks gives: waits and the locks held on what they wait for.
pub const LOCK_ROWS: usize = 2000;

/// The lock waits a tick read ([`crate::monitor::Want::locks`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockList {
    /// The server's clock when they were read.
    pub time: Timestamp,
    pub waits: Vec<LockWait>,
    /// Waits the server has that the read left out (past [`LOCK_WAITS`] or [`LOCK_ROWS`]).
    pub more: u64,
    /// The holders of each wait are what the server names as its blockers, or the holders of
    /// incompatible locks (MySQL). `false`: they are only what the blockers hold on the object
    /// (PostgreSQL, whose session list names the blockers).
    pub blocks: bool,
}

impl LockList {
    /// The waits of session `id`.
    pub fn waits_of(&self, id: i64) -> impl Iterator<Item = &LockWait> {
        self.waits.iter().filter(move |w| w.waiter.id == id)
    }
}

/// A session waiting for a lock, what it asked for, and who holds locks on that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockWait {
    pub waiter: SessionKey,
    pub object: LockObject,
    /// The mode asked for, in the server's words (`ShareLock`, `X,REC_NOT_GAP`, `EXCLUSIVE`).
    pub mode: String,
    /// When the wait started, where the server tells (PostgreSQL 14 on).
    pub since: Option<Timestamp>,
    /// When the waiter's statement started, where the read tells (PostgreSQL; MySQL from the
    /// session list read with it): a later statement of the same session waits for something
    /// else.
    pub query_started: Option<Timestamp>,
    /// `query_started` is worked out from whole seconds (MySQL), so it may move a little from
    /// one read to the next.
    pub whole_seconds: bool,
    pub holders: Vec<LockHolder>,
}

impl LockWait {
    /// Whether this wait (of a lock read, perhaps older than the session list) is still the
    /// wait of session row `r`: the same session, the same statement where the read tells it,
    /// and, where the row names what it waits for (PostgreSQL `wait_event`), the same kind of
    /// lock. The mode and object are empty where the read does not tell them.
    pub fn is_of(&self, r: &SessionRow) -> bool {
        let statement = match (self.query_started, r.query_started) {
            (None, _) => true,
            (Some(a), Some(b)) if self.whole_seconds => (a.0 - b.0).abs() < TEXT_SAME_START_US,
            (a, b) => a == b,
        };
        // MySQL: a row lock wait (`RECORD`) is the session's `row lock`, a metadata lock wait
        // anything else; PostgreSQL: `wait_event` names the lock type.
        let kind = match &r.wait {
            Some(w) if self.whole_seconds => (self.object.kind == "RECORD") == (w.name == "row lock"),
            Some(w) => self.query_started.is_none() || w.name == self.object.kind,
            // MySQL without `INNODB_TRX`: a row lock wait does not show in the list, which cannot
            // tell it is over.
            None => self.whole_seconds && r.tx == Tx::Unknown,
        };
        self.waiter == r.key && statement && kind
    }
}

/// What a lock is on, in the server's words.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct LockObject {
    /// The kind of lock or object (PostgreSQL `locktype`, MySQL `LOCK_TYPE` of a row lock or
    /// `OBJECT_TYPE` of a metadata lock).
    pub kind: String,
    /// The table (or other object) by name, where the server tells it.
    pub name: Option<String>,
    /// Which part of it, in the server's notation (a transaction id, a tuple's `(page,tuple)`,
    /// an index and its record).
    pub detail: Option<String>,
    /// The database the object is in, when another than the monitor's (PostgreSQL names
    /// relations of its own database only).
    pub database: Option<String>,
}

/// Who holds a lock (or asks for one ahead of a waiter).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Holder {
    Session(i64),
    /// A prepared transaction (PostgreSQL), which belongs to no session; `None` when the read
    /// could not tell which.
    Prepared(Option<Prepared>),
    /// A transaction by the engine's id (MySQL), with no session known to have it:
    /// `sessionless` when the server says none has it (an XA transaction prepared and detached
    /// from its session), else its session is not known (not readable, or not found).
    Transaction {
        id: String,
        sessionless: bool,
    },
}

/// A prepared transaction: it holds its locks until someone runs `COMMIT PREPARED` or
/// `ROLLBACK PREPARED` with its id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Prepared {
    pub gid: String,
    pub owner: Option<String>,
    pub database: Option<String>,
    pub since: Option<Timestamp>,
}

/// A lock on what a waiter waits for, of another session or transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockHolder {
    pub holder: Holder,
    /// The modes it holds (or asks for, when [`LockHolder::waiting`]), in the server's words.
    pub modes: Vec<String>,
    /// It asks for the lock too, ahead of the waiter, and does not hold it yet.
    pub waiting: bool,
    pub blame: Blame,
    /// It holds the lock past its transactions (MySQL `LOCK_DURATION = EXPLICIT`: `GET_LOCK`,
    /// `LOCK TABLES`): the session's own, whatever transaction it has open.
    pub explicit: bool,
}

/// How a holder stands to a waiter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Blame {
    /// The server names it as blocking the waiter.
    Proven,
    /// It holds a lock the waiter's request cannot be granted beside: it may block it (MySQL
    /// metadata locks).
    Incompatible,
    /// It holds a lock on the same object whose compatibility with the request is not known.
    CannotTell,
    /// It holds a lock on the object; whether it blocks is the session list's to say
    /// (PostgreSQL).
    Holds,
}

/// Whether a PostgreSQL lock request of mode `requested` waits for a lock of mode `held` on the
/// same object (the table of conflicting lock modes; every kind of lock uses these modes);
/// `None` for a mode the table does not know.
pub fn pg_conflicts(requested: &str, held: &str) -> Option<bool> {
    const MODES: [&str; 8] = [
        "AccessShareLock",
        "RowShareLock",
        "RowExclusiveLock",
        "ShareUpdateExclusiveLock",
        "ShareLock",
        "ShareRowExclusiveLock",
        "ExclusiveLock",
        "AccessExclusiveLock",
    ];
    // For each mode, the modes it conflicts with, as bits in the order above.
    const CONFLICTS: [u8; 8] = [0x80, 0xC0, 0xF0, 0xF8, 0xEC, 0xFC, 0xFE, 0xFF];
    let r = MODES.iter().position(|m| *m == requested)?;
    let h = MODES.iter().position(|m| *m == held)?;
    Some(CONFLICTS[r] & (1 << h) != 0)
}

/// Whether a MySQL metadata lock request of type `requested` waits for a granted lock of type
/// `granted` on the same object of kind `object` (`performance_schema.metadata_locks`
/// `OBJECT_TYPE` and `LOCK_TYPE`), as the server's lock manager decides it (`sql/mdl.cc`); `None`
/// when a type or kind is not one the table knows. Two locks of one owner never conflict.
pub fn mdl_conflicts(object: &str, requested: &str, granted: &str) -> Option<bool> {
    MdlRequest::new(object, requested).waits_for(granted)
}

/// A metadata lock request with its object's kind and its type read once, to check against
/// each lock granted on the object ([`MdlRequest::waits_for`]), as [`mdl_conflicts`] does.
#[derive(Clone, Copy, Debug)]
pub struct MdlRequest(Option<(MdlTable, usize)>);

/// The compatibility table a metadata lock kind is checked with, as the server's lock manager.
#[derive(Clone, Copy, Debug)]
enum MdlTable {
    Scoped,
    Object,
}

#[cfg(test)]
thread_local! {
    /// How many metadata lock kinds this thread read ([`MdlRequest::new`]).
    static MDL_KINDS_READ: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many metadata lock kinds this thread has read so far.
#[cfg(test)]
pub(crate) fn mdl_kinds_read() -> usize {
    MDL_KINDS_READ.with(std::cell::Cell::get)
}

impl MdlRequest {
    /// A request of type `requested` on an object of kind `object`.
    pub fn new(object: &str, requested: &str) -> Self {
        #[cfg(test)]
        MDL_KINDS_READ.with(|n| n.set(n.get() + 1));
        // `BACKUP LOCK` and `BACKUP_LOCK` alike.
        let kind = object.trim().replace('_', " ").to_ascii_uppercase();
        let read = || {
            let r = MdlType::of(requested)?;
            if SCOPED.contains(&kind.as_str()) {
                Some((MdlTable::Scoped, r.scoped()?))
            } else if OBJECT.contains(&kind.as_str()) {
                Some((MdlTable::Object, r.object()?))
            } else {
                None
            }
        };
        Self(read())
    }

    /// Whether it waits for a granted lock of type `granted` on the same object; `None` when a
    /// type or the kind is not one the table knows.
    pub fn waits_for(self, granted: &str) -> Option<bool> {
        let (table, r) = self.0?;
        let g = MdlType::of(granted)?;
        Some(match table {
            MdlTable::Scoped => SCOPED_INCOMPATIBLE[r] & (1 << g.scoped()?) != 0,
            MdlTable::Object => OBJECT_INCOMPATIBLE[r] & (1 << g.object()?) != 0,
        })
    }
}

/// The metadata lock kinds (`OBJECT_TYPE`) the server's lock manager takes as scoped locks.
const SCOPED: &[&str] =
    &["GLOBAL", "TABLESPACE", "SCHEMA", "COMMIT", "BACKUP LOCK", "RESOURCE GROUPS", "FOREIGN KEY", "CHECK CONSTRAINT"];

/// The metadata lock kinds it takes as object locks.
const OBJECT: &[&str] = &[
    "TABLE",
    "FUNCTION",
    "PROCEDURE",
    "TRIGGER",
    "EVENT",
    "USER LEVEL LOCK",
    "LOCKING SERVICE",
    "SRID",
    "ACL CACHE",
    "COLUMN STATISTICS",
];

/// A metadata lock type (`LOCK_TYPE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MdlType {
    IntentionExclusive,
    Shared,
    SharedHighPrio,
    SharedRead,
    SharedWrite,
    SharedWriteLowPrio,
    SharedUpgradable,
    SharedReadOnly,
    SharedNoWrite,
    SharedNoReadWrite,
    Exclusive,
}

impl MdlType {
    fn of(name: &str) -> Option<Self> {
        Some(match name.trim() {
            "INTENTION_EXCLUSIVE" => MdlType::IntentionExclusive,
            "SHARED" => MdlType::Shared,
            "SHARED_HIGH_PRIO" => MdlType::SharedHighPrio,
            "SHARED_READ" => MdlType::SharedRead,
            "SHARED_WRITE" => MdlType::SharedWrite,
            "SHARED_WRITE_LOW_PRIO" => MdlType::SharedWriteLowPrio,
            "SHARED_UPGRADABLE" => MdlType::SharedUpgradable,
            "SHARED_READ_ONLY" => MdlType::SharedReadOnly,
            "SHARED_NO_WRITE" => MdlType::SharedNoWrite,
            "SHARED_NO_READ_WRITE" => MdlType::SharedNoReadWrite,
            "EXCLUSIVE" => MdlType::Exclusive,
            _ => return None,
        })
    }

    /// Its place in [`OBJECT_INCOMPATIBLE`] (S, SH, SR, SW, SWLP, SU, SRO, SNW, SNRW, X); an
    /// object lock is never intention exclusive.
    fn object(self) -> Option<usize> {
        Some(match self {
            MdlType::IntentionExclusive => return None,
            MdlType::Shared => 0,
            MdlType::SharedHighPrio => 1,
            MdlType::SharedRead => 2,
            MdlType::SharedWrite => 3,
            MdlType::SharedWriteLowPrio => 4,
            MdlType::SharedUpgradable => 5,
            MdlType::SharedReadOnly => 6,
            MdlType::SharedNoWrite => 7,
            MdlType::SharedNoReadWrite => 8,
            MdlType::Exclusive => 9,
        })
    }

    /// Its place in [`SCOPED_INCOMPATIBLE`] (IX, S, X); a scoped lock is of these only.
    fn scoped(self) -> Option<usize> {
        match self {
            MdlType::IntentionExclusive => Some(0),
            MdlType::Shared => Some(1),
            MdlType::Exclusive => Some(2),
            _ => None,
        }
    }
}

/// For each requested object lock type (S, SH, SR, SW, SWLP, SU, SRO, SNW, SNRW, X), the granted
/// types it waits for, as bits in the same order: the server's matrix of granted requests for
/// object locks.
const OBJECT_INCOMPATIBLE: [u16; 10] = {
    const fn bits(types: &[usize]) -> u16 {
        let mut b = 0;
        let mut i = 0;
        while i < types.len() {
            b |= 1 << types[i];
            i += 1;
        }
        b
    }
    const S: usize = 0;
    const SH: usize = 1;
    const SR: usize = 2;
    const SW: usize = 3;
    const SWLP: usize = 4;
    const SU: usize = 5;
    const SRO: usize = 6;
    const SNW: usize = 7;
    const SNRW: usize = 8;
    const X: usize = 9;
    [
        bits(&[X]),
        bits(&[X]),
        bits(&[SNRW, X]),
        bits(&[SRO, SNW, SNRW, X]),
        bits(&[SRO, SNW, SNRW, X]),
        bits(&[SU, SNW, SNRW, X]),
        bits(&[SW, SWLP, SNRW, X]),
        bits(&[SW, SWLP, SU, SNW, SNRW, X]),
        bits(&[SR, SW, SWLP, SU, SRO, SNW, SNRW, X]),
        bits(&[S, SH, SR, SW, SWLP, SU, SRO, SNW, SNRW, X]),
    ]
};

/// The same for scoped locks (IX, S, X): intention exclusive waits for shared and exclusive,
/// shared for intention exclusive and exclusive, exclusive for all three.
const SCOPED_INCOMPATIBLE: [u16; 3] = [0b110, 0b101, 0b111];

/// One edge of the tree: `waiter` waits for `holder`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edge {
    pub waiter: i64,
    pub holder: Holder,
    pub blame: Blame,
}

/// Who waits for whom, as a tick's reads tell it ([`edges`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Waits {
    pub edges: Vec<Edge>,
    /// Sessions waiting for a lock whose holder the reads did not find (the lock was granted
    /// just after, or a MySQL metadata lock request waits behind another request rather than a
    /// granted lock).
    pub alone: Vec<i64>,
    /// The reads may have left some waits or holders out (a lock read cut short, edges past
    /// [`EDGES_MAX`], parallel workers whose leader is not told, metadata locks the server did
    /// not track): no root is sure.
    pub partial: bool,
}

/// The edges a tick's reads tell: the blockers the session list names (PostgreSQL), and the
/// lock read's holders where it names them ([`LockList::blocks`]). A prepared transaction the
/// session list names is told apart by the lock read when it has the waiter's wait.
///
/// The lock read may be older than the session list (it is read less often): a wait of a
/// session the list shows not waiting for a lock is over and left out, and so is one of a
/// session it does not list when it lists every session (`complete`).
pub fn edges(sessions: Option<&SessionList>, locks: Option<&LockList>, complete: bool) -> Waits {
    let mut out = Out::default();
    let mut alone = Vec::new();
    let mut alone_set: HashSet<i64> = HashSet::new();
    let by_id: HashMap<i64, &SessionRow> =
        sessions.into_iter().flat_map(|l| l.rows.iter()).map(|r| (r.key.id, r)).collect();
    let mut named: HashSet<i64> = HashSet::new();
    // A parallel worker's wait is its leader's (blockers are named by their leader): the group is
    // one node. A worker whose leader is not told (before PostgreSQL 13) leaves the tree unsure.
    let mut unsure_group = false;
    if let Some(list) = sessions {
        for r in &list.rows {
            let Some(blockers) = &r.blocked_by else { continue };
            let waiter = match r.kind {
                SessionKind::ParallelWorker { leader: Some(l) } => l,
                SessionKind::ParallelWorker { leader: None } => {
                    unsure_group = true;
                    r.key.id
                }
                _ => r.key.id,
            };
            named.insert(waiter);
            if blockers.is_empty() && alone_set.insert(waiter) {
                alone.push(waiter);
            }
            for b in blockers {
                match b {
                    Blocker::Session(id) if *id == waiter => {}
                    Blocker::Session(id) => out.push(waiter, Holder::Session(*id), Blame::Proven),
                    Blocker::PreparedTransaction => {
                        // Told apart by the lock read when it has this very wait: of the
                        // prepared transactions holding a lock on its object, those whose lock
                        // conflicts with the one asked for (a granted conflicting lock blocks).
                        let mut found = false;
                        let waits = locks.into_iter().flat_map(|l| l.waits.iter()).filter(|w| w.is_of(r));
                        for w in waits {
                            for h in &w.holders {
                                // A mode the table does not know (`SIReadLock`) blocks nothing it can tell.
                                let conflicts = h.modes.iter().any(|m| pg_conflicts(&w.mode, m) == Some(true));
                                if matches!(h.holder, Holder::Prepared(Some(_))) && conflicts {
                                    found = true;
                                    out.push(waiter, h.holder.clone(), Blame::Proven);
                                }
                            }
                        }
                        if !found {
                            out.push(waiter, Holder::Prepared(None), Blame::Proven);
                        }
                    }
                }
            }
        }
    }
    let Some(locks) = locks.filter(|l| l.blocks) else {
        // A blocker that waits itself but was past the waiters asked about (PostgreSQL asks
        // the longest ones only): waiting all the same, for a holder not asked.
        for r in sessions.into_iter().flat_map(|l| l.rows.iter()) {
            let id = r.key.id;
            let blocks = out.holders.contains(&Holder::Session(id));
            if blocks && r.waits_for_lock() && !named.contains(&id) && alone_set.insert(id) {
                alone.push(id);
            }
        }
        return Waits { partial: out.cut || unsure_group, edges: out.edges, alone };
    };
    for w in &locks.waits {
        if named.contains(&w.waiter.id) {
            continue;
        }
        if sessions.is_some() {
            match by_id.get(&w.waiter.id) {
                Some(r) if !w.is_of(r) => continue,
                None if complete => continue,
                _ => {}
            }
        }
        for h in &w.holders {
            if h.holder == Holder::Session(w.waiter.id) {
                continue;
            }
            // A metadata lock lasts until its transaction ends: a holder known to have none open
            // holds it for another (a prepared XA transaction keeps the thread that took it), so
            // whether that session blocks is not known.
            // (A lock held past transactions, `GET_LOCK` or `LOCK TABLES`, is the session's own.)
            let none_open = match &h.holder {
                Holder::Session(id) => !h.explicit && by_id.get(id).is_some_and(|r| r.tx == Tx::None),
                _ => false,
            };
            let blame = match h.blame {
                Blame::Incompatible if none_open => Blame::CannotTell,
                b => b,
            };
            out.push(w.waiter.id, h.holder.clone(), blame);
        }
    }
    for w in &locks.waits {
        let id = w.waiter.id;
        let shown = sessions.is_none() || by_id.get(&id).map_or(!complete, |r| w.is_of(r));
        if shown && !named.contains(&id) && !out.waiters.contains(&id) && alone_set.insert(id) {
            alone.push(id);
        }
    }
    // A session the list shows waiting whose blocker no read named (the lock tables refused,
    // or the wait began after they were read): waiting all the same.
    for r in sessions.into_iter().flat_map(|l| l.rows.iter()) {
        let id = r.key.id;
        if r.waits_for_lock() && !named.contains(&id) && !out.waiters.contains(&id) && alone_set.insert(id) {
            alone.push(id);
        }
    }
    Waits { partial: locks.more > 0 || out.cut || unsure_group, edges: out.edges, alone }
}

/// The most edges a tree is built from: many metadata lock requests on one object, each paired
/// with every holder, would otherwise make millions (the rest are left out, and said).
pub const EDGES_MAX: usize = 10_000;

/// Edges gathered once each, looked up by (waiter, holder).
#[derive(Default)]
struct Out {
    edges: Vec<Edge>,
    at: HashMap<(i64, Holder), usize>,
    waiters: HashSet<i64>,
    holders: HashSet<Holder>,
    /// Edges left out past [`EDGES_MAX`].
    cut: bool,
}

impl Out {
    fn push(&mut self, waiter: i64, holder: Holder, blame: Blame) {
        match self.at.get(&(waiter, holder.clone())) {
            // The surer of two reasons a holder blocks.
            Some(&i) if rank(blame) < rank(self.edges[i].blame) => self.edges[i].blame = blame,
            Some(_) => {}
            None if self.edges.len() >= EDGES_MAX => self.cut = true,
            None => {
                self.at.insert((waiter, holder.clone()), self.edges.len());
                self.waiters.insert(waiter);
                self.holders.insert(holder.clone());
                self.edges.push(Edge { waiter, holder, blame });
            }
        }
    }
}

fn rank(b: Blame) -> u8 {
    match b {
        Blame::Proven => 0,
        Blame::Incompatible => 1,
        Blame::CannotTell => 2,
        Blame::Holds => 3,
    }
}

/// The blocking tree: each session or transaction on one line, under the one it waits for,
/// root blockers first (those with the most sessions waiting behind them).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tree {
    pub lines: Vec<Line>,
    /// Sessions waiting for a lock (the tree's lines below a root).
    pub waiting: usize,
    /// From a lock read that left some out ([`Waits::partial`]).
    pub partial: bool,
}

/// A line of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub node: Holder,
    /// 0 for a root.
    pub depth: usize,
    /// The line it waits for (`None` for a root), and how.
    pub parent: Option<(usize, Blame)>,
    /// What else it waits for, besides the line it is under.
    pub also: Vec<(Holder, Blame)>,
    /// A root that waits itself, in a cycle of waits (a deadlock the server has not broken
    /// yet): it is put first of its cycle.
    pub cycle: bool,
    /// Sessions waiting behind it, at any depth.
    pub behind: usize,
    /// A session waiting for a lock whose holder was not found ([`Waits::alone`]).
    pub alone: bool,
}

impl Tree {
    /// The tree of `waits`: its edges, then the sessions waiting alone, after the roots.
    pub fn build(waits: &Waits) -> Tree {
        let mut tree = Tree::build_edges(&waits.edges);
        tree.partial = waits.partial;
        let mut alone = waits.alone.clone();
        alone.sort_unstable();
        for id in alone {
            let node = Holder::Session(id);
            // Already a line (a root others wait for): it waits too, for a holder not found.
            if let Some(l) = tree.lines.iter_mut().find(|l| l.node == node) {
                if !l.alone && l.parent.is_none() && !l.cycle {
                    tree.waiting += 1;
                }
                l.alone = true;
                continue;
            }
            tree.waiting += 1;
            tree.lines.push(Line {
                node,
                depth: 0,
                parent: None,
                also: Vec::new(),
                cycle: false,
                behind: 0,
                alone: true,
            });
        }
        tree
    }

    fn build_edges(edges: &[Edge]) -> Tree {
        if edges.is_empty() {
            return Tree::default();
        }
        // What each waiter waits for, in the order told.
        let mut waits: HashMap<i64, Vec<(Holder, Blame)>> = HashMap::new();
        // Who waits for each holder.
        let mut behind: HashMap<Holder, Vec<i64>> = HashMap::new();
        let mut nodes: Vec<Holder> = Vec::new();
        let mut seen: HashSet<Holder> = HashSet::new();
        for e in edges {
            waits.entry(e.waiter).or_default().push((e.holder.clone(), e.blame));
            behind.entry(e.holder.clone()).or_default().push(e.waiter);
            for n in [Holder::Session(e.waiter), e.holder.clone()] {
                if seen.insert(n.clone()) {
                    nodes.push(n);
                }
            }
        }
        let waits_for = |n: &Holder| match n {
            Holder::Session(id) => waits.get(id),
            _ => None,
        };
        // Roots: those that wait for nobody.
        let mut roots: Vec<Holder> = nodes.iter().filter(|n| waits_for(n).is_none()).cloned().collect();
        let mut placed: HashMap<Holder, (Option<Holder>, usize)> = HashMap::new();
        let mut cycles: HashSet<Holder> = HashSet::new();
        // Place everything reachable from the roots, nearest first; then what is left waits in
        // or behind a cycle: its smallest session on the cycle is taken as a root, and again.
        let mut order: Vec<Holder> = Vec::new();
        loop {
            let mut queue: std::collections::VecDeque<Holder> = std::collections::VecDeque::new();
            for r in &roots {
                if !placed.contains_key(r) {
                    placed.insert(r.clone(), (None, 0));
                    order.push(r.clone());
                    queue.push_back(r.clone());
                }
            }
            while let Some(h) = queue.pop_front() {
                let depth = placed[&h].1;
                let mut next: Vec<i64> = behind.get(&h).cloned().unwrap_or_default();
                next.sort_unstable();
                next.dedup();
                for w in next {
                    let n = Holder::Session(w);
                    if !placed.contains_key(&n) {
                        placed.insert(n.clone(), (Some(h.clone()), depth + 1));
                        queue.push_back(n);
                    }
                }
            }
            let left: Vec<&Holder> = nodes.iter().filter(|n| !placed.contains_key(*n)).collect();
            let Some(start) = left.first() else { break };
            // Follow the first wait from a node left until a node repeats: that one is on a
            // cycle; the cycle's smallest session becomes the root.
            let mut path: Vec<Holder> = Vec::new();
            let mut at = (*start).clone();
            while !path.contains(&at) {
                path.push(at.clone());
                match waits_for(&at).and_then(|w| w.iter().find(|(h, _)| !placed.contains_key(h))) {
                    Some((h, _)) => at = h.clone(),
                    None => break,
                }
            }
            let from = path.iter().position(|n| *n == at).unwrap_or(0);
            let root = path[from..]
                .iter()
                .min_by_key(|n| match n {
                    Holder::Session(id) => *id,
                    _ => i64::MAX,
                })
                .cloned()
                .unwrap_or(at);
            cycles.insert(root.clone());
            roots = vec![root];
        }
        // Children of each placed node, and the count behind each.
        let mut children: HashMap<Holder, Vec<Holder>> = HashMap::new();
        for n in &nodes {
            if let Some((Some(p), _)) = placed.get(n) {
                children.entry(p.clone()).or_default().push(n.clone());
            }
        }
        fn count(n: &Holder, children: &HashMap<Holder, Vec<Holder>>, memo: &mut HashMap<Holder, usize>) -> usize {
            if let Some(c) = memo.get(n) {
                return *c;
            }
            let c = children.get(n).map_or(0, |cs| cs.iter().map(|c| 1 + count(c, children, memo)).sum());
            memo.insert(n.clone(), c);
            c
        }
        let mut memo = HashMap::new();
        for n in &nodes {
            count(n, &children, &mut memo);
        }
        let key = |n: &Holder| match n {
            Holder::Session(id) => (0, *id, String::new()),
            Holder::Prepared(Some(p)) => (1, 0, p.gid.clone()),
            Holder::Prepared(None) => (1, 0, String::new()),
            Holder::Transaction { id, .. } => (2, 0, id.clone()),
        };
        for cs in children.values_mut() {
            cs.sort_by(|a, b| memo[b].cmp(&memo[a]).then_with(|| key(a).cmp(&key(b))));
        }
        let mut tops: Vec<Holder> = order.into_iter().filter(|n| placed[n].0.is_none()).collect();
        tops.sort_by(|a, b| memo[b].cmp(&memo[a]).then_with(|| key(a).cmp(&key(b))));
        let mut tree = Tree::default();
        let mut stack: Vec<(Holder, Option<usize>)> = tops.into_iter().rev().map(|n| (n, None)).collect();
        while let Some((n, parent)) = stack.pop() {
            let at = tree.lines.len();
            let all = waits_for(&n).cloned().unwrap_or_default();
            let up = placed[&n].0.clone();
            let blame = up.as_ref().and_then(|p| all.iter().find(|(h, _)| h == p).map(|(_, b)| *b));
            let also = all.into_iter().filter(|(h, _)| Some(h) != up.as_ref()).collect();
            tree.lines.push(Line {
                depth: placed[&n].1,
                parent: parent.zip(blame),
                also,
                cycle: cycles.contains(&n),
                behind: memo[&n],
                alone: false,
                node: n.clone(),
            });
            if parent.is_some() || cycles.contains(&n) {
                tree.waiting += 1;
            }
            for c in children.get(&n).into_iter().flatten().rev() {
                stack.push((c.clone(), Some(at)));
            }
        }
        tree
    }

    /// The root of line `at`'s branch, and whether the way up is sure: every wait on it is one
    /// the server names, or a holder of incompatible locks that is the waiter's only one.
    pub fn root_of(&self, at: usize) -> Option<(usize, bool)> {
        let mut i = at;
        let mut sure = true;
        let mut steps = 0;
        while let Some((p, blame)) = self.lines.get(i)?.parent {
            let only = self.lines[i].also.is_empty();
            sure &= !self.partial && (blame == Blame::Proven || (blame == Blame::Incompatible && only));
            i = p;
            steps += 1;
            if steps > self.lines.len() {
                return None;
            }
        }
        // A root that waits itself, for a holder not found, is not the root of the waits.
        Some((i, sure && !self.lines[i].alone))
    }
}
