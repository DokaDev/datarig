//! How each engine's monitor sends an action on every provider (`monitor::action::route`): the
//! provider's procedures that stop a session when the standard statement is refused, the codes
//! that call for them, and the grants and service accounts a refusal names; and what the
//! monitor's views call things on each engine (`MonitorRules::terms`). MariaDB has no monitor
//! rules (its probe is refused at any version), so it has no route and no terms.

use crate::Case;
use crate::Engine::{self, MariaDb, MySql, Pg};
use datarig_core::engine::{EngineId, Fact, MonitorTerms, ReplicationTerms};
use datarig_core::monitor::action::route;
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

/// MariaDB's monitor rules: none, and why.
fn mariadb() -> String {
    match &EngineId::MariaDb.spec().monitor {
        Fact::Known(rules) => format!("Known({:?})", rules.routes().standard),
        Fact::No(why) => format!("No({why:?})"),
        Fact::Unknown(why) => format!("Unknown({why:?})"),
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
    let r = match route(&format) {
        Ok(r) => r,
        Err(why) => return format!("Err({why:?})"),
    };
    let fallback = match r.fallback {
        Some(f) => format!("{} / {} on {:?}", f.cancel, f.terminate, f.on),
        None => "none".to_string(),
    };
    format!(
        "{:?} | fallback {fallback} | grants {:?} | service {:?}",
        r.flavour, r.guidance.grants, r.guidance.service_users
    )
}

const MARIADB: &str = "No(NotBuilt)";

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

/// Every field of `t`, by name, as its value reads.
pub fn term_fields(t: &MonitorTerms) -> Vec<(&'static str, String)> {
    let MonitorTerms {
        session_id,
        role,
        version_line,
        waits_asked,
        charts,
        start_in_seconds,
        end_says_rollback,
        replication,
    } = *t;
    let ReplicationTerms {
        channels,
        receiver,
        last_message,
        receiver_line_always,
        applier,
        applied_on_applier_line,
        zero_lag_note,
        lag_bytes,
        replica,
    } = replication;
    vec![
        ("session id", format!("{session_id:?}")),
        ("role", format!("{role:?}")),
        ("version line", format!("{version_line:?}")),
        ("waits asked", format!("{waits_asked:?}")),
        ("charts", format!("{charts:?}")),
        ("start in seconds", format!("{start_in_seconds:?}")),
        ("end says rollback", format!("{end_says_rollback:?}")),
        ("replication channels", format!("{channels:?}")),
        ("replication receiver", format!("{receiver:?}")),
        ("replication last message", format!("{last_message:?}")),
        ("replication receiver line always", format!("{receiver_line_always:?}")),
        ("replication applier", format!("{applier:?}")),
        ("replication applied on applier line", format!("{applied_on_applier_line:?}")),
        ("replication zero lag note", format!("{zero_lag_note:?}")),
        ("replication lag bytes", format!("{lag_bytes:?}")),
        ("replication replica", format!("{replica:?}")),
    ]
}

/// Field `field` of the engine's monitor terms, or why the engine has none.
pub fn term_of(e: Engine, field: &&'static str) -> String {
    let id = match e {
        Pg => EngineId::Postgres,
        MySql => EngineId::MySql,
        MariaDb => EngineId::MariaDb,
        other => panic!("{other:?} is not a column of the monitor terms table"),
    };
    let rules = match &id.spec().monitor {
        Fact::Known(rules) => *rules,
        Fact::No(why) => return format!("No({why:?})"),
        Fact::Unknown(why) => return format!("Unknown({why:?})"),
    };
    let fields = term_fields(rules.terms());
    match fields.into_iter().find(|(name, _)| name == field) {
        Some((_, value)) => value,
        None => panic!("{field} is not a field of the monitor terms"),
    }
}

/// Each field of the monitor terms: its value on every engine.
pub fn term_cases() -> Vec<Case<&'static str>> {
    vec![
        Case { name: "session id", input: "session id", expect: &[(Pg, "Pid"), (MySql, "Id"), (MariaDb, MARIADB)] },
        Case { name: "role", input: "role", expect: &[(Pg, "Recovery"), (MySql, "ReadOnlyFlag"), (MariaDb, MARIADB)] },
        Case {
            name: "version line",
            input: "version line",
            expect: &[(Pg, "UpToBuild"), (MySql, "NumberThenProduct"), (MariaDb, MARIADB)],
        },
        Case {
            name: "waits asked",
            input: "waits asked",
            expect: &[(Pg, "Longest"), (MySql, "All"), (MariaDb, MARIADB)],
        },
        Case {
            name: "charts",
            input: "charts",
            expect: &[(Pg, "RowsAndStates"), (MySql, "StatementsAndCommands"), (MariaDb, MARIADB)],
        },
        Case {
            name: "start in seconds",
            input: "start in seconds",
            expect: &[(Pg, "false"), (MySql, "true"), (MariaDb, MARIADB)],
        },
        Case {
            name: "end says rollback",
            input: "end says rollback",
            expect: &[(Pg, "true"), (MySql, "false"), (MariaDb, MARIADB)],
        },
        Case {
            name: "replication channels",
            input: "replication channels",
            expect: &[(Pg, "Streams"), (MySql, "Named"), (MariaDb, MARIADB)],
        },
        Case {
            name: "replication receiver",
            input: "replication receiver",
            expect: &[(Pg, "WalReceiver"), (MySql, "Receiver"), (MariaDb, MARIADB)],
        },
        Case {
            name: "replication last message",
            input: "replication last message",
            expect: &[(Pg, "Message"), (MySql, "Heartbeat"), (MariaDb, MARIADB)],
        },
        Case {
            name: "replication receiver line always",
            input: "replication receiver line always",
            expect: &[(Pg, "false"), (MySql, "true"), (MariaDb, MARIADB)],
        },
        Case {
            name: "replication applier",
            input: "replication applier",
            expect: &[(Pg, "Replay"), (MySql, "Applier"), (MariaDb, MARIADB)],
        },
        Case {
            name: "replication applied on applier line",
            input: "replication applied on applier line",
            expect: &[(Pg, "false"), (MySql, "true"), (MariaDb, MARIADB)],
        },
        Case {
            name: "replication zero lag note",
            input: "replication zero lag note",
            expect: &[(Pg, "false"), (MySql, "true"), (MariaDb, MARIADB)],
        },
        Case {
            name: "replication lag bytes",
            input: "replication lag bytes",
            expect: &[(Pg, "Behind"), (MySql, "Fetched"), (MariaDb, MARIADB)],
        },
        Case {
            name: "replication replica",
            input: "replication replica",
            expect: &[(Pg, "NameClientStateSync"), (MySql, "ClientUserState"), (MariaDb, MARIADB)],
        },
    ]
}
