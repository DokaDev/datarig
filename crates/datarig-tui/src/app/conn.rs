//! Connection state per profile: each profile has its own metadata session, schema
//! tree and completion catalog. Tabs borrow a profile's connection; each opens its own query
//! session. Events and password commands carry the generation of the attempt that started
//! them, so what an older attempt of the same profile sends is dropped.

use super::{Notice, TabId};
use crate::widgets::tree::Tree;
use datarig_core::driver::{KeyCatalog, KeyMarks, Session};
use datarig_core::profile::{ConnectionConfig, ProfileId};
use datarig_core::sql::complete::Catalog;
use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;
use std::time::Instant;

/// A connection attempt in progress (a spinner; `Esc` cancels it).
#[derive(Clone, Debug)]
pub struct Connecting {
    pub name: String,
    pub started: Instant,
    /// A password was sent (otherwise an auth failure means "no saved password").
    pub had_password: bool,
    /// Its password is being read from the keychain (nothing was sent to the server yet):
    /// `Ctrl+C` in a tab of the profile cancels the attempt too.
    pub keychain: bool,
    /// Its SSH tunnel is opening, at this stage.
    pub tunnel: Option<datarig_ssh::tunnel::Stage>,
}

/// A statement run in a tab whose profile was not connected yet: it runs once that profile
/// connects, and only if tab `tab` still exists, is still bound to `profile` and still has
/// the tab generation `binding` it had when it was queued. Anything else drops it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Queued {
    pub tab: TabId,
    pub profile: ProfileId,
    pub binding: u64,
    pub statements: Vec<String>,
}

/// The key constraints of a profile's tables. Not knowing them is never taken for "no keys":
/// the grid then shows no marks, the inspector says they are not known and a copy as SQL
/// INSERT does not name a table.
#[derive(Clone, Debug, Default)]
pub enum Keys {
    /// Not read yet (or being read again after a refresh), or the driver cannot tell.
    #[default]
    Unknown,
    Loaded(KeyCatalog),
    /// Reading them failed: why.
    Failed(String),
}

impl Keys {
    /// The catalog, when it was read.
    pub fn catalog(&self) -> Option<&KeyCatalog> {
        match self {
            Keys::Loaded(k) => Some(k),
            _ => None,
        }
    }

    /// The marks of the column `origin` names (none while the keys are not known).
    pub fn marks(&self, origin: Option<&datarig_core::driver::ColumnOrigin>) -> KeyMarks {
        self.catalog().map(|k| k.marks(origin)).unwrap_or_default()
    }
}

/// Where a profile's connection is (the explorer's node state).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeState {
    Disconnected,
    Connecting,
    Connected,
    Failed,
}

/// The connection of one profile.
pub struct ProfileConn {
    /// The profile's metadata session (tree, catalog).
    pub meta: Option<Session>,
    /// Generation of the current attempt: events and password commands of older attempts of
    /// this profile are dropped.
    pub generation: u64,
    /// The metadata session reported `Connected`.
    pub connected: bool,
    /// Connection attempt in progress.
    pub connecting: Option<Connecting>,
    /// Why the last attempt failed or the connection was lost (the explorer's error line).
    pub error: Option<Notice>,
    pub tree: Tree,
    pub catalog: Catalog,
    /// Why the catalog could not be read (the last one read, if any, stays).
    pub catalog_error: Option<String>,
    /// The key constraints of the profile's tables, read once per
    /// connection and again on a schema refresh.
    pub keys: Keys,
    /// The profile with the password of the current attempt: tabs open their query sessions
    /// with it, without asking the password source again.
    pub resolved: Option<ConnectionConfig>,
    /// Statements that wait for the connection, at most one per tab (run once it is
    /// connected, see [`Queued`]).
    pub pending: Vec<Queued>,
    /// The profile's node shows its schema tree (while connected).
    pub expanded: bool,
    /// Expand the node once the current attempt connects (Enter / `l` in the explorer).
    pub expand_on_connect: bool,
    /// Open a console tab once the current attempt connects, if the profile has no tab yet; `true`: and focus its editor (quick connect).
    pub console: Option<bool>,
    /// The database the metadata session works in, as the server said.
    pub database: Option<String>,
    /// The server's databases (`DbCommand::LoadDatabases`), once asked: the names, or why they
    /// could not be read.
    pub databases: Option<Result<Vec<String>, String>>,
    /// The explorer asked for [`ProfileConn::databases`] and the answer has not come (step
    /// 2.7.2: they are asked for once when the profile's node opens).
    pub databases_asked: bool,
    /// The explorer's node of the profile's own database is open (its schema tree shows).
    pub own_open: bool,
    /// The explorer's nodes of other databases that are open (their aux session's tree shows).
    pub open_dbs: BTreeSet<String>,
    /// A session of the current attempt turned its statement cache off and the status bar said
    /// so: the other sessions of the profile say it in their Messages only.
    pub cache_off_said: bool,
    /// The profile's open SSH tunnel.
    pub tunnel: Option<super::tunnel::ProfileTunnel>,
    /// The profile with its database password, waiting for the tunnel to open.
    pub tunnel_wait: Option<ConnectionConfig>,
    /// Why its tunnel ended, while the profile is not connected again.
    pub tunnel_lost: Option<Notice>,
}

