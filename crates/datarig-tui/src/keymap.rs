//! Context keymap. One table of default bindings
//! ([`defaults`]) plus the user's `[keymap.<context>]` entries decide what a key does in the
//! current [`Ctx`]:
//!
//! * [`Keymap::feed`] resolves keys innermost context first and collects key sequences
//!   (`g g`, `Space c n`). The result is an action, "wait for more keys", keys to hand to the
//!   focused widget (reserved keys such as the editor's vim commands), or nothing.
//! * [`check`] finds conflicts; tests run it on the defaults, startup runs it on user entries.
//! * [`doc`] renders `docs/keybindings.md`.
//!
//! The command line shows the keys of an action through [`Keymap::keys_label`], so remapped keys
//! appear everywhere at once.

mod check;
mod context;
mod defaults;
pub mod doc;
pub mod keys;

pub use check::{Conflict, ConflictKind, check, check_contexts, check_ids};
pub use context::Ctx;
pub use defaults::{BindTarget, Binding, HINTS, LEADER, PROTECTED, VIM_BASICS, defaults};
pub use keys::{KeyChord, KeyError, parse_keys};

use crate::app::action::{self, Action};
use datarig_core::config::KeymapConfig;
use datarig_core::i18n::Label;

/// What a bound key sequence does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Action(Action),
    /// Handed to the focused widget (the note says whose key it is).
    Reserved(&'static str),
    /// Unbound by the user (`"none"`): hides an outer binding of the same keys.
    Disabled,
    /// A leader group: a prefix of longer bindings, with its label.
    Group(Label),
}

