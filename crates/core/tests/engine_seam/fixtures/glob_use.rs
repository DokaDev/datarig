//! A glob import of the variants: the bare names are still engine variants.
use datarig_core::engine::EngineId;
use datarig_core::engine::EngineId::*;

fn default_port(engine: EngineId) -> u16 {
    match engine {
        Postgres => 5432,
        // MySQL, and every engine added later.
        _ => 3306,
    }
}
