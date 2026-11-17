//! A UI module that names engines, even in an exhaustive match, and reads legacy markers.
use datarig_core::monitor::format::MonitorFormat;
use datarig_core::profile::dsn::Scheme;

fn session_id_label(format: &MonitorFormat) -> &'static str {
    // Exhaustive, but the engine is decided here instead of in the seam.
    match format {
        MonitorFormat::Postgres(_) => "pid",
        MonitorFormat::MySql(_) => "id",
    }
}

fn port(driver: &str, mysql: bool, mariadb: bool) -> u16 {
    if mysql || mariadb { 3306 } else { Scheme::of_driver(driver).default_port() }
}
