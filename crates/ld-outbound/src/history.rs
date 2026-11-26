//! The [`Store`] port over `$XDG_STATE_HOME/lazydb/history.toml`, and
//! `saved.toml` beside it. State, not cache: cleaners may empty a cache.
//! `CREATE ROLE … PASSWORD` is a statement, so each file is 0600 in a 0700
//! directory.
//!
//! [`Store`]: ld_domain::ports::Store

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use ld_domain::clock::Stamp;
use ld_domain::config::{Config, ConnectionName, Diagnostic, Origin};
use ld_domain::engine::Engine;
use ld_domain::history::{Entry, Histories};
use ld_domain::ports::{Store, StoreError};
use ld_domain::saved::{Saved, SavedQuery};
use ld_domain::style::Severity;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

use crate::config::{non_empty_env, save::quote};

const HEADER: &str = "# LazyDB query history, newest first. Rewritten on every run.\n";

/// `None` without `$XDG_STATE_HOME` or `$HOME`.
fn path(xdg_state: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    let state = xdg_state
        .map(Path::to_path_buf)
        .or_else(|| home.map(|home| home.join(".local").join("state")))?;

    Some(state.join("lazydb").join("history.toml"))
}

#[derive(Debug, Default, Deserialize)]
struct File {
    #[serde(default)]
    connections: BTreeMap<String, Section>,
}

#[derive(Debug, Default, Deserialize)]
struct Section {
    #[serde(default)]
    entries: Vec<Wire>,
}

/// Files from before 0.4 hold bare strings.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Wire {
    Bare(String),
    Full(Stored),
}

/// As the file holds it: an engine this build does not know survives a save
/// by another connection.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct Stored {
    pub(crate) statement: String,
    pub(crate) at: Option<u64>,
    pub(crate) engine: Option<String>,
    pub(crate) database: Option<String>,
}

impl Stored {
    pub(crate) fn of(entry: &Entry) -> Self {
        Self {
            statement: entry.statement.clone(),
            at: entry.at.map(Stamp::unix),
            engine: Some(entry.engine.key().to_owned()),
            database: entry.database.clone(),
        }
    }

    /// `fallback` for an entry that names no engine, or one this build lacks.
    pub(crate) fn entry(self, fallback: Engine) -> Entry {
        let engine = Engine::ALL
            .into_iter()
            .find(|engine| self.engine.as_deref() == Some(engine.key()))
            .unwrap_or(fallback);

        Entry {
            statement: self.statement,
            at: self.at.map(Stamp::from_unix),
            engine,
            database: self.database,
        }
    }
}

type Sections = BTreeMap<String, Vec<Stored>>;

fn parse(source: &str) -> Result<Sections, String> {
    let file: File = toml::from_str(source).map_err(|error| error.message().to_owned())?;

    Ok(file
        .connections
        .into_iter()
        .map(|(name, section)| {
            let entries = section
                .entries
                .into_iter()
                .map(|wire| match wire {
                    Wire::Bare(statement) => Stored {
                        statement,
                        at: None,
                        engine: None,
                        database: None,
                    },
                    Wire::Full(stored) => stored,
                })
                .collect();
            (name, entries)
        })
        .collect())
}

/// Hand-rolled: `toml`'s serialiser is off. One inline table a line, the
/// statement last so the metadata lines up.
fn render(sections: &Sections) -> String {
    let mut out = String::from(HEADER);

    for (name, entries) in sections.iter().filter(|(_, entries)| !entries.is_empty()) {
        // Writing into a String cannot fail.
        let _written = write!(out, "\n[connections.{}]\nentries = [\n", quote(name));

        for entry in entries {
            out.push_str("  { ");
            fields(&mut out, entry);
        }

        out.push_str("]\n");
    }

    out
}

