//! Key conflict check. For every context, the bindings of the context and its
//! ancestors must not:
//!
//! 1. bind the same keys twice in one context ([`ConflictKind::Duplicate`]);
//! 2. hide an ancestor's keys, unless the inner context is the editor, the inner binding is a
//!    user's `"none"`, or it is the inspector's `Tab`/`Shift+Tab` over the pane keys
//!    ([`ConflictKind::Shadow`], see [`tabs_its_own`]);
//! 3. make one sequence a prefix of another, e.g. `g` and `g t` ([`ConflictKind::Prefix`]);
//! 4. put an app binding on a reserved key ([`ConflictKind::Reserved`]);
//! 5. name an action that does not exist ([`ConflictKind::UnknownAction`], see [`check_ids`]);
//! 6. bind a character key or a `Space` sequence in a text input context
//!    ([`ConflictKind::TextKey`]);
//! 7. hide a protected key, even from the editor ([`ConflictKind::Protected`]).

use super::defaults::{BindTarget, Binding, PROTECTED};
use super::{Bound, Ctx, KeyChord, Target, keys, parse_keys};
use crate::app::action;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConflictKind {
    Duplicate,
    Shadow,
    Prefix,
    Reserved,
    UnknownAction,
    TextKey,
    Protected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub kind: ConflictKind,
    /// The context whose effective bindings clash.
    pub ctx: Ctx,
    pub a: Bound,
    /// The other binding (`None` for problems of a single binding).
    pub b: Option<Bound>,
}

fn describe(b: &Bound) -> String {
    let what = match b.target {
        Target::Action(a) => action::spec(a).id.to_string(),
        Target::Reserved(note) => format!("reserved: {note}"),
        Target::Disabled => "none".to_string(),
        Target::Group(label) => format!("group {}", label.key()),
    };
    format!("[{}] {} = {what}", b.ctx.name(), keys::notation(&b.keys))
}

impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} in {}: {}", self.kind, self.ctx.name(), describe(&self.a))?;
        if let Some(b) = &self.b {
            write!(f, " vs {}", describe(b))?;
        }
        Ok(())
    }
}

impl Conflict {
    pub fn involves(&self, x: &Bound) -> bool {
        &self.a == x || self.b.as_ref() == Some(x)
    }

    /// The keys of the binding that is not `x`.
    pub fn other_keys(&self, x: &Bound) -> String {
        let other = if &self.a == x { self.b.as_ref() } else { Some(&self.a) };
        other.map(|o| keys::notation(&o.keys)).unwrap_or_default()
    }
}

fn protected() -> Vec<KeyChord> {
    PROTECTED.iter().filter_map(|k| parse_keys(k).ok()).map(|k| k[0]).collect()
}

fn reserved(b: &Bound) -> bool {
    matches!(b.target, Target::Reserved(_))
}

/// Conflicts among resolved bindings, seen from every context.
pub fn check(bindings: &[Bound]) -> Vec<Conflict> {
    check_contexts(bindings, &Ctx::ALL)
}

/// Conflicts seen from `contexts` (e.g. the contexts of one editor mode).
pub fn check_contexts(bindings: &[Bound], contexts: &[Ctx]) -> Vec<Conflict> {
    let protected = protected();
    let mut out: Vec<Conflict> = Vec::new();
    let mut push = |c: Conflict| {
        let dup = out.iter().any(|o| {
            o.kind == c.kind
                && ((o.a == c.a && o.b == c.b) || (Some(&o.a) == c.b.as_ref() && o.b.as_ref() == Some(&c.a)))
        });
        if !dup {
            out.push(c);
        }
    };
    for &leaf in contexts {
        let chain = leaf.chain();
        let depth = |c: Ctx| chain.iter().position(|x| *x == c);
        let eff: Vec<(&Bound, usize)> = bindings.iter().filter_map(|b| depth(b.ctx).map(|d| (b, d))).collect();
        for (i, &(x, dx)) in eff.iter().enumerate() {
            if dx == 0
                && x.ctx.is_text_input()
                && (x.keys[0] == KeyChord::char(' ') || x.keys.iter().any(KeyChord::is_plain_char))
            {
                push(Conflict { kind: ConflictKind::TextKey, ctx: leaf, a: x.clone(), b: None });
            }
            for &(y, dy) in &eff[i + 1..] {
                let same = x.keys == y.keys;
                let prefix = !same && (x.keys.starts_with(&y.keys) || y.keys.starts_with(&x.keys));
                if !same && !prefix {
                    continue;
                }
                let group = |b: &Bound| matches!(b.target, Target::Group(_));
                if group(x) || group(y) {
                    // A group is meant to be a prefix of longer bindings; anything else
                    // with the same keys (or shorter) clashes with it.
                    let (g, o) = if group(x) { (x, y) } else { (y, x) };
                    let fine = if group(o) { !same || dx != dy } else { o.keys.len() > g.keys.len() };
                    if !fine {
                        let outer = if dx < dy { y } else { x };
                        let kind = if dx != dy && protected.contains(&outer.keys[0]) {
                            ConflictKind::Protected
                        } else if group(o) {
                            ConflictKind::Duplicate
                        } else {
                            ConflictKind::Prefix
                        };
                        push(Conflict { kind, ctx: leaf, a: x.clone(), b: Some(y.clone()) });
                    }
                    continue;
                }
                // `inner` hides `outer` (same depth: neither).
                let (inner, outer) = if dx < dy { (x, y) } else { (y, x) };
                let kind = if dx != dy && protected.contains(&outer.keys[0]) {
                    Some(ConflictKind::Protected)
                } else if prefix {
                    Some(ConflictKind::Prefix)
                } else if dx == dy {
                    Some(if reserved(x) || reserved(y) { ConflictKind::Reserved } else { ConflictKind::Duplicate })
                } else if inner.target == Target::Disabled || inner.ctx.is_editor() || tabs_its_own(inner, outer) {
                    None
                } else if reserved(inner) || reserved(outer) {
                    Some(ConflictKind::Reserved)
                } else {
                    Some(ConflictKind::Shadow)
                };
                if let Some(kind) = kind {
                    push(Conflict { kind, ctx: leaf, a: inner.clone(), b: Some(outer.clone()) });
                }
            }
        }
    }
    out
}

/// The one pane whose `Tab` and `Shift+Tab` switch its own tabs instead of moving between
/// panes: the result inspector, once clicked. Leaving it stays one key away
/// (`Esc`, `F6`, `Shift+F6`), so hiding `pane.next`/`pane.prev` there is allowed; nothing else
/// may hide them.
fn tabs_its_own(inner: &Bound, outer: &Bound) -> bool {
    let pane = |t: Target| matches!(t, Target::Action(a) if matches!(a, action::Action::FocusNext | action::Action::FocusPrev));
    inner.ctx == Ctx::Inspector && pane(outer.target) && inner.target == Target::Action(action::Action::DetailTab)
}

/// Default-table entries whose keys do not parse or whose action id is unknown (the resolved
/// table silently drops them, so the tests check the raw table too).
pub fn check_ids(raw: &[Binding]) -> Vec<String> {
    raw.iter()
        .filter_map(|b| {
            if let Err(e) = parse_keys(b.keys) {
                return Some(format!("[{}] {:?}: {e}", b.ctx.name(), b.keys));
            }
            match b.target {
                BindTarget::Action(id) if action::by_id(id).is_none() => Some(format!(
                    "{:?} in {}: [{}] {} = {id}",
                    ConflictKind::UnknownAction,
                    b.ctx.name(),
                    b.ctx.name(),
                    b.keys
                )),
                _ => None,
            }
        })
        .collect()
}
