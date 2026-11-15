//! What the query session tells the app of how its server writes dates, times and intervals
//! as text ([`TextStyle`]): its `DateStyle`, `IntervalStyle` and `TimeZone`, which the server
//! reports at startup and whenever they change, and the connection keeps in a
//! [`SharedParameters`] handle.
//!
//! Every event of the session goes through [`Events::send`], which reads the handle first. The
//! session reads the response to each request it sends up to its `ReadyForQuery`, and the
//! server reports a change before that, so once a response has been read the handle holds every
//! report sent in it:
//!
//! * before an event that ends a request (`Done`, `Finished`, `Failed`, the answer to a count,
//!   …), a change is said (`DbEvent::TextStyle` with the settings now): the next result is read
//!   in them;
//! * a page of rows (`Page`, `StepRows`) read while the settings changed, which may hold rows
//!   written in both, comes between `DbEvent::TextStyle(None)` and the settings after it;
//! * the events that come between a response and its page (`Released`, `TxOpen`, `Block`,
//!   `TxAborted`, `Info`, `ManualBegun`, `StatementCacheOff`) say nothing, so a change in that
//!   response stays the page's.

use datarig_core::driver::{DbEvent, TextStyle};
use std::sync::Mutex;
use tokio::sync::mpsc::UnboundedSender;
use tokio_postgres::SharedParameters;

/// The parameters of [`TextStyle`], in its fields' order.
const NAMES: [&str; 3] = ["DateStyle", "IntervalStyle", "TimeZone"];

/// Each of [`NAMES`] as last reported, with the number of reports of it.
type Snapshot = [Option<(String, u64)>; 3];

/// Reads one parameter as last reported (`SharedParameters::get`).
type Read = Box<dyn Fn(&str) -> Option<(String, u64)> + Send + Sync>;

/// The query session's events, sent to the app with what it needs of the settings first.
pub(crate) struct Events {
    tx: UnboundedSender<DbEvent>,
    watch: Option<Watch>,
}

struct Watch {
    read: Read,
    /// What the app was last told (`None`: nothing yet).
    told: Mutex<Option<Snapshot>>,
}

impl Watch {
    fn now(&self) -> Snapshot {
        NAMES.map(|n| (self.read)(n))
    }
}

/// The settings of `s`, when the server reported all three.
fn style_of(s: &Snapshot) -> Option<TextStyle> {
    let [Some((date_style, _)), Some((interval_style, _)), Some((time_zone, _))] = s else { return None };
    Some(TextStyle {
        date_style: date_style.clone(),
        interval_style: interval_style.clone(),
        time_zone: time_zone.clone(),
    })
}

impl Events {
    /// Events of a session whose connection keeps `params`.
    pub(crate) fn new(tx: UnboundedSender<DbEvent>, params: SharedParameters) -> Self {
        Self::reading(tx, Box::new(move |name| params.get(name)))
    }

    /// Events of a session that says nothing of its settings (tests that drive one by hand).
    #[cfg(test)]
    pub(crate) fn plain(tx: UnboundedSender<DbEvent>) -> Self {
        Self { tx, watch: None }
    }

    /// Events of a session whose settings `read` reads.
    fn reading(tx: UnboundedSender<DbEvent>, read: Read) -> Self {
        Self { tx, watch: Some(Watch { read, told: Mutex::new(None) }) }
    }

    /// Tell the app the settings now, when they changed since it was last told (or it never
    /// was).
    pub(crate) fn report(&self) {
        let Some(w) = &self.watch else { return };
        let now = w.now();
        let mut told = w.told.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if told.as_ref() != Some(&now) {
            let _ = self.tx.send(DbEvent::TextStyle(style_of(&now)));
            *told = Some(now);
        }
    }

    /// Send `ev`, with what the app must know of the settings first (see the module's docs);
    /// `false` when the app is gone.
    pub(crate) fn send(&self, ev: DbEvent) -> bool {
        let Some(w) = &self.watch else { return self.tx.send(ev).is_ok() };
        match ev {
            DbEvent::Page { .. } | DbEvent::StepRows { .. } => {
                let moved = {
                    let told = w.told.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    told.as_ref() != Some(&w.now())
                };
                if !moved {
                    return self.tx.send(ev).is_ok();
                }
                let _ = self.tx.send(DbEvent::TextStyle(None));
                let sent = self.tx.send(ev).is_ok();
                self.report();
                sent
            }
            DbEvent::Released { .. }
            | DbEvent::TxOpen(_)
            | DbEvent::Block(_)
            | DbEvent::TxAborted(_)
            | DbEvent::Info { .. }
            | DbEvent::ManualBegun { .. }
            | DbEvent::StatementCacheOff
            | DbEvent::TextStyle(_) => self.tx.send(ev).is_ok(),
            _ => {
                self.report();
                self.tx.send(ev).is_ok()
            }
        }
    }
}

impl std::ops::Deref for Events {
    type Target = UnboundedSender<DbEvent>;

    /// The channel itself, for code that answers before any request runs (a run cancelled
    /// before it started).
    fn deref(&self) -> &Self::Target {
        &self.tx
    }
}

#[cfg(test)]
mod tests;
