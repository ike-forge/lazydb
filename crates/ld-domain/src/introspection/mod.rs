//! The queries that read a catalog, as data: the adapter runs them and maps
//! their rows, and the Logs tab can show exactly what was asked (vision.md §4,
//! principle 3). One query per kind of object.

pub mod cassandra;
pub mod postgres;

use crate::engine::Engine;

/// In the order they are sent, each with what it reads.
#[must_use]
pub const fn queries(engine: Engine) -> &'static [(&'static str, &'static str)] {
    match engine {
        Engine::Postgres => &postgres::ALL,
        Engine::Cassandra => &cassandra::ALL,
    }
}
