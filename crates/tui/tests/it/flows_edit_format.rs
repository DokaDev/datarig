//! The edits a profile's server takes: what a session of it says (`DbEvent::Edit`) is the
//! profile's for the current connection attempt, as the plan format is.

use crate::common;

use common::*;
use datarig_core::driver::{DbEvent, EditFormat, MySqlEdit};
use datarig_i18n::Lang;
use datarig_tui::app::{AppEvent, EventTarget};

#[test]
fn a_sessions_word_on_edits_is_the_profiles_for_the_current_attempt() {
    let mut h = Harness::connected(Lang::En);
    let id = h.app.profiles[0].id;
    let name = h.app.profiles[0].name.clone();
    let edit = |h: &Harness| h.app.conns.get(id).and_then(|c| c.edit);
    assert_eq!(edit(&h), None, "the driver's holds until a session says");
    h.db(DbEvent::Edit(Some(EditFormat::Postgres)));
    assert_eq!(edit(&h), Some(Some(EditFormat::Postgres)), "a query session's word");
    h.meta_db(&name, DbEvent::Edit(None));
    assert_eq!(edit(&h), Some(None), "the metadata session's");
    let old = h.app.conns.get(id).unwrap().generation;
    h.app.conns.next_generation(id);
    assert_eq!(edit(&h), None, "forgotten at a new attempt");
    let mysql = EditFormat::MySql(MySqlEdit { nowait: true });
    h.app.on_app_event(AppEvent::Db { target: EventTarget::Meta(id), generation: old, ev: DbEvent::Edit(Some(mysql)) });
    assert_eq!(edit(&h), None, "an older attempt's word is dropped");
    h.meta_db(&name, DbEvent::Edit(Some(mysql)));
    assert_eq!(edit(&h), Some(Some(mysql)), "the current attempt's counts");
}