impl ProfileConn {
    fn new() -> Self {
        Self {
            meta: None,
            generation: 0,
            connected: false,
            connecting: None,
            error: None,
            tree: Tree::new(),
            catalog: Catalog::default(),
            catalog_error: None,
            keys: Keys::Unknown,
            resolved: None,
            pending: Vec::new(),
            expanded: false,
            expand_on_connect: false,
            console: None,
            database: None,
            databases: None,
            databases_asked: false,
            own_open: true,
            open_dbs: BTreeSet::new(),
            cache_off_said: false,
            tunnel: None,
            tunnel_wait: None,
            tunnel_lost: None,
        }
    }

    /// Back to "not connected": the session state, the tree and what waited for the attempt
    /// are dropped (the sessions must be closed already).
    pub fn reset(&mut self) {
        let generation = self.generation;
        *self = Self::new();
        self.generation = generation;
    }

    pub fn state(&self) -> NodeState {
        if self.connecting.is_some() {
            NodeState::Connecting
        } else if self.connected {
            NodeState::Connected
        } else if self.error.is_some() {
            NodeState::Failed
        } else {
            NodeState::Disconnected
        }
    }
}

static EMPTY_CATALOG: LazyLock<Catalog> = LazyLock::new(Catalog::default);
static UNKNOWN_KEYS: LazyLock<Keys> = LazyLock::new(|| Keys::Unknown);

/// How long an aux metadata session stays open when no tab works in its database and nothing
/// used it. Fixed; the policy's `paging_idle_timeout` is about portals.
pub const AUX_IDLE: std::time::Duration = std::time::Duration::from_secs(60);

/// A metadata session of a profile in another database than the profile's own (a
/// database is a connection of its own in PostgreSQL): that database's schemas for the picker
/// and the explorer, its completion catalog and its keys for the tabs that work there. Opened on
/// demand once the profile is connected, closed with the profile's sessions, and closed after
/// [`AUX_IDLE`] when no tab works there and nothing used it; what it read stays,
/// and the next use opens it again (as it does after a failure or a lost connection).
pub struct AuxMeta {
    /// Its id: events carry it (`EventTarget::Aux`); an id no aux has any more is stale. A
    /// session opened again gets a new one.
    pub id: u64,
    pub profile: ProfileId,
    pub database: String,
    pub session: Option<Session>,
    /// The database's schemas, once read, or why they could not be (unknown until then).
    pub schemas: Option<Result<Vec<String>, String>>,
    pub catalog: Catalog,
    pub keys: Keys,
    /// When something last used it (the picker, the explorer, completion, a run).
    pub used: Instant,
    /// The database's schema tree in the explorer, as the profile's own has.
    pub tree: Tree,
}

#[derive(Default)]
pub struct ConnectionManager {
    conns: HashMap<ProfileId, ProfileConn>,
    /// Last generation handed out (shared by all profiles, never reused).
    generation: u64,
    /// Metadata sessions in other databases.
    aux: Vec<AuxMeta>,
    aux_seq: u64,
}

impl ConnectionManager {
    pub fn get(&self, id: ProfileId) -> Option<&ProfileConn> {
        self.conns.get(&id)
    }

    pub fn get_mut(&mut self, id: ProfileId) -> Option<&mut ProfileConn> {
        self.conns.get_mut(&id)
    }

    /// The connection of profile `id`, created disconnected on first use.
    pub fn entry(&mut self, id: ProfileId) -> &mut ProfileConn {
        self.conns.entry(id).or_insert_with(ProfileConn::new)
    }

    /// Forget profile `id`'s connection (its sessions must be closed already).
    pub fn remove(&mut self, id: ProfileId) -> Option<ProfileConn> {
        self.conns.remove(&id)
    }

    /// Start a new generation for profile `id`: whatever its older attempts send from now on
    /// is dropped.
    pub fn next_generation(&mut self, id: ProfileId) -> u64 {
        self.generation += 1;
        let generation = self.generation;
        self.entry(id).generation = generation;
        generation
    }

    /// An event of generation `generation` of profile `id` belongs to its current attempt.
    pub fn is_current(&self, id: ProfileId, generation: u64) -> bool {
        self.conns.get(&id).is_some_and(|c| c.generation == generation)
    }

