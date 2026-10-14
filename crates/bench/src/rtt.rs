//! Round trips and wall time per statement through the latency proxy ([`crate::proxy`]): the
//! driver's query session runs each statement, and the proxy counts the flights until the
//! result arrives and until the session is quiet again.

use crate::proxy;
use crate::stats::{Summary, ms};
use datarig_core::driver::PagingMode;
use datarig_core::driver::ddl::DdlObject;
use datarig_core::driver::{ConnectOptions, DbCommand, DbEvent, Driver, Session, SessionRole};
use datarig_core::profile::ConnectionConfig;
use datarig_driver_postgres::PgDriver;
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

pub struct WiredSession {
    pub(crate) session: Session,
    rx: UnboundedReceiver<DbEvent>,
    id: u64,
    pub counts: std::sync::Arc<proxy::Counts>,
    one_way: Duration,
    /// What a result with more rows keeps: `NoHold` as the app by default, `Hold` for the
    /// scenarios of `paging = "hold"`.
    pub paging: PagingMode,
}

/// What one statement cost.
#[derive(Clone, Copy, Debug)]
pub struct Cost {
    /// Flights until the result (the first page, the outcome or the failure) arrived.
    pub to_result: u64,
    /// Flights until the session was quiet again (a trailing `COMMIT` included).
    pub total: u64,
    /// Wall time until the result.
    pub ms: f64,
}

impl WiredSession {
    /// A query session behind a proxy that delays each direction by `one_way`, of a profile
    /// whose policy is read-only (`read_only`: every transaction is `READ ONLY`) or not.
    pub async fn open(url: &str, one_way: Duration, read_only: bool) -> Result<WiredSession, String> {
        WiredSession::open_as(url, one_way, read_only, SessionRole::Query).await
    }

    /// A session of `role` behind the proxy (the metadata session for the explorer's reads).
    pub async fn open_as(
        url: &str,
        one_way: Duration,
        read_only: bool,
        role: SessionRole,
    ) -> Result<WiredSession, String> {
        WiredSession::open_with(&PgDriver, url, one_way, read_only, role).await
    }

    /// A session of `role` of `driver` (the server of `url`) behind the proxy.
    pub async fn open_with(
        driver: &dyn Driver,
        url: &str,
        one_way: Duration,
        read_only: bool,
        role: SessionRole,
    ) -> Result<WiredSession, String> {
        use datarig_core::profile::dsn::Scheme;
        let d = datarig_core::profile::dsn::parse(url).map_err(|e| format!("{e:?}"))?;
        let port = d.port.unwrap_or(d.scheme.default_port());
        let p = match d.scheme {
            Scheme::Postgres => proxy::start(d.host.clone(), port, one_way).await,
            Scheme::MySql => proxy::start_raw(d.host.clone(), port, one_way).await,
        }
        .map_err(|e| e.to_string())?;
        let cfg = ConnectionConfig {
            name: "bench".into(),
            driver: match d.scheme {
                Scheme::Postgres => "postgres".into(),
                Scheme::MySql => "mysql".into(),
            },
            host: "127.0.0.1".into(),
            port: p.port,
            user: d.user.clone(),
            password: d.password.clone().unwrap_or_default(),
            database: d.database.clone(),
            sslmode: "disable".into(),
            ..ConnectionConfig::default()
        };
        let (tx, rx) = unbounded_channel();
        let opts = ConnectOptions::new(500, role, &format!("bench{}", std::process::id())).read_only(read_only);
        let session = driver.connect(&cfg, role, opts, tx);
        let mut s = WiredSession { session, rx, id: 0, counts: p.counts, one_way, paging: PagingMode::NoHold };
        match s.next(Duration::from_secs(10)).await? {
            DbEvent::Connected => Ok(s),
            ev => Err(format!("connect: {ev:?}")),
        }
    }

    /// Round trips so far (opening the session's connection, before any statement).
    pub fn flights(&self) -> u64 {
        self.counts.flights()
    }