/// The rest of an inline table, closing it: the statement last so the
/// metadata lines up.
pub(crate) fn fields(out: &mut String, entry: &Stored) {
    // Writing into a String cannot fail.
    if let Some(at) = entry.at {
        let _written = write!(out, "at = {at}, ");
    }
    if let Some(engine) = &entry.engine {
        let _written = write!(out, "engine = {}, ", quote(engine));
    }
    if let Some(database) = &entry.database {
        let _written = write!(out, "database = {}, ", quote(database));
    }
    let _written = writeln!(out, "statement = {} }},", quote(&entry.statement));
}

#[derive(Debug)]
pub struct FileStore {
    path: Option<PathBuf>,
    saved: Option<PathBuf>,
    /// Saves read-modify-write one file; two at once would each start from the
    /// old file.
    writes: tokio::sync::Mutex<()>,
}

impl FileStore {
    #[must_use]
    pub fn new(xdg_state: Option<&Path>, home: Option<&Path>) -> Self {
        Self::at(path(xdg_state, home))
    }

    /// The saved queries beside `path`.
    #[must_use]
    pub fn at(path: Option<PathBuf>) -> Self {
        Self {
            saved: path
                .as_ref()
                .map(|path| path.with_file_name(crate::saved::FILE)),
            path,
            writes: tokio::sync::Mutex::new(()),
        }
    }

    #[must_use]
    pub fn from_env() -> Self {
        let xdg_state = non_empty_env("XDG_STATE_HOME");
        let home = non_empty_env("HOME");

        Self::new(xdg_state.as_deref(), home.as_deref())
    }

