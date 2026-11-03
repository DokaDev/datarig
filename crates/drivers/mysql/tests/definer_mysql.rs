//! A cancel of a statement that runs another account's code for every row, against a real
//! MySQL (`DATARIG_TEST_MYSQL_URL` and `DATARIG_TEST_MYSQL_ADMIN_URL`, as `integration_mysql`).
//!
//! Its own test binary, so that no other test runs beside it: each call of a `SQL SECURITY
//! DEFINER` function looks its definer up in the server's account cache, and while another
//! connection changes accounts or routines (`CREATE USER`, `GRANT`, `DROP FUNCTION`, …, as the
//! tests of `integration_mysql` do side by side) that lookup waits. A `KILL QUERY` landing in
//! that wait ends it as a failed lookup: the statement fails with ERROR 1449 ("the user
//! specified as a definer does not exist") instead of being interrupted.
//!
//! Every object made here is named `zz_it_…` and dropped again, also when the test fails.

use datarig_core::driver::{
    ConnectOptions, DbCommand, DbError, DbEvent, Driver, PagingMode, Session, SessionRole, TxRule,
};
use datarig_core::profile::ConnectionConfig;
use datarig_core::transport::{Dialer, DialerRef, TcpDialer};
use datarig_driver_mysql::MyDriver;
use mysql_async::prelude::Queryable;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

fn env_url(var: &str, test: &str, example: &str) -> Option<String> {
    match std::env::var(var) {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            if std::env::var("DATARIG_REQUIRE_MYSQL").is_ok_and(|v| v == "1") {
                panic!("{var} must be set when DATARIG_REQUIRE_MYSQL=1");
            }
            // Written to the real stderr handle so it is visible even with output capture.
            let _ = writeln!(std::io::stderr(), "SKIPPED {test}: set {var}={example}");
            None
        }
    }
}

/// The server's URL and an administrator's (root) URL.
fn urls(test: &str) -> Option<(String, String)> {
    let url = env_url("DATARIG_TEST_MYSQL_URL", test, "mysql://datarig:datarig@127.0.0.1:53306/shop")?;
    let admin = env_url("DATARIG_TEST_MYSQL_ADMIN_URL", test, "mysql://root:datarig-root@127.0.0.1:53306/")?;
    Some((url, admin))
}

/// The tag of test `test`'s sessions in this run (their `program_name` is
/// `datarig-<role>-<tag>`).
fn tag(test: &str) -> String {
    format!("it{}-{test}", std::process::id())
}

/// The dialer every session of the run goes through (`DATARIG_TEST_DIAL=tcp`), or none.
fn dialer() -> Option<DialerRef> {
    match std::env::var("DATARIG_TEST_DIAL").as_deref() {
        Ok("tcp") => Some(DialerRef(Arc::new(TcpDialer::default()) as Arc<dyn Dialer>)),
        Ok("") | Err(_) => None,
        Ok(other) => panic!("DATARIG_TEST_DIAL={other}: only `tcp` is known"),
    }
}

/// The profile of `url`, in the TLS mode `DATARIG_TEST_MYSQL_SSLMODE` names (unset: `DISABLED`).
fn profile(url: &str) -> ConnectionConfig {
    let sslmode = std::env::var("DATARIG_TEST_MYSQL_SSLMODE").unwrap_or_else(|_| "DISABLED".into());
    ConnectionConfig {
        name: "it".into(),
        driver: "mysql".into(),
        dsn: Some(url.to_string()),
        sslmode,
        ..Default::default()
    }
}

struct Conn {
    session: Session,
    rx: UnboundedReceiver<DbEvent>,
}

impl Conn {
    /// A query session of `url`'s profile for test `test`, once it connected. A run over TLS
    /// (`DATARIG_TEST_MYSQL_SSLMODE`) is over TLS indeed: the server offers it.
    async fn open(url: &str, test: &str) -> Conn {
        let (tx, rx) = unbounded_channel();
        let opts = ConnectOptions::new(500, SessionRole::Query, &tag(test)).dialer(dialer());
        let session = MyDriver.connect(&profile(url), SessionRole::Query, opts, tx);
        let mut c = Conn { session, rx };
        let tls = std::env::var("DATARIG_TEST_MYSQL_SSLMODE").is_ok_and(|m| !m.eq_ignore_ascii_case("DISABLED"));
        let mut encryption = None;
        loop {
            match c.next(15).await {
                DbEvent::Encryption(e) => encryption = Some(e),
                DbEvent::Connected => break,
                DbEvent::ConnectFailed { error, .. } => panic!("connect failed: {error:?}"),
                _ => {}
            }
        }
        if tls {
            let encrypted = matches!(encryption, Some(datarig_core::tls::Encryption::Tls { .. }));
            assert!(encrypted, "{encryption:?}");
        }
        c
    }

