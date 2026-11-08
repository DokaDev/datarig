//! Who waits for whom: the lock waits a tick read ([`LockList`]), the metadata lock
//! compatibility of MySQL ([`mdl_conflicts`]), and the blocking tree the Locks view draws
//! ([`Tree`]), root blockers first.
//!
//! Where the server names a waiter's blockers (PostgreSQL `pg_blocking_pids`, MySQL
//! `data_lock_waits`), the tree shows them as such. Where it does not (MySQL metadata locks), the
//! holders of locks on the same object that the waiter's request cannot be granted beside are
//! shown as "holders of incompatible locks", which is not proof that they block it: a waiter may
//! also wait behind a request of higher priority, which is not modelled.

use super::model::{Blocker, SessionKey, SessionList, Timestamp};
use std::collections::{HashMap, HashSet};

/// The most lock waits a read holds (the longest first where the server tells since when).
pub const LOCK_WAITS: usize = 200;

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
    pub holders: Vec<LockHolder>,
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
    /// A transaction no session has (MySQL: an XA transaction prepared by a session that has
    /// gone), by the engine's transaction id.
    Transaction(String),
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

/// Whether a MySQL metadata lock request of type `requested` waits for a granted lock of type
/// `granted` on the same object of kind `object` (`performance_schema.metadata_locks`
/// `OBJECT_TYPE` and `LOCK_TYPE`), as the server's lock manager decides it (`sql/mdl.cc`); `None`
/// when a type or kind is not one the table knows. Two locks of one owner never conflict.
pub fn mdl_conflicts(object: &str, requested: &str, granted: &str) -> Option<bool> {
    // `BACKUP LOCK` and `BACKUP_LOCK` alike.
    let kind = object.trim().replace('_', " ").to_ascii_uppercase();
    let r = MdlType::of(requested)?;
    let g = MdlType::of(granted)?;
    if SCOPED.contains(&kind.as_str()) {
        let (r, g) = (r.scoped()?, g.scoped()?);
        Some(SCOPED_INCOMPATIBLE[r] & (1 << g) != 0)
    } else if OBJECT.contains(&kind.as_str()) {
        let (r, g) = (r.object()?, g.object()?);
        Some(OBJECT_INCOMPATIBLE[r] & (1 << g) != 0)
    } else {
        None
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
}

/// The edges a tick's reads tell: the blockers the session list names (PostgreSQL), and the
/// lock read's holders where it names them ([`LockList::blocks`]). A prepared transaction the
/// session list names is told apart by the lock read when it has the waiter's wait.
///
/// The lock read may be older than the session list (it is read less often): a wait of a
/// session the list shows not waiting for a lock is over and left out, and so is one of a
/// session it does not list when it lists every session (`complete`).
pub fn edges(sessions: Option<&SessionList>, locks: Option<&LockList>, complete: bool) -> Waits {
    let mut out = Vec::new();
    let mut alone = Vec::new();
    let mut named: HashSet<i64> = HashSet::new();
    if let Some(list) = sessions {
        for r in &list.rows {
            let Some(blockers) = &r.blocked_by else { continue };
            named.insert(r.key.id);
            if blockers.is_empty() {
                alone.push(r.key.id);
            }
            for b in blockers {
                match b {
                    Blocker::Session(id) => push(&mut out, r.key.id, Holder::Session(*id), Blame::Proven),
                    Blocker::PreparedTransaction => {
                        let mut found = false;
                        let waits = locks.into_iter().flat_map(|l| l.waits.iter()).filter(|w| w.waiter == r.key);
                        for w in waits {
                            for h in &w.holders {
                                if let Holder::Prepared(Some(_)) = h.holder {
                                    found = true;
                                    push(&mut out, r.key.id, h.holder.clone(), Blame::Proven);
                                }
                            }
                        }
                        if !found {
                            push(&mut out, r.key.id, Holder::Prepared(None), Blame::Proven);
                        }
                    }
                }
            }
        }
    }
    let Some(locks) = locks.filter(|l| l.blocks) else { return Waits { edges: out, alone } };
    for w in &locks.waits {
        if named.contains(&w.waiter.id) {
            continue;
        }
        if let Some(list) = sessions {
            match list.rows.iter().find(|r| r.key.id == w.waiter.id) {
                Some(r) if !r.waits_for_lock() => continue,
                None if complete => continue,
                _ => {}
            }
        }
        for h in &w.holders {
            if h.holder != Holder::Session(w.waiter.id) {
                push(&mut out, w.waiter.id, h.holder.clone(), h.blame);
            }
        }
    }
    for w in &locks.waits {
        let id = w.waiter.id;
        let shown = sessions.is_none_or(|list| match list.rows.iter().find(|r| r.key.id == id) {
            Some(r) => r.waits_for_lock(),
            None => !complete,
        });
        if shown && !named.contains(&id) && !out.iter().any(|e| e.waiter == id) && !alone.contains(&id) {
            alone.push(id);
        }
    }
    Waits { edges: out, alone }
}

fn push(out: &mut Vec<Edge>, waiter: i64, holder: Holder, blame: Blame) {
    match out.iter_mut().find(|e| e.waiter == waiter && e.holder == holder) {
        // The surer of two reasons a holder blocks.
        Some(e) if rank(blame) < rank(e.blame) => e.blame = blame,
        Some(_) => {}
        None => out.push(Edge { waiter, holder, blame }),
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
        let mut alone = waits.alone.clone();
        alone.sort_unstable();
        for id in alone {
            let node = Holder::Session(id);
            if tree.lines.iter().any(|l| l.node == node) {
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
            Holder::Transaction(t) => (2, 0, t.clone()),
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
            sure &= blame == Blame::Proven || (blame == Blame::Incompatible && only);
            i = p;
            steps += 1;
            if steps > self.lines.len() {
                return None;
            }
        }
        Some((i, sure))
    }
}
