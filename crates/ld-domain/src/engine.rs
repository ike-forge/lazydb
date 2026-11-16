//! Which database a connection speaks to. Everything the panels show is in the
//! shared vocabulary of [`crate::model`]; the engine decides only how that
//! vocabulary is spelled back as statements.

/// `Postgres` is the default: a catalog or statement built without a
/// connection is spelled as SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Engine {
    #[default]
    Postgres,
    Cassandra,
}

impl Engine {
    pub const ALL: [Self; 2] = [Self::Postgres, Self::Cassandra];

    /// The one spelling a config file uses.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Cassandra => "cassandra",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Postgres => "PostgreSQL",
            Self::Cassandra => "Cassandra",
        }
    }

    /// The language the editor tab and the log name.
    #[must_use]
    pub const fn language(self) -> &'static str {
        match self {
            Self::Postgres => "SQL",
            Self::Cassandra => "CQL",
        }
    }

    /// What this engine calls a [`crate::model::Schema`].
    #[must_use]
    pub const fn namespace(self) -> &'static str {
        match self {
            Self::Postgres => "schema",
            Self::Cassandra => "keyspace",
        }
    }

    /// A user's `BEGIN` holds every later request, so the session is asked
    /// after each write. Cassandra has none: a batch is one request.
    #[must_use]
    pub const fn has_transactions(self) -> bool {
        matches!(self, Self::Postgres)
    }

    /// Whether a server holds several databases a session switches between.
    /// A Cassandra cluster is one keyspace set.
    #[must_use]
    pub const fn has_databases(self) -> bool {
        matches!(self, Self::Postgres)
    }

    /// Whether the engine accepts every statement of a buffer in one request.
    /// Cassandra's protocol takes one per request; a batch is one statement.
    #[must_use]
    pub const fn runs_scripts(self) -> bool {
        matches!(self, Self::Postgres)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_has_a_distinct_config_key() {
        let keys: std::collections::BTreeSet<_> = Engine::ALL.iter().map(|e| e.key()).collect();
        assert_eq!(keys.len(), Engine::ALL.len());
    }
}