    /// Every event up to and with the terminal one of run `id` (its first page, `Done` or
    /// `Failed`).
    async fn answer(&mut self, id: u64) -> Vec<DbEvent> {
        let mut out = Vec::new();
        loop {
            let ev = self.next(60).await;
            let end = matches!(&ev, DbEvent::Page { id: i, columns: Some(_), .. }
                | DbEvent::Done { id: i, .. } | DbEvent::Failed { id: i, .. } if *i == id);
            if let DbEvent::ConnectFailed { error, .. } = &ev {
                panic!("connect failed: {error:?}");
            }
            out.push(ev);
            if end {
                return out;
            }
        }
    }

    /// Run `sql` as run `id`, not held: its events up to its answer.
    async fn run(&mut self, id: u64, sql: &str) -> Vec<DbEvent> {
        self.session.send(DbCommand::Execute {
            id,
            statements: vec![sql.to_string()],
            paging: PagingMode::NoHold,
            tx: TxRule::Auto,
        });
        self.answer(id).await
    }

    /// The first event, whatever it is.
    async fn next(&mut self, secs: u64) -> DbEvent {
        match tokio::time::timeout(Duration::from_secs(secs), self.rx.recv()).await {
            Ok(Some(ev)) => ev,
            Ok(None) => panic!("event channel closed"),
            Err(_) => panic!("timed out after {secs}s waiting for an event"),
        }
    }
}

/// A plain client connection to the server from the side.
async fn side(url: &str) -> mysql_async::Conn {
    let opts = mysql_async::Opts::from_url(url).expect("a mysql:// URL");
    let opts = mysql_async::OptsBuilder::from_opts(opts).prefer_socket(false);
    mysql_async::Conn::new(opts).await.expect("the side connection opens")
}

/// Runs `sql` on `url` when dropped, also when the test panics.
struct Cleanup {
    url: String,
    sql: String,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let (url, sql) = (self.url.clone(), self.sql.clone());
        // Dropped on a thread of its own: the test's runtime may be shutting down.
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let mut c = side(&url).await;
                let _ = c.query_drop(sql).await;
                let _ = c.disconnect().await;
            });
        })
        .join();
    }
}

/// Ends test `test`'s own sessions (`KILL` of the connections whose program name carries its
/// tag) when it ends, also when an assertion panics: a statement it left running goes with them.
struct EndSessions {
    admin: String,
    test: &'static str,
}

impl Drop for EndSessions {
    fn drop(&mut self) {
        let (admin, pattern) = (self.admin.clone(), format!("datarig-%-{}", tag(self.test).replace('_', "\\_")));
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                let mut c = side(&admin).await;
                let ids: Vec<u64> = c
                    .exec(
                        "SELECT PROCESSLIST_ID FROM performance_schema.session_connect_attrs \
                         WHERE ATTR_NAME = 'program_name' AND ATTR_VALUE LIKE ?",
                        (pattern,),
                    )
                    .await
                    .unwrap_or_default();
                for id in ids {
                    let _ = c.query_drop(format!("KILL {id}")).await;
                }
                let _ = c.disconnect().await;
            });
        })
        .join();
    }
}

/// A statement that runs as another account only for a moment at a time (a `SQL SECURITY
/// DEFINER` function called for every row): a kill that lands inside the function is refused,
/// one between two calls is not. The refusal is tried again (at once, then while the session
/// waits: the statement sends nothing), and the run ends cancelled with the session kept.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_refused_for_a_moment_is_tried_again() {
    let test = "a_cancel_refused_for_a_moment";
    let Some((url, admin)) = urls(test) else { return };
    let f = format!("zz_it_rowfn_{}", std::process::id());
    let _f = Cleanup { url: admin.clone(), sql: format!("DROP FUNCTION IF EXISTS shop.{f}") };
    // Dropped first: nothing of the test's keeps running past it.
    let _end = EndSessions { admin: admin.clone(), test };
    let mut a = side(&admin).await;
    a.query_drop(format!(
        "CREATE FUNCTION shop.{f}(x INT) RETURNS INT DETERMINISTIC SQL SECURITY DEFINER RETURN x + 1"
    ))
    .await
    .unwrap();
    a.disconnect().await.unwrap();
    let mut c = Conn::open(&url, test).await;
    c.session.send(DbCommand::Execute {
        id: 1,
        statements: vec![format!(
            "SELECT COUNT(*) FROM shop.events a CROSS JOIN shop.events b WHERE shop.{f}(a.id) > 0"
        )],
        paging: PagingMode::NoHold,
        tx: TxRule::Auto,
    });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    c.session.cancel();
    let evs = c.answer(1).await;
    assert!(matches!(evs.last(), Some(DbEvent::Failed { error: DbError::Cancelled, cancelled: true, .. })), "{evs:?}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let lost = c.rx.try_recv();
    assert!(!matches!(lost, Ok(DbEvent::Lost { .. })), "{lost:?}");
    let more = c.run(2, "SELECT 1").await;
    assert!(!more.iter().any(|e| matches!(e, DbEvent::Lost { .. })), "{more:?}");
    match more.last() {
        Some(DbEvent::Page { columns: Some(_), rows, .. }) => assert_eq!(rows, &vec![vec![Some("1".to_string())]]),
        other => panic!("{other:?}"),
    }
}
