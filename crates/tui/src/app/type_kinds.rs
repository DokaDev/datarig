//! The kinds of a PostgreSQL database's own types that the connection reports as base types
//! (`Dated::Lookup`: an extension's `citext`, or a multirange of the database's own), asked
//! of the profile's metadata session once a result with such a column comes, and kept per
//! profile, database, connection (its generation), asking tab (its query session's
//! generation) and type: only a base type or an enum holds no dates. A column of a tab
//! working in another database than the metadata session's stays not known.

use super::App;
use super::conn::TypeKindKey;
use super::tabs::TabId;
use datarig_core::driver::{Dated, DbCommand, DbError};
use datarig_core::profile::ProfileId;

impl App {
    /// Tab `id`'s results came: the kinds of their columns' types to look up are asked of its
    /// profile's metadata session (those not asked yet), and those known are taken.
    pub(in crate::app) fn type_kinds_for_tab(&mut self, id: TabId) {
        let Some(t) = self.tabs.get(id) else { return };
        let Some(p) = t.profile else { return };
        if self.other_database(t).is_some() {
            return;
        }
        let db = self.own_database(p);
        let mut oids: Vec<u32> = Vec::new();
        let shown = match &t.results {
            super::Results::Rows(rs) => Some(rs.as_ref()),
            _ => None,
        };
        for rs in shown.into_iter().chain(t.exec.steps.values().map(|s| &s.rs)) {
            for c in &rs.columns {
                if let Dated::Lookup(oid) = c.meta.dated
                    && !oids.contains(&oid)
                {
                    oids.push(oid);
                }
            }
        }
        let session = t.exec.generation;
        let connected = self.conns.get(p).is_some_and(|c| c.meta.is_some());
        let c = self.conns.entry(p);
        let generation = c.generation;
        oids.retain(|oid| !c.type_kinds.contains_key(&(db.clone(), generation, id, session, *oid)));
        if connected && !oids.is_empty() {
            for oid in &oids {
                c.type_kinds.insert((db.clone(), generation, id, session, *oid), None);
            }
            // The metadata session answers for the database the tab's query session works in:
            // the same assumption the table's structure, keys and column types for edits rely on
            // (both sessions reach the same server).
            self.send_meta(p, DbCommand::LoadTypeKinds { oids });
        }
        self.take_type_kinds(p);
    }

    /// The metadata session of profile `p` answered the kinds of `oids`: a type it did not
    /// list no longer exists (not known); a read that failed leaves them to be asked again.
    pub(in crate::app) fn on_type_kinds(
        &mut self,
        p: ProfileId,
        oids: Vec<u32>,
        result: Result<Vec<(u32, char)>, DbError>,
    ) {
        let db = self.own_database(p);
        let c = self.conns.entry(p);
        // Events of an older connection of the profile are dropped before they come here: the
        // answer is the current connection's.
        let generation = c.generation;
        // What older connections answered is of their servers: gone.
        c.type_kinds.retain(|(_, g, ..), _| *g == generation);
        // The answer is every asking tab's (by its session then) still waiting for it.
        let waiting =
            |k: &TypeKindKey, v: &Option<char>, oid: u32| v.is_none() && k.0 == db && k.1 == generation && k.4 == oid;
        for oid in oids {
            let keys: Vec<TypeKindKey> =
                c.type_kinds.iter().filter(|(k, v)| waiting(k, v, oid)).map(|(k, _)| k.clone()).collect();
            for k in keys {
                match &result {
                    Ok(kinds) => {
                        let kind = kinds.iter().find(|(o, _)| *o == oid).map_or('?', |(_, k)| *k);
                        c.type_kinds.insert(k, Some(kind));
                    }
                    // Failed: asked again with the next result.
                    Err(_) => {
                        c.type_kinds.remove(&k);
                    }
                }
            }
        }
        self.take_type_kinds(p);
    }

    /// Every column of a type to look up, of a tab of profile `p` working in its own database,
    /// whose kind is known now: a base type or an enum holds no dates, any other kind (a
    /// multirange, a type that no longer exists) is not known.
    fn take_type_kinds(&mut self, p: ProfileId) {
        let db = self.own_database(p);
        let ids: Vec<(TabId, u64)> = (self.tabs.iter())
            .filter(|t| t.profile == Some(p) && self.other_database(t).is_none())
            .map(|t| (t.id, t.exec.generation))
            .collect();
        let Some(c) = self.conns.get(p) else { return };
        let (kinds, generation) = (c.type_kinds.clone(), c.generation);
        for (id, session) in ids {
            let Some(t) = self.tabs.get_mut(id) else { continue };
            for rs in t.row_results_mut() {
                for col in &mut rs.columns {
                    let Dated::Lookup(oid) = col.meta.dated else { continue };
                    match kinds.get(&(db.clone(), generation, id, session, oid)) {
                        Some(Some('b' | 'e')) => {
                            col.meta.dated = Dated::No;
                            col.meta.styled = false;
                            col.meta.zoned = false;
                        }
                        Some(Some(_)) => col.meta.dated = Dated::Unknown,
                        _ => {}
                    }
                }
            }
        }
    }
}
