use super::*;
use crate::app::profiles::{DRIVERS, ProfileForm};
use crate::icons::{self, UNKNOWN_DRIVER};
use datarig_core::engine::EngineId;
use datarig_core::profile::ConnectionConfig;
use datarig_core::profile::dsn::Scheme;
use datarig_core::profile::session::Dbms;

const PG_GLYPH: &str = "\u{e76e}";
const MY_GLYPH: &str = "\u{e704}";
const PG: Option<&[EngineId]> = Some(&[EngineId::Postgres]);
const MY: Option<&[EngineId]> = Some(&[EngineId::MySql, EngineId::MariaDb]);

/// What every reader of a profile's driver name answers for it.
#[derive(Debug, PartialEq, Eq)]
struct Answers {
    /// The engines of the driver that serves it (`None`: no driver does).
    engines: Option<&'static [EngineId]>,
    scheme: Scheme,
    port: u16,
    tls_name: &'static str,
    dbms: Option<Dbms>,
    glyph: &'static str,
    /// The profile form: its driver row, that row's label, whether MySQL's fields are shown,
    /// the presets it offers and the driver name it saves.
    form_index: usize,
    form_label: &'static str,
    form_mysql: bool,
    form_dbms: Option<Dbms>,
    form_scheme: Scheme,
    saved_driver: String,
    /// The database the session view names (the name as written when no preset is for it).
    session_label: String,
}

/// A driver name and what it answers: engines, scheme, port, presets, icon, the form's driver
/// row, that row's label and whether the form shows MySQL's fields.
type Row =
    (&'static str, Option<&'static [EngineId]>, Scheme, u16, Option<Dbms>, &'static str, usize, &'static str, bool);

fn answers(driver: &str) -> Answers {
    let c = ConnectionConfig { driver: driver.to_string(), ..ConnectionConfig::default() };
    let form = ProfileForm::from_profile(&c, String::new(), None);
    let scheme = c.scheme();
    let dbms = c.dbms();
    Answers {
        engines: driver_for(driver).map(|d| d.engines()),
        scheme,
        port: scheme.default_port(),
        tls_name: form.tls_mode().name(scheme),
        dbms,
        glyph: icons::for_driver(driver),
        form_index: form.driver,
        form_label: DRIVERS[form.driver].1,
        form_mysql: form.form_fields().server_key,
        form_dbms: form.profile_dbms(),
        form_scheme: form.scheme(),
        saved_driver: form.to_profile().driver,
        session_label: dbms.map_or(driver.to_string(), |d| d.display().to_string()),
    }
}

/// Every reader of a driver name answers as it did before they all went through one parser:
/// the driver that connects, the URL form, port and TLS names, the presets, the profile form's
/// driver row and fields, the icon and the name the session view shows. The one change: an
/// engine's name written in another case shows its engine's icon (the rows marked), as every
/// other reader already read it.
#[test]
fn the_five_parsers_answer_as_before() {
    use Scheme::{MySql as MyScheme, Postgres as PgScheme};
    let (pg, my) = (Some(Dbms::Postgres), Some(Dbms::MySql));
    #[rustfmt::skip]
    let table: [Row; 19] = [
        ("postgres",      PG,   PgScheme, 5432, pg,   PG_GLYPH,       0, "PostgreSQL",    false),
        ("postgresql",    PG,   PgScheme, 5432, pg,   PG_GLYPH,       0, "PostgreSQL",    false),
        ("PostgreSQL",    PG,   PgScheme, 5432, pg,   PG_GLYPH,       0, "PostgreSQL",    false), // was the unknown icon
        ("pg",            PG,   PgScheme, 5432, pg,   PG_GLYPH,       0, "PostgreSQL",    false),
        ("PG",            PG,   PgScheme, 5432, pg,   PG_GLYPH,       0, "PostgreSQL",    false), // was the unknown icon
        ("mysql",         MY,   MyScheme, 3306, my,   MY_GLYPH,       1, "MySQL",         true),
        ("MySQL",         MY,   MyScheme, 3306, my,   MY_GLYPH,       1, "MySQL",         true),  // was the unknown icon
        ("mariadb",       MY,   MyScheme, 3306, my,   MY_GLYPH,       1, "MySQL",         true),
        ("MariaDB",       MY,   MyScheme, 3306, my,   MY_GLYPH,       1, "MySQL",         true),  // was the unknown icon
        ("MARIADB",       MY,   MyScheme, 3306, my,   MY_GLYPH,       1, "MySQL",         true),  // was the unknown icon
        ("redis",         None, PgScheme, 5432, None, "\u{e76d}",     2, "Redis",         false),
        ("Redis",         None, PgScheme, 5432, None, UNKNOWN_DRIVER, 2, "Redis",         false),
        ("elasticsearch", None, PgScheme, 5432, None, "\u{e7ca}",     3, "Elasticsearch", false),
        ("sqlite",        None, PgScheme, 5432, None, UNKNOWN_DRIVER, 0, "PostgreSQL",    false),
        ("oracle",        None, PgScheme, 5432, None, UNKNOWN_DRIVER, 0, "PostgreSQL",    false),
        ("",              None, PgScheme, 5432, None, UNKNOWN_DRIVER, 0, "PostgreSQL",    false),
        (" mysql",        None, PgScheme, 5432, None, UNKNOWN_DRIVER, 0, "PostgreSQL",    false),
        ("mysql ",        None, PgScheme, 5432, None, UNKNOWN_DRIVER, 0, "PostgreSQL",    false),
        ("my sql",        None, PgScheme, 5432, None, UNKNOWN_DRIVER, 0, "PostgreSQL",    false),
    ];
    for (driver, engines, scheme, port, dbms, glyph, form_index, form_label, form_mysql) in table {
        // The TLS mode a profile without one connects with, by the driver's name for it.
        let tls_name = match scheme {
            PgScheme => "disable",
            MyScheme => "PREFERRED",
        };
        // The form follows its driver row: a name no engine serves opens on the first row.
        let (form_dbms, form_scheme) = match form_index {
            0 => (pg, PgScheme),
            1 => (my, MyScheme),
            _ => (None, PgScheme),
        };
        let want = Answers {
            engines,
            scheme,
            port,
            tls_name,
            dbms,
            glyph,
            form_index,
            form_label,
            form_mysql,
            form_dbms,
            form_scheme,
            saved_driver: driver.to_string(),
            session_label: dbms.map_or(driver.to_string(), |d| d.display().to_string()),
        };
        assert_eq!(answers(driver), want, "{driver:?}");
    }
}

/// Each engine's own name reaches a driver that says it serves that engine.
#[test]
fn every_engine_has_a_driver_that_serves_it() {
    for &id in EngineId::ALL {
        let key = id.spec().key;
        let driver = driver_for(key).unwrap_or_else(|| panic!("{id:?}: no driver for {key:?}"));
        assert!(driver.engines().contains(&id), "{id:?}: the driver for {key:?} serves {:?}", driver.engines());
    }
}
