//! A driver module, not its engine module, that decides by engine.
use datarig_core::engine::EngineId;

fn own_read_bound(engine: EngineId) -> &'static str {
    match engine {
        EngineId::MariaDb => "SET STATEMENT max_statement_time = 2 FOR",
        EngineId::MySql | EngineId::Postgres => "",
    }
}
