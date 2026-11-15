use super::*;
use datarig_core::driver::{DbError, Outcome};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::unbounded_channel;

/// The parameters a server reported, set by hand.
#[derive(Clone, Default)]
struct Reported(Arc<std::sync::Mutex<HashMap<String, (String, u64)>>>);

impl Reported {
    fn set(&self, name: &str, value: &str) {
        let mut m = self.0.lock().unwrap();
        let n = m.get(name).map_or(0, |(_, n)| n + 1);
        m.insert(name.to_string(), (value.to_string(), n));
    }
}

fn session(r: &Reported) -> (Events, tokio::sync::mpsc::UnboundedReceiver<DbEvent>) {
    session_of(r, false)
}

/// [`session`], `blind` as a session whose start-up statements go in each transaction.
fn session_of(r: &Reported, blind: bool) -> (Events, tokio::sync::mpsc::UnboundedReceiver<DbEvent>) {
    let (tx, rx) = unbounded_channel();
    let r = r.clone();
    (Events::reading(tx, Box::new(move |name| r.0.lock().unwrap().get(name).cloned()), blind), rx)
}

fn iso() -> Reported {
    let r = Reported::default();
    r.set("DateStyle", "ISO, MDY");
    r.set("IntervalStyle", "postgres");
    r.set("TimeZone", "UTC");
    r
}

fn style(date: &str) -> Option<TextStyle> {
    Some(TextStyle { date_style: date.into(), interval_style: "postgres".into(), time_zone: "UTC".into() })
}

fn page() -> DbEvent {
    DbEvent::Page { id: 1, columns: Some(Vec::new()), rows: Vec::new(), more: false, elapsed: Duration::ZERO }
}

fn done() -> DbEvent {
    DbEvent::Done { id: 1, outcome: Outcome::Command("SET".into()), elapsed: Duration::ZERO }
}

/// The events sent, as `TextStyle(date style)`, `page`, `done` or `other`.
fn names(rx: &mut tokio::sync::mpsc::UnboundedReceiver<DbEvent>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(match ev {
            DbEvent::TextStyle(s) => format!("style {}", s.map_or("unknown".into(), |s| s.date_style)),
            DbEvent::Page { .. } => "page".into(),
            DbEvent::Done { .. } => "done".into(),
            _ => "other".into(),
        });
    }
    out
}

#[test]
fn the_settings_at_connect_are_said_once_and_a_change_before_the_end_of_its_request() {
    let r = iso();
    let (events, mut rx) = session(&r);
    events.report();
    events.report();
    assert_eq!(names(&mut rx), ["style ISO, MDY"]);
    // `SET DateStyle`: said before its answer, so the next result is read in it.
    r.set("DateStyle", "SQL, DMY");
    events.send(done());
    events.send(page());
    assert_eq!(names(&mut rx), ["style SQL, DMY", "done", "page"]);
}

#[test]
fn a_page_read_while_the_settings_changed_is_read_in_settings_not_known() {
    let r = iso();
    let (events, mut rx) = session(&r);
    events.report();
    names(&mut rx);
    // `SELECT set_config('DateStyle', …)`: reported in the response that read the page; what
    // comes between the response and the page says nothing.
    r.set("DateStyle", "SQL, DMY");
    events.send(DbEvent::TxOpen(true));
    events.send(page());
    assert_eq!(names(&mut rx), ["other", "style unknown", "page", "style SQL, DMY"]);
    events.send(done());
    assert_eq!(names(&mut rx), ["done"]);
}

#[test]
fn a_missing_setting_is_not_known() {
    let r = Reported::default();
    r.set("DateStyle", "ISO, MDY");
    let (events, mut rx) = session(&r);
    events.report();
    let fail = DbEvent::Failed { id: 1, error: DbError::Closed, cancelled: false };
    events.send(fail);
    assert_eq!(names(&mut rx), ["style unknown", "other"]);
    assert_eq!(style_of(&[Some(("ISO, MDY".into(), 0)), Some(("postgres".into(), 0)), None]), None);
    let all = [Some(("ISO, MDY".into(), 0)), Some(("postgres".into(), 2)), Some(("UTC".into(), 0))];
    assert_eq!(style_of(&all), style("ISO, MDY"));
}

/// Start-up statements sent first in each transaction may set the settings in the very request
/// that reads a page and commits it, which sets them back unreported: such a session says they
/// are not known, as it connects and after any report.
#[test]
fn a_session_with_start_up_statements_in_each_transaction_never_knows_its_settings() {
    let r = iso();
    let (events, mut rx) = session_of(&r, true);
    events.report();
    events.send(page());
    r.set("TimeZone", "Asia/Seoul");
    events.send(done());
    assert_eq!(names(&mut rx), ["style unknown", "page", "style unknown", "done"]);
}

#[test]
fn a_session_without_a_watch_sends_its_events_as_they_are() {
    let (tx, mut rx) = unbounded_channel();
    let events = Events::plain(tx);
    events.report();
    events.send(page());
    assert_eq!(names(&mut rx), ["page"]);
}