    /// A query session already opened whose connection crosses the proxy `counts` counts
    /// (through an SSH tunnel).
    pub async fn wired(
        session: Session,
        rx: UnboundedReceiver<DbEvent>,
        counts: std::sync::Arc<proxy::Counts>,
        one_way: Duration,
    ) -> Result<WiredSession, String> {
        let mut s = WiredSession { session, rx, id: 0, counts, one_way, paging: PagingMode::NoHold };
        match s.next(Duration::from_secs(20)).await? {
            DbEvent::Connected => Ok(s),
            ev => Err(format!("connect: {ev:?}")),
        }
    }

    async fn next(&mut self, within: Duration) -> Result<DbEvent, String> {
        match tokio::time::timeout(within, self.rx.recv()).await {
            Ok(Some(ev)) => Ok(ev),
            Ok(None) => Err("the session ended".into()),
            Err(_) => Err(format!("no event within {within:?}")),
        }
    }

    /// Wait until nothing has crossed the proxy for a while: every trailing request of the last
    /// statement has been answered.
    pub async fn quiet(&self) {
        let pause = self.one_way * 4 + Duration::from_millis(30);
        let mut last = (self.counts.flights(), self.counts.requests());
        loop {
            tokio::time::sleep(pause).await;
            let now = (self.counts.flights(), self.counts.requests());
            if now == last {
                return;
            }
            last = now;
        }
    }

    /// Run `sql` and measure it. A result with more rows held open is closed afterwards, outside
    /// the measurement.
    pub async fn cost(&mut self, sql: &str) -> Result<(Cost, DbEvent), String> {
        self.quiet().await;
        self.id += 1;
        let id = self.id;
        let before = self.counts.flights();
        let t0 = Instant::now();
        self.session.send(DbCommand::Execute { id, statements: vec![sql.to_string()], paging: self.paging });
        let ev = loop {
            match self.next(Duration::from_secs(60)).await? {
                ev @ (DbEvent::Page { id: i, .. } | DbEvent::Done { id: i, .. } | DbEvent::Failed { id: i, .. })
                    if i == id =>
                {
                    break ev;
                }
                DbEvent::Lost { error } => return Err(format!("lost: {error:?}")),
                _ => {}
            }
        };
        let elapsed = ms(t0.elapsed());
        let to_result = self.counts.delivered() - before;
        self.quiet().await;
        let total = self.counts.flights() - before;
        if let DbEvent::Page { more: true, .. } = ev {
            self.session.send(DbCommand::ClosePortal { id });
        }
        // Drain the events of the statement (transaction state) before the next one.
        while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(1), self.rx.recv()).await {}
        Ok((Cost { to_result, total, ms: elapsed }, ev))
    }
}