    pub fn state(&self, id: ProfileId) -> NodeState {
        self.conns.get(&id).map_or(NodeState::Disconnected, ProfileConn::state)
    }

    pub fn is_connected(&self, id: ProfileId) -> bool {
        self.conns.get(&id).is_some_and(|c| c.connected)
    }

    /// The most recently started attempt of any profile.
    pub fn attempt(&self) -> Option<&Connecting> {
        self.conns.values().filter_map(|c| c.connecting.as_ref()).max_by_key(|c| c.started)
    }

    /// Profiles with an attempt in progress.
    pub fn connecting(&self) -> impl Iterator<Item = (ProfileId, &Connecting)> {
        self.conns.iter().filter_map(|(id, c)| c.connecting.as_ref().map(|a| (*id, a)))
    }

    /// The completion catalog of profile `id` (empty when it has none).
    pub fn catalog(&self, id: Option<ProfileId>) -> &Catalog {
        id.and_then(|id| self.conns.get(&id)).map_or(&EMPTY_CATALOG, |c| &c.catalog)
    }

    /// The metadata session of `profile` in `database`, if one was opened.
    pub fn aux(&self, profile: ProfileId, database: &str) -> Option<&AuxMeta> {
        self.aux.iter().find(|a| a.profile == profile && a.database == database)
    }

    /// The metadata session of `profile` in `database`, to change.
    pub fn aux_mut(&mut self, profile: ProfileId, database: &str) -> Option<&mut AuxMeta> {
        self.aux.iter_mut().find(|a| a.profile == profile && a.database == database)
    }

    /// The aux session with id `id`, to read.
    pub fn aux_by_id_ref(&self, id: u64) -> Option<&AuxMeta> {
        self.aux.iter().find(|a| a.id == id)
    }

    /// The aux session with id `id`.
    pub fn aux_by_id(&mut self, id: u64) -> Option<&mut AuxMeta> {
        self.aux.iter_mut().find(|a| a.id == id)
    }

    /// A new aux entry of `profile` in `database` (its session set by the caller); its id.
    pub fn add_aux(&mut self, profile: ProfileId, database: &str, now: Instant) -> u64 {
        self.aux_seq += 1;
        let id = self.aux_seq;
        self.aux.push(AuxMeta {
            id,
            profile,
            database: database.to_string(),
            session: None,
            schemas: None,
            catalog: Catalog::default(),
            keys: Keys::Unknown,
            used: now,
            tree: Tree::new(),
        });
        id
    }

    /// A new id for aux `id`, whose session opens again (what the old one still sends is
    /// dropped); the new id.
    pub fn renew_aux(&mut self, id: u64) -> Option<u64> {
        self.aux_seq += 1;
        let next = self.aux_seq;
        let a = self.aux.iter_mut().find(|a| a.id == id)?;
        a.id = next;
        Some(next)
    }

    /// The aux sessions, to read.
    pub fn auxes(&self) -> impl Iterator<Item = &AuxMeta> {
        self.aux.iter()
    }

    /// Close and forget the aux sessions of `profile` (every profile: `None`).
    pub fn close_aux(&mut self, profile: Option<ProfileId>) {
        let (gone, keep): (Vec<AuxMeta>, Vec<AuxMeta>) =
            std::mem::take(&mut self.aux).into_iter().partition(|a| profile.is_none_or(|p| a.profile == p));
        self.aux = keep;
        for a in gone {
            if let Some(s) = a.session {
                s.close();
            }
        }
    }

    /// The keys that apply to a result of a tab of `profile` in `database` (`None`: the
    /// profile's own): another database's only from its own session, never the profile's
    /// (table ids of one database mean nothing in another); unknown until read.
    pub fn keys_in(&self, profile: Option<ProfileId>, database: Option<&str>) -> &Keys {
        let Some(p) = profile else { return &UNKNOWN_KEYS };
        match database {
            None => self.conns.get(&p).map_or(&UNKNOWN_KEYS, |c| &c.keys),
            Some(d) => self.aux(p, d).map_or(&UNKNOWN_KEYS, |a| &a.keys),
        }
    }

    /// The completion catalog of `profile` in `database` (`None`: the profile's own); empty
    /// while another database's is not read.
    pub fn catalog_in(&self, profile: Option<ProfileId>, database: Option<&str>) -> &Catalog {
        match database {
            None => self.catalog(profile),
            Some(d) => profile.and_then(|p| self.aux(p, d)).map_or(&EMPTY_CATALOG, |a| &a.catalog),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (ProfileId, &ProfileConn)> {
        self.conns.iter().map(|(id, c)| (*id, c))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (ProfileId, &mut ProfileConn)> {
        self.conns.iter_mut().map(|(id, c)| (*id, c))
    }
}