    async fn read(path: &Path) -> Result<Option<String>, std::io::Error> {
        match tokio::fs::read_to_string(path).await {
            Ok(source) => Ok(Some(source)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
}

pub(crate) fn warning(path: &Path, message: String) -> Diagnostic {
    Diagnostic {
        origin: Origin::new(&path.display().to_string()),
        span: None,
        severity: Severity::Warning,
        message,
    }
}

#[async_trait]
impl Store for FileStore {
    async fn load(&self, config: &Config) -> (Histories, Vec<Diagnostic>) {
        let limit = config.history.limit;
        let mut histories = Histories::default();
        let Some(path) = self.path.as_ref().filter(|_| !limit.is_off()) else {
            return (histories, Vec::new());
        };
        let source = match Self::read(path).await {
            Ok(Some(source)) => source,
            Ok(None) => return (histories, Vec::new()),
            Err(error) => {
                let message = format!("query history not read: {error}");
                return (histories, vec![warning(path, message)]);
            }
        };
        let sections = match parse(&source) {
            Ok(sections) => sections,
            Err(message) => {
                let message =
                    format!("query history ignored, and replaced on the next run: {message}");
                return (histories, vec![warning(path, message)]);
            }
        };
        let mut diagnostics = Vec::new();

        for (name, stored) in sections {
            match ConnectionName::try_new(&name) {
                Ok(name) => {
                    let fallback = engine_of(config, &name);
                    let entries = stored
                        .into_iter()
                        .map(|stored| stored.entry(fallback))
                        .collect();
                    histories.restore(&name, entries, limit);
                }
                Err(error) => diagnostics.push(warning(
                    path,
                    format!("history for `{name}` ignored: {error}"),
                )),
            }
        }

        (histories, diagnostics)
    }

    async fn save(&self, connection: &ConnectionName, entries: &[Entry]) -> Result<(), StoreError> {
        let Some(path) = &self.path else {
            return Err(StoreError::Unavailable(
                "neither XDG_STATE_HOME nor HOME is set".to_owned(),
            ));
        };
        let failed =
            |error: std::io::Error| StoreError::Unavailable(format!("{}: {error}", path.display()));
        let _one_at_a_time = self.writes.lock().await;

        // An unparsable file is replaced: `load` already reported it.
        let mut sections = Self::read(path)
            .await
            .map_err(failed)?
            .and_then(|source| parse(&source).ok())
            .unwrap_or_default();
        sections.insert(
            connection.as_str().to_owned(),
            entries.iter().map(Stored::of).collect(),
        );

        write(path, &render(&sections)).await.map_err(failed)
    }

    async fn load_saved(&self, config: &Config) -> (Saved, Vec<Diagnostic>) {
        let Some(path) = &self.saved else {
            return (Saved::default(), Vec::new());
        };
        let source = match Self::read(path).await {
            Ok(Some(source)) => source,
            Ok(None) => return (Saved::default(), Vec::new()),
            Err(error) => {
                let message = format!("saved queries not read: {error}");
                return (Saved::default(), vec![warning(path, message)]);
            }
        };

        match crate::saved::parse(&source) {
            Ok(sections) => crate::saved::restore(sections, config, path),
            // Not replaced, unlike the history: they were named by hand.
            Err(message) => {
                let message =
                    format!("saved queries ignored; fix the file before saving another: {message}");
                (Saved::default(), vec![warning(path, message)])
            }
        }
    }

    async fn save_queries(
        &self,
        connection: &ConnectionName,
        queries: &[SavedQuery],
    ) -> Result<(), StoreError> {
        let Some(path) = &self.saved else {
            return Err(StoreError::Unavailable(
                "neither XDG_STATE_HOME nor HOME is set".to_owned(),
            ));
        };
        let failed =
            |error: std::io::Error| StoreError::Unavailable(format!("{}: {error}", path.display()));
        let _one_at_a_time = self.writes.lock().await;

        let source = Self::read(path).await.map_err(failed)?;
        let mut sections = match source.as_deref().map(crate::saved::parse) {
            Some(Ok(sections)) => sections,
            None => crate::saved::Sections::default(),
            Some(Err(message)) => {
                return Err(StoreError::Unavailable(format!(
                    "{}: not overwritten, it does not parse: {message}",
                    path.display()
                )));
            }
        };
        sections.insert(
            connection.as_str().to_owned(),
            queries.iter().map(crate::saved::Wire::of).collect(),
        );

        write(path, &crate::saved::render(&sections))
            .await
            .map_err(failed)
    }
}

/// A connection since removed from the config is taken for the default.
pub(crate) fn engine_of(config: &Config, name: &ConnectionName) -> Engine {
    config
        .connections
        .iter()
        .find(|connection| connection.name == *name)
        .map_or_else(Engine::default, ld_domain::config::ConnectionConfig::engine)
}

/// Beside the target for one filesystem; named after this process so two cannot
/// collide.
async fn write(path: &Path, contents: &str) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        let mut directory = tokio::fs::DirBuilder::new();
        directory.recursive(true);
        #[cfg(unix)]
        directory.mode(0o700);
        directory.create(parent).await?;
    }

    let stem = path
        .file_stem()
        .map_or_else(Default::default, |stem| stem.to_string_lossy());
    let temporary = path.with_file_name(format!(".{stem}.{}.tmp", std::process::id()));
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);

    let mut file = options.open(&temporary).await?;
    let written = async {
        file.write_all(contents.as_bytes()).await?;
        file.flush().await
    }
    .await;

    if let Err(error) = written {
        let _ignored = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }

    tokio::fs::rename(&temporary, path).await.inspect_err(|_| {
        let _ignored = std::fs::remove_file(&temporary);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(statement: &str) -> Stored {
        Stored {
            statement: statement.to_owned(),
            at: Some(1_764_153_015),
            engine: Some("postgres".to_owned()),
            database: Some("shop".to_owned()),
        }
    }

    fn sections(of: &[(&str, &[Stored])]) -> Sections {
        of.iter()
            .map(|(name, entries)| ((*name).to_owned(), entries.to_vec()))
            .collect()
    }

    #[test]
    fn the_file_is_under_the_state_directory() {
        assert_eq!(
            path(Some(Path::new("/s")), Some(Path::new("/h"))),
            Some(PathBuf::from("/s/lazydb/history.toml"))
        );
    }

    #[test]
    fn without_a_state_directory_it_falls_back_to_the_home_one() {
        assert_eq!(
            path(None, Some(Path::new("/h"))),
            Some(PathBuf::from("/h/.local/state/lazydb/history.toml"))
        );
        assert_eq!(path(None, None), None);
    }

    #[test]
    fn what_is_rendered_parses_back() {
        let held = sections(&[
            (
                "dev",
                &[
                    stored("SELECT 1;"),
                    stored("SELECT *\nFROM \"a\\b\" WHERE x = 'é';"),
                ],
            ),
            (
                "events",
                &[Stored {
                    engine: Some("cassandra".to_owned()),
                    database: None,
                    ..stored("SELECT 2;")
                }],
            ),
        ]);

        assert_eq!(parse(&render(&held)), Ok(held));
    }

    #[test]
    fn an_entry_is_one_line_with_its_statement_last() {
        let rendered = render(&sections(&[("dev", &[stored("SELECT\n1;")])]));

        assert!(
            rendered.contains(
                "  { at = 1764153015, engine = \"postgres\", database = \"shop\", \
                 statement = \"SELECT\\u000A1;\" },\n"
            ),
            "{rendered}"
        );
    }

    #[test]
    fn a_connection_with_nothing_is_left_out() {
        let rendered = render(&sections(&[("dev", &[]), ("prod", &[stored("SELECT 2;")])]));

        assert!(!rendered.contains("dev"));
        assert!(rendered.contains("prod"));
    }

    #[test]
    fn a_name_that_needs_quoting_survives() {
        let held = sections(&[("we\"ird.name", &[stored("SELECT 1;")])]);

        assert_eq!(parse(&render(&held)), Ok(held));
    }

    #[test]
    fn a_file_that_is_not_toml_is_an_error_not_a_panic() {
        assert!(parse("[connections.dev\nentries = ").is_err());
    }

    #[test]
    fn keys_nobody_declared_are_ignored() {
        let source = "future = 1\n[connections.dev]\nentries = [\n  \
                      { at = 1764153015, engine = \"postgres\", database = \"shop\", \
                      statement = \"SELECT 1;\", took = 3 },\n]\nextra = true\n";

        assert_eq!(
            parse(source),
            Ok(sections(&[("dev", &[stored("SELECT 1;")])]))
        );
    }

    #[test]
    fn a_file_of_bare_statements_still_loads() {
        let source = "[connections.dev]\nentries = [\"SELECT 1;\", \"SELECT 2;\"]\n";

        assert_eq!(
            parse(source),
            Ok(sections(&[(
                "dev",
                &[
                    Stored {
                        statement: "SELECT 1;".to_owned(),
                        at: None,
                        engine: None,
                        database: None,
                    },
                    Stored {
                        statement: "SELECT 2;".to_owned(),
                        at: None,
                        engine: None,
                        database: None,
                    },
                ],
            )]))
        );
    }

    #[test]
    fn an_entry_without_a_known_engine_takes_its_connections() {
        let unnamed = Stored {
            engine: None,
            ..stored("SELECT 1;")
        };
        let unknown = Stored {
            engine: Some("oracle".to_owned()),
            ..stored("SELECT 1;")
        };

        assert_eq!(unnamed.entry(Engine::Cassandra).engine, Engine::Cassandra);
        assert_eq!(unknown.entry(Engine::Cassandra).engine, Engine::Cassandra);
        assert_eq!(
            stored("SELECT 1;").entry(Engine::Cassandra).engine,
            Engine::Postgres
        );
    }

    #[test]
    fn an_entry_reads_back_as_it_was_recorded() {
        let entry = Entry {
            statement: "SELECT 1;".to_owned(),
            at: Some(Stamp::from_unix(42)),
            engine: Engine::Cassandra,
            database: None,
        };

        assert_eq!(Stored::of(&entry).entry(Engine::Postgres), entry);
    }
}