impl WiredSession {
    /// Run `sql` (a result with more than a page) holding its portal, then fetch `pages` more
    /// pages and measure each; the portal is closed afterwards.
    pub async fn next_pages(&mut self, sql: &str, pages: usize) -> Result<Vec<Cost>, String> {
        self.quiet().await;
        self.id += 1;
        let id = self.id;
        self.session.send(DbCommand::Execute { id, statements: vec![sql.to_string()], paging: PagingMode::Hold });
        self.page(id).await?;
        let mut costs = Vec::new();
        for _ in 0..pages {
            self.quiet().await;
            let before = self.counts.flights();
            let t0 = Instant::now();
            self.session.send(DbCommand::FetchMore { id });
            let more = self.page(id).await?;
            let elapsed = ms(t0.elapsed());
            let to_result = self.counts.delivered() - before;
            self.quiet().await;
            costs.push(Cost { to_result, total: self.counts.flights() - before, ms: elapsed });
            if !more {
                break;
            }
        }
        self.session.send(DbCommand::ClosePortal { id });
        self.quiet().await;
        while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(1), self.rx.recv()).await {}
        Ok(costs)
    }

    /// Send `cmd` and measure it until an event `done` recognises (a count, a
    /// resumed result). A result left with more rows is closed afterwards, outside the
    /// measurement.
    pub async fn measure(
        &mut self,
        cmd: DbCommand,
        done: impl Fn(&DbEvent) -> bool,
    ) -> Result<(Cost, DbEvent), String> {
        self.quiet().await;
        let before = self.counts.flights();
        let t0 = Instant::now();
        self.session.send(cmd);
        let ev = loop {
            match self.next(Duration::from_secs(60)).await? {
                ev if done(&ev) => break ev,
                DbEvent::Lost { error } => return Err(format!("lost: {error:?}")),
                _ => {}
            }
        };
        let elapsed = ms(t0.elapsed());
        let to_result = self.counts.delivered() - before;
        self.quiet().await;
        let total = self.counts.flights() - before;
        while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(1), self.rx.recv()).await {}
        Ok((Cost { to_result, total, ms: elapsed }, ev))
    }

    /// A new result id.
    pub fn next_id(&mut self) -> u64 {
        self.id += 1;
        self.id
    }

    /// Wait for a page of result `id`; whether more follow.
    pub async fn page(&mut self, id: u64) -> Result<bool, String> {
        loop {
            match self.next(Duration::from_secs(60)).await? {
                DbEvent::Page { id: i, more, .. } if i == id => return Ok(more),
                DbEvent::Failed { id: i, error, .. } if i == id => return Err(format!("{error:?}")),
                DbEvent::Lost { error } => return Err(format!("lost: {error:?}")),
                _ => {}
            }
        }
    }
}

pub fn report(name: &str, costs: &[Cost]) -> Value {
    let to_result = Summary::of(&costs.iter().map(|c| c.to_result as f64).collect::<Vec<_>>());
    let total = Summary::of(&costs.iter().map(|c| c.total as f64).collect::<Vec<_>>());
    let wall = Summary::of(&costs.iter().map(|c| c.ms).collect::<Vec<_>>());
    println!(
        "  {name:<22} round trips to result {:.0} (min {:.0}, max {:.0}), in total {:.0} (max {:.0}); ms {}",
        to_result.median,
        to_result.min,
        to_result.max,
        total.median,
        total.max,
        wall.line("")
    );
    json!({
        "name": name,
        "rtt_to_result": to_result.json(),
        "rtt_total": total.json(),
        "ms_to_result": wall.json(),
    })
}

/// One scenario: `runs` statements from `sql(i)`, after `warm` unmeasured ones.
pub async fn scenario(
    s: &mut WiredSession,
    name: &str,
    runs: usize,
    warm: usize,
    sql: impl Fn(usize) -> String,
) -> Result<Value, String> {
    for i in 0..warm {
        s.cost(&sql(i)).await?;
    }
    let mut costs = Vec::new();
    for i in 0..runs {
        let (c, ev) = s.cost(&sql(warm + i)).await?;
        if let DbEvent::Failed { error, .. } = ev {
            return Err(format!("{name}: {error:?}"));
        }
        costs.push(c);
    }
    Ok(report(name, &costs))
}

