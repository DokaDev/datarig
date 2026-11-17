//! A glob import of the variants is reported: variants are named through their enum.
use datarig_core::engine::EngineId;
use datarig_core::engine::EngineId::*;

fn default_port(engine: EngineId) -> u16 {
    match engine {
        Postgres => 5432,
        // MySQL, and every engine added later.
        _ => 3306,
    }
}
