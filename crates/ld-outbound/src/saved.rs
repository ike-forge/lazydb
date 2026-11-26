//! `saved.toml`: the saved queries, one section per connection, newest first.
//! Written and locked by [`FileStore`](crate::history::FileStore).

use std::collections::BTreeMap;
use std::path::Path;

use ld_domain::config::{Config, ConnectionName, Diagnostic};
use ld_domain::saved::{QueryName, Saved, SavedQuery};
use serde::Deserialize;

use crate::config::save::quote;
use crate::history::{Stored, engine_of, fields, warning};

pub(crate) const FILE: &str = "saved.toml";

const HEADER: &str = "# LazyDB saved queries, newest first. Rewritten on every save.\n";

#[derive(Debug, Default, Deserialize)]
struct File {
    #[serde(default)]
    connections: BTreeMap<String, Section>,
}

#[derive(Debug, Default, Deserialize)]
struct Section {
    #[serde(default)]
    queries: Vec<Wire>,
}

/// As the file holds it: a name this build refuses is reported, not lost
/// silently.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Wire {
    name: String,
    #[serde(flatten)]
    stored: Stored,
}

impl Wire {
    pub(crate) fn of(query: &SavedQuery) -> Self {
        Self {
            name: query.name.as_str().to_owned(),
            stored: Stored::of(&query.entry),
        }
    }
}

pub(crate) type Sections = BTreeMap<String, Vec<Wire>>;

pub(crate) fn parse(source: &str) -> Result<Sections, String> {
    let file: File = toml::from_str(source).map_err(|error| error.message().to_owned())?;

    Ok(file
        .connections
        .into_iter()
        .map(|(name, section)| (name, section.queries))
        .collect())
}

/// Hand-rolled, as the history's: one inline table a line, the name first.
pub(crate) fn render(sections: &Sections) -> String {
    let mut out = String::from(HEADER);

    for (name, queries) in sections.iter().filter(|(_, queries)| !queries.is_empty()) {
        out.push_str("\n[connections.");
        out.push_str(&quote(name));
        out.push_str("]\nqueries = [\n");

        for query in queries {
            out.push_str("  { name = ");
            out.push_str(&quote(&query.name));
            out.push_str(", ");
            fields(&mut out, &query.stored);
        }

        out.push_str("]\n");
    }

    out
}

/// A connection or query name the domain refuses is reported and skipped.
pub(crate) fn restore(
    sections: Sections,
    config: &Config,
    path: &Path,
) -> (Saved, Vec<Diagnostic>) {
    let mut saved = Saved::default();
    let mut diagnostics = Vec::new();

    for (name, wires) in sections {
        let connection = match ConnectionName::try_new(&name) {
            Ok(connection) => connection,
            Err(error) => {
                diagnostics.push(warning(
                    path,
                    format!("saved queries for `{name}` ignored: {error}"),
                ));
                continue;
            }
        };
        let fallback = engine_of(config, &connection);
        let mut queries = Vec::with_capacity(wires.len());

        for wire in wires {
            match QueryName::try_new(&wire.name) {
                Ok(called) => queries.push(SavedQuery {
                    name: called,
                    entry: wire.stored.entry(fallback),
                }),
                Err(error) => diagnostics.push(warning(
                    path,
                    format!("a saved query on `{name}` ignored: {error}"),
                )),
            }
        }

        saved.restore(&connection, queries);
    }

    (saved, diagnostics)
}

#[cfg(test)]
mod tests {
    use ld_domain::clock::Stamp;
    use ld_domain::engine::Engine;
    use ld_domain::history::Entry;

    use super::*;

    fn wire(name: &str, statement: &str) -> Wire {
        Wire {
            name: name.to_owned(),
            stored: Stored {
                statement: statement.to_owned(),
                at: Some(1_764_153_015),
                engine: Some("postgres".to_owned()),
                database: Some("shop".to_owned()),
            },
        }
    }

    fn sections(of: &[(&str, &[Wire])]) -> Sections {
        of.iter()
            .map(|(name, queries)| ((*name).to_owned(), queries.to_vec()))
            .collect()
    }

    #[test]
    fn what_is_rendered_parses_back() {
        let held = sections(&[
            (
                "dev",
                &[
                    wire("pending \"orders\"", "SELECT *\nFROM orders;"),
                    wire("é", "SELECT 1;"),
                ],
            ),
            ("prod", &[wire("one", "SELECT 1;")]),
        ]);

        assert_eq!(parse(&render(&held)), Ok(held));
    }

    #[test]
    fn a_query_is_one_line_with_its_name_first_and_statement_last() {
        let rendered = render(&sections(&[("dev", &[wire("one", "SELECT 1;")])]));

        assert!(
            rendered.contains(
                "  { name = \"one\", at = 1764153015, engine = \"postgres\", \
                 database = \"shop\", statement = \"SELECT 1;\" },\n"
            ),
            "{rendered}"
        );
    }

    #[test]
    fn a_connection_with_nothing_saved_is_left_out() {
        let rendered = render(&sections(&[
            ("dev", &[]),
            ("prod", &[wire("a", "SELECT 1;")]),
        ]));

        assert!(!rendered.contains("dev"));
        assert!(rendered.contains("prod"));
    }

    #[test]
    fn a_query_without_a_name_does_not_parse() {
        assert!(parse("[connections.dev]\nqueries = [{ statement = \"SELECT 1;\" }]\n").is_err());
    }

    #[test]
    fn a_refused_name_is_reported_and_the_rest_kept() {
        let held = sections(&[("dev", &[wire("  ", "SELECT 1;"), wire("ok", "SELECT 2;")])]);

        let (saved, diagnostics) = restore(held, &Config::default(), Path::new("/s/saved.toml"));

        let dev = ConnectionName::try_new("dev").expect("a valid name");
        assert_eq!(saved.of(&dev).len(), 1);
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn a_saved_query_reads_back_as_it_was_saved() {
        let query = SavedQuery {
            name: QueryName::try_new("one").expect("a valid name"),
            entry: Entry {
                statement: "SELECT 1;".to_owned(),
                at: Some(Stamp::from_unix(42)),
                engine: Engine::Cassandra,
                database: None,
            },
        };
        let held = sections(&[("dev", &[Wire::of(&query)])]);

        let (saved, _) = restore(held, &Config::default(), Path::new("/s/saved.toml"));

        let dev = ConnectionName::try_new("dev").expect("a valid name");
        assert_eq!(saved.of(&dev), [query]);
    }
}