pub async fn run(url: &str, one_way: Duration, runs: usize) -> Result<Value, String> {
    let mut s = WiredSession::open(url, one_way, false).await?;
    println!("rtt: {runs} runs per statement, {} ms one way", one_way.as_millis());
    let mut out = Vec::new();
    // A temporary table: it goes away with the session, whatever happens.
    s.cost("CREATE TEMP TABLE zz_bench_rtt (x int)").await?;
    let small = "SELECT id, event_type, created_at FROM analytics.events WHERE id <= 100";
    // The default (`paging = "no_hold"`): the first page's `COMMIT` goes out with it, so the
    // session is quiet once the page is there, a page of a large result too.
    out.push(scenario(&mut s, "select_small_cold", runs, 0, |i| format!("{small} AND {i} >= 0")).await?);
    out.push(scenario(&mut s, "select_small_warm", runs, 1, |_| small.to_string()).await?);
    let big = "SELECT * FROM analytics.events";
    out.push(scenario(&mut s, "select_page_of_4m_cold", runs, 0, |i| format!("{big} WHERE {i} >= 0")).await?);
    out.push(scenario(&mut s, "select_page_of_4m_warm", runs, 1, |_| big.to_string()).await?);
    // `paging = "hold"`: a result that fits in a page commits after it (one round trip more in
    // total); a large one keeps its portal, and its next page is one round trip.
    s.paging = PagingMode::Hold;
    out.push(scenario(&mut s, "hold_select_small_warm", runs, 1, |_| small.to_string()).await?);
    out.push(scenario(&mut s, "hold_select_page_of_4m_warm", runs, 1, |_| big.to_string()).await?);
    let pages = s.next_pages(big, runs).await?;
    out.push(report("next_page", &pages));
    s.paging = PagingMode::NoHold;
    // A count the user asks for, while a result pages (under a savepoint in
    // the portal's transaction, `paging = "hold"`) and with no portal open; and a result resumed
    // past a closed portal or one never held (its statement prepared already): the next page
    // of the default, whose `COMMIT` goes out with it.
    let count = datarig_core::sql::risk::repeat::count_query(small).map_err(|e| format!("{e:?}"))?;
    let counted = |e: &DbEvent| matches!(e, DbEvent::Counted { .. });
    let paging = s.next_id();
    s.session.send(DbCommand::Execute { id: paging, statements: vec![big.to_string()], paging: PagingMode::Hold });
    s.page(paging).await?;
    let mut costs = Vec::new();
    for _ in 0..runs {
        let (c, ev) = s.measure(DbCommand::Count { id: paging, sql: count.clone() }, counted).await?;
        if !matches!(ev, DbEvent::Counted { result: Ok(100), .. }) {
            return Err(format!("count_while_paging: {ev:?}"));
        }
        costs.push(c);
    }
    s.session.send(DbCommand::ClosePortal { id: paging });
    out.push(report("count_while_paging", &costs));
    let mut costs = Vec::new();
    for _ in 0..runs {
        let (c, ev) = s.measure(DbCommand::Count { id: paging, sql: count.clone() }, counted).await?;
        if !matches!(ev, DbEvent::Counted { result: Ok(100), .. }) {
            return Err(format!("count_idle: {ev:?}"));
        }
        costs.push(c);
    }
    out.push(report("count_idle", &costs));
    let mut costs = Vec::new();
    for i in 0..=runs {
        let id = s.next_id();
        let resume = DbCommand::Resume { id, sql: big.to_string(), skip: 500, paging: PagingMode::NoHold };
        let (c, ev) = s
            .measure(resume, |e| matches!(e, DbEvent::Page { id: p, .. } | DbEvent::Failed { id: p, .. } if *p == id))
            .await?;
        if !matches!(ev, DbEvent::Page { more: true, .. }) {
            return Err(format!("resume_warm: {ev:?}"));
        }
        s.session.send(DbCommand::ClosePortal { id });
        // The first one prepares the statement.
        if i > 0 {
            costs.push(c);
        }
    }
    out.push(report("resume_warm", &costs));
    out.push(scenario(&mut s, "insert", runs, 0, |i| format!("INSERT INTO zz_bench_rtt VALUES ({i})")).await?);
    s.cost("BEGIN").await?;
    // Inside the user's block a resumed result runs under a savepoint, set with the question.
    let mut costs = Vec::new();
    for i in 0..=runs {
        let id = s.next_id();
        let resume = DbCommand::Resume { id, sql: big.to_string(), skip: 500, paging: PagingMode::Hold };
        let (c, ev) = s
            .measure(resume, |e| matches!(e, DbEvent::Page { id: p, .. } | DbEvent::Failed { id: p, .. } if *p == id))
            .await?;
        if !matches!(ev, DbEvent::Page { more: true, .. }) {
            return Err(format!("resume_in_block: {ev:?}"));
        }
        // Its portal ends with the next command (the savepoint is released then).
        s.cost("SELECT 1").await?;
        // Inside the block every statement is prepared again: the first one alike.
        if i > 0 {
            costs.push(c);
        }
    }
    out.push(report("resume_in_block", &costs));
    out.push(scenario(&mut s, "select_small_in_block", runs, 1, |_| small.to_string()).await?);
    out.push(scenario(&mut s, "insert_in_block", runs, 0, |i| format!("INSERT INTO zz_bench_rtt VALUES ({i})")).await?);
    s.cost("ROLLBACK").await?;
    s.cost("DROP TABLE zz_bench_rtt").await?;
    // A read-only policy: `BEGIN READ ONLY` goes out with the first page, and
    // `SET TRANSACTION READ ONLY` with the first statement of the user's block.
    let mut r = WiredSession::open(url, one_way, true).await?;
    out.push(scenario(&mut r, "ro_select_small_cold", runs, 0, |i| format!("{small} AND {i} >= 0")).await?);
    out.push(scenario(&mut r, "ro_select_small_warm", runs, 1, |_| small.to_string()).await?);
    out.push(scenario(&mut r, "ro_select_page_of_4m_warm", runs, 1, |_| big.to_string()).await?);
    out.push(scenario(&mut r, "ro_set", runs, 0, |i| format!("SET work_mem = '{}MB'", 4 + i)).await?);
    let mut first = Vec::new();
    for _ in 0..runs {
        r.cost("BEGIN").await?;
        let (c, ev) = r.cost(small).await?;
        if let DbEvent::Failed { error, .. } = ev {
            return Err(format!("ro_first_in_block: {error:?}"));
        }
        first.push(c);
        r.cost("ROLLBACK").await?;
    }
    out.push(report("ro_first_in_block", &first));
    // The explorer opening a table: its structure, read on the metadata session (after the
    // reads it makes when it connects).
    let mut m = WiredSession::open_as(url, one_way, false, SessionRole::Meta).await?;
    let mut costs = Vec::new();
    for _ in 0..runs {
        let load = DbCommand::LoadStructure { schema: "shop".into(), table: "orders".into() };
        let (c, ev) = m.measure(load, |e| matches!(e, DbEvent::Structure { .. })).await?;
        if !matches!(ev, DbEvent::Structure { result: Ok(_), .. }) {
            return Err(format!("table_structure: {ev:?}"));
        }
        costs.push(c);
    }
    out.push(report("table_structure", &costs));
    // Show DDL of a table: its structure and what its `CREATE` needs, one catalog read.
    let mut costs = Vec::new();
    for i in 0..runs {
        let object = DdlObject::Relation { schema: "shop".into(), name: "orders".into() };
        let load = DbCommand::LoadDdl { id: i as u64, object };
        let (c, ev) = m.measure(load, |e| matches!(e, DbEvent::Ddl { .. })).await?;
        if !matches!(ev, DbEvent::Ddl { result: Ok(_), .. }) {
            return Err(format!("table_ddl: {ev:?}"));
        }
        costs.push(c);
    }
    out.push(report("table_ddl", &costs));
    // The explorer opening a schema: its objects with the estimates of their rows and size.
    let mut costs = Vec::new();
    for _ in 0..runs {
        let load = DbCommand::LoadObjects { schema: "shop".into() };
        let (c, ev) = m.measure(load, |e| matches!(e, DbEvent::Objects { .. })).await?;
        if !matches!(&ev, DbEvent::Objects { result: Ok(o), .. } if o.stats.contains_key("orders")) {
            return Err(format!("schema_objects: {ev:?}"));
        }
        costs.push(c);
    }
    out.push(report("schema_objects", &costs));
    Ok(json!({ "one_way_ms": one_way.as_millis() as u64, "scenarios": out }))
}
