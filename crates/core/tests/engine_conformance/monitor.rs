//! How each engine's monitor sends an action on every provider (`monitor::action::route`): the
//! provider's procedures that stop a session when the standard statement is refused, the codes
//! that call for them, and the grants and service accounts a refusal names. MariaDB has no
//! monitor: its probe is refused at any version, so it has no route.

use crate::Case;
use crate::Engine::{self, MariaDb, MySql, Pg};
use datarig_core::monitor::action::route;
use datarig_core::monitor::decode::{self, Answer};
use datarig_core::monitor::format::MYSQL_OLDEST;
use datarig_core::monitor::{Flavour, MonitorFormat, MySqlMonitor, PgMonitor, Readable, Route};

/// A PostgreSQL server of `flavour`, as its probe reads it.
fn pg(flavour: Flavour) -> MonitorFormat {
    MonitorFormat::Postgres(PgMonitor {
        version: 170011,
        superuser: false,
        read_all_stats: true,
        signal_backend: false,
        flavour,
        route: Route::Direct,
    })
}

/// A MySQL server of `flavour`, as its probe reads it.
fn mysql(flavour: Flavour) -> MonitorFormat {
    MonitorFormat::MySql(MySqlMonitor {
        version: 80411,
        performance_schema: true,
        user: "zz_admin".into(),
        process: Readable::Yes,
        innodb_trx: Readable::Yes,
        metadata_locks: Readable::Yes,
        statements: Readable::Yes,
        threads: Readable::Yes,
        unconfirmed: false,
        flavour,
    })
}

/// A MariaDB server's probe, which the monitor refuses at any version.
fn mariadb() -> String {
    let none = Answer::Rows(Vec::new());
    let first = Answer::Rows(vec![vec![Some("11.4.13-MariaDB-ubu2404".into()), Some("1".into()), Some("u@%".into())]]);
    let answers = [first, none.clone(), none.clone(), none.clone(), none.clone(), none.clone(), none];
    match decode::mysql::probe(&answers) {
        Ok(m) => format!("probed {m:?}"),
        Err(e) => format!("no route: {:?}", e.into_db(MYSQL_OLDEST)),
    }
}

/// The route of a server of the engine of `flavour`.
pub fn route_of(e: Engine, flavour: &Flavour) -> String {
    let format = match e {
        Pg => pg(*flavour),
        MySql => mysql(*flavour),
        MariaDb => return mariadb(),
        other => panic!("{other:?} is not a column of the monitor routes table"),
    };
    let r = route(&format);
    let fallback = match r.fallback {
        Some(f) => format!("{} / {} on {:?}", f.cancel, f.terminate, f.on),
        None => "none".to_string(),
    };
    format!(
        "{:?} | fallback {fallback} | grants {:?} | service {:?}",
        r.flavour, r.guidance.grants, r.guidance.service_users
    )
}

const MARIADB: &str = "no route: MonitorUnsupported(\"MariaDB 11.4.13\")";

pub fn route_cases() -> Vec<Case<Flavour>> {
    vec![
        Case {
            name: "standard",
            input: Flavour::Standard,
            expect: &[
                (Pg, "Standard | fallback none | grants [\"pg_signal_backend\"] | service []"),
                (MySql, "Standard | fallback none | grants [\"CONNECTION_ADMIN\"] | service []"),
                (MariaDb, MARIADB),
            ],
        },
        Case {
            name: "Amazon RDS",
            input: Flavour::AmazonRds,
            expect: &[
                (
                    Pg,
                    "AmazonRds | fallback none | grants [\"rds_superuser\", \"pg_signal_backend\"] | service [\"rdsadmin\"]",
                ),
                (
                    MySql,
                    "AmazonRds | fallback mysql.rds_kill_query / mysql.rds_kill on [1095] | grants [\"CONNECTION_ADMIN\"] | service [\"rdsadmin\"]",
                ),
                (MariaDb, MARIADB),
            ],
        },
        Case {
            name: "Amazon Aurora",
            input: Flavour::AmazonAurora,
            expect: &[
                (
                    Pg,
                    "AmazonAurora | fallback none | grants [\"rds_superuser\", \"pg_signal_backend\"] | service [\"rdsadmin\"]",
                ),
                (
                    MySql,
                    "AmazonAurora | fallback mysql.rds_kill_query / mysql.rds_kill on [1095] | grants [\"CONNECTION_ADMIN\"] | service [\"rdsadmin\"]",
                ),
                (MariaDb, MARIADB),
            ],
        },
        Case {
            name: "Google Cloud SQL",
            input: Flavour::GoogleCloudSql,
            expect: &[
                (
                    Pg,
                    "GoogleCloudSql | fallback none | grants [\"cloudsqlsuperuser\", \"pg_signal_backend\"] | service [\"cloudsqladmin\"]",
                ),
                // Not listed for MySQL: the standard route.
                (MySql, "Standard | fallback none | grants [\"CONNECTION_ADMIN\"] | service []"),
                (MariaDb, MARIADB),
            ],
        },
        Case {
            name: "Google AlloyDB",
            input: Flavour::GoogleAlloyDb,
            expect: &[
                (
                    Pg,
                    "GoogleAlloyDb | fallback none | grants [\"alloydbsuperuser\", \"pg_signal_backend\"] | service []",
                ),
                (MySql, "Standard | fallback none | grants [\"CONNECTION_ADMIN\"] | service []"),
                (MariaDb, MARIADB),
            ],
        },
        Case {
            name: "Azure Database",
            input: Flavour::AzureDatabase,
            expect: &[
                (
                    Pg,
                    "AzureDatabase | fallback none | grants [\"azure_pg_admin\", \"pg_signal_backend\"] | service [\"azuresu\"]",
                ),
                (
                    MySql,
                    "AzureDatabase | fallback mysql.az_kill_query / mysql.az_kill on [1095] | grants [\"CONNECTION_ADMIN\"] | service [\"azure_superuser\"]",
                ),
                (MariaDb, MARIADB),
            ],
        },
    ]
}