/// What the next key after a prefix leads to (the which-key popup lists these).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Child {
    Action(Action),
    /// A group; its label, if the table names it.
    Group(Option<Label>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bound {
    pub ctx: Ctx,
    pub keys: Vec<KeyChord>,
    pub target: Target,
    /// Comes from the user's config.
    pub user: bool,
}

/// Keys typed so far of an unfinished sequence.
#[derive(Clone, Debug, Default)]
pub struct KeyState {
    pending: Vec<KeyChord>,
    ctx: Option<Ctx>,
}

impl KeyState {
    pub fn clear(&mut self) {
        self.pending.clear();
        self.ctx = None;
    }

    pub fn pending(&self) -> &[KeyChord] {
        &self.pending
    }

    /// The context the pending keys were typed in.
    pub fn ctx(&self) -> Option<Ctx> {
        self.ctx
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    Action(Action),
    /// A prefix of a longer binding: wait for the next key.
    Pending,
    /// Keys for the focused widget (reserved keys, or keys no binding claims).
    Forward(Vec<KeyChord>),
    /// Nothing happens (an unfinished leader sequence, or keys the user unbound).
    Unbound(Vec<KeyChord>),
}

/// A `[keymap.*]` entry that was not applied (or replaced a default), for a startup notice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub ctx: String,
    pub key: String,
    pub kind: IssueKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueKind {
    UnknownContext,
    BadKey(keys::KeyError),
    UnknownAction(String),
    /// A character key or `Space` in a text input context.
    TextKey,
    /// Would take a reserved key (the editor's, a dialog's or a later action's).
    Reserved,
    /// Would hide a protected app key.
    Protected,
    /// One sequence would be a prefix of another (`other`).
    Prefix(String),
    /// Applied; the default action `old` of that key is gone.
    Replaced {
        action: String,
        old: String,
    },
}

/// One action of a keyboard help section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelpEntry {
    pub action: Action,
    pub keys: Vec<Vec<KeyChord>>,
    /// Keys of this action that the editor uses instead (shown as such).
    pub editor_keys: Vec<Vec<KeyChord>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keymap {
    bindings: Vec<Bound>,
}

/// Resolve the default table. Entries whose keys do not parse or whose action id is unknown
/// are skipped here; the unit tests keep the table free of both.
fn resolve(raw: &[Binding]) -> Vec<Bound> {
    raw.iter()
        .filter_map(|b| {
            let keys = parse_keys(b.keys).ok()?;
            let target = match b.target {
                BindTarget::Action(id) => Target::Action(action::by_id(id)?.action),
                BindTarget::Reserved(note) => Target::Reserved(note),
                BindTarget::Group(label) => Target::Group(label),
            };
            Some(Bound { ctx: b.ctx, keys, target, user: false })
        })
        .collect()
}

impl Default for Keymap {
    fn default() -> Self {
        Self { bindings: resolve(&defaults()) }
    }
}

impl Keymap {
    pub fn from_bindings(bindings: Vec<Bound>) -> Self {
        Self { bindings }
    }

    pub fn bindings(&self) -> &[Bound] {
        &self.bindings
    }

    /// The defaults with the user's `[keymap.<context>]` entries applied. Bad entries are
    /// skipped and reported; the rest still apply (the app always starts).
    pub fn with_user(cfg: &KeymapConfig) -> (Self, Vec<Issue>) {
        let mut km = Self::default();
        let mut issues = Vec::new();
        for (ctx_name, entries) in cfg {
            let Some(ctx) = Ctx::from_name(ctx_name) else {
                issues.push(Issue { ctx: ctx_name.clone(), key: String::new(), kind: IssueKind::UnknownContext });
                continue;
            };
            for (key, value) in entries {
                if let Some(kind) = km.apply_user(ctx, key, value) {
                    issues.push(Issue { ctx: ctx_name.clone(), key: key.clone(), kind });
                }
            }
        }
        (km, issues)
    }

    /// Apply one user entry; returns why it was skipped, or that it replaced a default.
    fn apply_user(&mut self, ctx: Ctx, key: &str, value: &str) -> Option<IssueKind> {
        let keys = match parse_keys(key) {
            Ok(k) => k,
            Err(e) => return Some(IssueKind::BadKey(e)),
        };
        let target = if value == "none" {
            Target::Disabled
        } else {
            match action::by_id(value) {
                Some(s) => Target::Action(s.action),
                None => return Some(IssueKind::UnknownAction(value.to_string())),
            }
        };
        if ctx.is_text_input() && (keys[0] == KeyChord::char(' ') || keys.iter().any(KeyChord::is_plain_char)) {
            return Some(IssueKind::TextKey);
        }
        let same = |b: &Bound| b.ctx == ctx && b.keys == keys;
        if self.bindings.iter().any(|b| same(b) && matches!(b.target, Target::Reserved(_))) {
            return Some(IssueKind::Reserved);
        }
        if target == Target::Disabled && !self.bindings.iter().any(|b| ctx.chain().contains(&b.ctx) && b.keys == keys) {
            // Nothing to unbind in this context.
            return None;
        }
        let before = self.bindings.clone();
        let old: Vec<Action> = self
            .bindings
            .iter()
            .filter(|b| same(b))
            .filter_map(|b| if let Target::Action(a) = b.target { Some(a) } else { None })
            .collect();
        self.bindings.retain(|b| !same(b));
        let new = Bound { ctx, keys, target, user: true };
        self.bindings.push(new.clone());
        let problem = check(&self.bindings).into_iter().find(|c| c.involves(&new) && c.kind != ConflictKind::Shadow);
        if let Some(c) = problem {
            self.bindings = before;
            return Some(match c.kind {
                ConflictKind::Protected => IssueKind::Protected,
                ConflictKind::Prefix => IssueKind::Prefix(c.other_keys(&new)),
                ConflictKind::TextKey => IssueKind::TextKey,
                _ => IssueKind::Reserved,
            });
        }
        match (old.first(), target) {
            (Some(o), Target::Action(a)) if *o != a => Some(IssueKind::Replaced {
                action: action::spec(a).id.to_string(),
                old: action::spec(*o).id.to_string(),
            }),
            _ => None,
        }
    }

    /// The binding of exactly `seq` seen from `ctx`, and whether a longer one starts with it.
    fn lookup(&self, ctx: Ctx, seq: &[KeyChord]) -> (Option<&Bound>, bool) {
        let mut exact = None;
        let mut longer = false;
        for c in ctx.chain() {
            for b in self.bindings.iter().filter(|b| b.ctx == c) {
                if matches!(b.target, Target::Group(_)) {
                    // A group only labels a prefix; its members make the sequence pending.
                    continue;
                }
                if b.keys == seq {
                    exact = exact.or(Some(b));
                } else if b.keys.len() > seq.len() && b.keys.starts_with(seq) {
                    longer = true;
                }
            }
        }
        (exact, longer)
    }

    /// What exactly `seq` is bound to in `ctx`, and whether a longer binding starts with it.
    pub fn resolve_seq(&self, ctx: Ctx, seq: &[KeyChord]) -> (Option<Target>, bool) {
        let (exact, longer) = self.lookup(ctx, seq);
        (exact.map(|b| b.target), longer)
    }

    /// The keys that may follow `prefix` in `ctx`, sorted by key: actions and groups that
    /// lead to at least one action. Reserved keys and unbound (`"none"`) keys are left out.
    pub fn children(&self, ctx: Ctx, prefix: &[KeyChord]) -> Vec<(KeyChord, Child)> {
        let mut seen: Vec<KeyChord> = Vec::new();
        let mut out: Vec<(KeyChord, Child)> = Vec::new();
        for c in ctx.chain() {
            for b in self.bindings.iter().filter(|b| b.ctx == c) {
                if b.keys.len() <= prefix.len() || !b.keys.starts_with(prefix) {
                    continue;
                }
                let next = b.keys[prefix.len()];
                let leaf = b.keys.len() == prefix.len() + 1;
                if seen.contains(&next) {
                    continue;
                }
                match b.target {
                    Target::Group(_) => continue,
                    Target::Action(a) if leaf => out.push((next, Child::Action(a))),
                    Target::Action(_) => {
                        let mut group = prefix.to_vec();
                        group.push(next);
                        out.push((next, Child::Group(self.group_label(ctx, &group))));
                    }
                    // The key is taken, but not by an action.
                    Target::Reserved(_) | Target::Disabled if leaf => {}
                    Target::Reserved(_) | Target::Disabled => continue,
                }
                seen.push(next);
            }
        }
        out.sort_by_key(|(k, _)| k.label());
        out
    }

    /// Label of the group `keys` in `ctx`, if the table names one.
    pub fn group_label(&self, ctx: Ctx, keys: &[KeyChord]) -> Option<Label> {
        let chain = ctx.chain();
        self.bindings.iter().find_map(|b| match b.target {
            Target::Group(l) if b.keys == keys && chain.contains(&b.ctx) => Some(l),
            _ => None,
        })
    }

    /// Whether `ctx` itself (not its ancestors) binds the single key `k`.
    pub fn binds(&self, ctx: Ctx, k: &KeyChord) -> bool {
        self.bindings.iter().any(|b| b.ctx == ctx && b.keys.len() == 1 && b.keys[0] == *k)
    }

    /// Resolve key `k` in context `ctx`, continuing the sequence in `st`.
    pub fn feed(&self, st: &mut KeyState, ctx: Ctx, k: KeyChord) -> Resolved {
        if st.ctx != Some(ctx) {
            st.clear();
        }
        let mut seq = std::mem::take(&mut st.pending);
        seq.push(k);
        let (exact, longer) = self.lookup(ctx, &seq);
        if let Some(b) = exact {
            st.clear();
            return match b.target {
                Target::Action(a) => Resolved::Action(a),
                Target::Reserved(_) => Resolved::Forward(seq),
                Target::Disabled | Target::Group(_) => Resolved::Unbound(seq),
            };
        }
        if longer {
            st.pending = seq;
            st.ctx = Some(ctx);
            return Resolved::Pending;
        }
        st.clear();
        let leader = parse_keys(LEADER).ok().and_then(|l| l.first().copied());
        if seq.len() > 1 && seq.first().copied() == leader { Resolved::Unbound(seq) } else { Resolved::Forward(seq) }
    }

    /// Key sequences that run `a` in `ctx`: global keys first, keys hidden by an inner binding
    /// left out. When `ctx` has none, the keys of the first context that binds `a`.
    pub fn keys_for(&self, a: Action, ctx: Ctx) -> Vec<Vec<KeyChord>> {
        let chain = ctx.chain();
        let mut out = Vec::new();
        for (depth, c) in chain.iter().enumerate().rev() {
            for b in self.bindings.iter().filter(|b| b.ctx == *c && b.target == Target::Action(a)) {
                let hidden = chain[..depth]
                    .iter()
                    .any(|inner| self.bindings.iter().any(|x| x.ctx == *inner && x.keys == b.keys));
                if !hidden && !out.contains(&b.keys) {
                    out.push(b.keys.clone());
                }
            }
        }
        if out.is_empty()
            && let Some(first) = self.bindings.iter().find(|b| b.target == Target::Action(a))
        {
            out = self
                .bindings
                .iter()
                .filter(|b| b.ctx == first.ctx && b.target == Target::Action(a))
                .map(|b| b.keys.clone())
                .collect();
        }
        out
    }

    /// The keys the hint line shows for `a` in `ctx`: the innermost single key that is not
    /// hidden by an inner binding, else the innermost sequence (`F1` rather than `Space ?`).
    /// `Ctrl+Enter` needs the kitty keyboard protocol (`enhanced`).
    pub fn hint_keys(&self, a: Action, ctx: Ctx, enhanced: bool) -> Option<Vec<KeyChord>> {
        let chain = ctx.chain();
        let ctrl_enter = parse_keys("ctrl+enter").ok()?;
        let mut sequence = None;
        for (depth, c) in chain.iter().enumerate() {
            for b in self.bindings.iter().filter(|b| b.ctx == *c && b.target == Target::Action(a)) {
                let hidden = chain[..depth]
                    .iter()
                    .any(|inner| self.bindings.iter().any(|x| x.ctx == *inner && x.keys == b.keys));
                if hidden || (!enhanced && b.keys == ctrl_enter) {
                    continue;
                }
                if b.keys.len() == 1 {
                    return Some(b.keys.clone());
                }
                sequence.get_or_insert_with(|| b.keys.clone());
            }
        }
        sequence
    }

    /// The actions bound in `ctx` itself, each with its keys, for the keyboard help. With
    /// `from` (the context the help was opened in), keys hidden by a context inside `from`'s
    /// chain are left out, except keys the editor takes, which come back separately.
    pub fn section(&self, ctx: Ctx, from: Option<Ctx>) -> Vec<HelpEntry> {
        let chain = from.map(Ctx::chain).unwrap_or_default();
        let inner: &[Ctx] = match chain.iter().position(|c| *c == ctx) {
            Some(d) => &chain[..d],
            None => &[],
        };
        let mut out: Vec<HelpEntry> = Vec::new();
        for b in self.bindings.iter().filter(|b| b.ctx == ctx) {
            let Target::Action(a) = b.target else { continue };
            let hider = inner.iter().find(|c| self.bindings.iter().any(|x| x.ctx == **c && x.keys == b.keys));
            let at = match out.iter().position(|e| e.action == a) {
                Some(i) => i,
                None => {
                    out.push(HelpEntry { action: a, keys: Vec::new(), editor_keys: Vec::new() });
                    out.len() - 1
                }
            };
            match hider {
                None => out[at].keys.push(b.keys.clone()),
                Some(c) if c.is_editor() => out[at].editor_keys.push(b.keys.clone()),
                Some(_) => {}
            }
        }
        out.retain(|e| !e.keys.is_empty() || !e.editor_keys.is_empty());
        out
    }

    /// `keys_for` as display text: `Ctrl+Enter / Ctrl+E`.
    pub fn keys_label(&self, a: Action, ctx: Ctx) -> String {
        self.keys_for(a, ctx).iter().map(|k| keys::label(k)).collect::<Vec<_>>().join(" / ")
    }

    /// [`Keymap::keys_label`] with only the keys this terminal can send ([`works`]).
    pub fn keys_label_in(&self, a: Action, ctx: Ctx, enhanced: bool) -> String {
        let keys = self.keys_for(a, ctx).into_iter().filter(|k| works(k, enhanced));
        keys.map(|k| keys::label(&k)).collect::<Vec<_>>().join(" / ")
    }
}

/// Whether the terminal can send `keys`: `Ctrl+Enter` needs the kitty keyboard protocol
/// (`enhanced`); without it the terminal sends plain `Enter` (iTerm2 over SSH may
/// not grant it).
pub fn works(keys: &[KeyChord], enhanced: bool) -> bool {
    enhanced || parse_keys("ctrl+enter").ok().is_none_or(|ce| keys != ce.as_slice())
}

#[cfg(test)]
mod tests;
