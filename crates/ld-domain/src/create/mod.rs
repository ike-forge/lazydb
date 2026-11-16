//! A relation's definition as the statements that would create it, rebuilt
//! from the catalog in the engine's language.

mod cassandra;
mod postgres;

use crate::engine::Engine;
use crate::model::{Schema, Table};

/// Lines, so the Structure panel can draw and yank them one at a time.
#[must_use]
pub fn statements(engine: Engine, schema: &Schema, table: &Table) -> Vec<String> {
    match engine {
        Engine::Postgres => postgres::statements(schema, table),
        Engine::Cassandra => cassandra::statements(schema, table),
    }
}

/// Bare when the engine would read it back unchanged, quoted otherwise.
#[must_use]
pub fn ident(engine: Engine, name: &str) -> String {
    match engine {
        Engine::Postgres => postgres::ident(name),
        Engine::Cassandra => cassandra::ident(name),
    }
}

fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}
