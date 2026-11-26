//! Statements kept under a name, newest first. A name is unique per
//! connection, case aside; keyed by connection name as the history is.

use std::collections::BTreeMap;

use crate::config::{ConnectionName, OutOfRange};
use crate::history::Entry;
use crate::line::Line;

/// Characters, not bytes.
pub const MAX_NAME: usize = 64;

/// Trimmed, one line, at most [`MAX_NAME`] characters. Compared ignoring case:
/// `Orders` and `orders` are one name.
#[derive(Debug, Clone, Eq)]
pub struct QueryName(String);

impl QueryName {
    /// # Errors
    ///
    /// When `name` is blank, longer than [`MAX_NAME`] or holds a control
    /// character.
    pub fn try_new(name: &str) -> Result<Self, OutOfRange> {
        let name = name.trim();

        if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control)
        {
            return Err(OutOfRange {
                what: "query name",
                value: name.to_owned(),
                expected: "non-empty, one line, at most 64 characters",
            });
        }

        Ok(Self(name.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn folded(&self) -> String {
        self.0.to_lowercase()
    }
}

impl PartialEq for QueryName {
    fn eq(&self, other: &Self) -> bool {
        self.folded() == other.folded()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedQuery {
    pub name: QueryName,
    /// `at` is when it was saved.
    pub entry: Entry,
}

/// Ordered, for determinism.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Saved(BTreeMap<ConnectionName, Vec<SavedQuery>>);

impl Saved {
    /// Replaces the query of the same name, so an overwrite moves to the top.
    pub fn save(&mut self, connection: &ConnectionName, query: SavedQuery) {
        let queries = self.0.entry(connection.clone()).or_default();

        queries.retain(|kept| kept.name != query.name);
        queries.insert(0, query);
    }

    /// A name a hand-edited file repeats keeps its first, newest, query.
    pub fn restore(&mut self, connection: &ConnectionName, stored: Vec<SavedQuery>) {
        let mut queries: Vec<SavedQuery> = Vec::with_capacity(stored.len());

        for query in stored {
            if !queries.iter().any(|kept| kept.name == query.name) {
                queries.push(query);
            }
        }

        self.0.insert(connection.clone(), queries);
    }

    #[must_use]
    pub fn of(&self, connection: &ConnectionName) -> &[SavedQuery] {
        self.0.get(connection).map_or(&[], Vec::as_slice)
    }

    #[must_use]
    pub fn find(&self, connection: &ConnectionName, name: &QueryName) -> Option<&SavedQuery> {
        self.of(connection).iter().find(|query| query.name == *name)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.values().all(Vec::is_empty)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&ConnectionName, &SavedQuery)> {
        self.0
            .iter()
            .flat_map(|(name, queries)| queries.iter().map(move |query| (name, query)))
    }
}

/// `ctrl-s` in the buffer: the statement under the cursor, named. The
/// overwrite question is a field, so it cannot be asked without a form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveForm {
    connection: ConnectionName,
    entry: Entry,
    name: Line,
    /// The name `⏎` found taken: `y` overwrites, `n` goes back to the name.
    taken: Option<QueryName>,
}

impl SaveForm {
    #[must_use]
    pub fn opened(connection: ConnectionName, entry: Entry) -> Self {
        Self {
            connection,
            entry,
            name: Line::default(),
            taken: None,
        }
    }

    #[must_use]
    pub const fn connection(&self) -> &ConnectionName {
        &self.connection
    }

    #[must_use]
    pub fn statement(&self) -> &str {
        &self.entry.statement
    }

    #[must_use]
    pub const fn name(&self) -> &Line {
        &self.name
    }

    /// `None` while the overwrite question is up.
    pub const fn name_mut(&mut self) -> Option<&mut Line> {
        match self.taken {
            Some(_) => None,
            None => Some(&mut self.name),
        }
    }

    /// `None` while the name is blank or too long: `⏎` is inert.
    #[must_use]
    pub fn valid_name(&self) -> Option<QueryName> {
        QueryName::try_new(self.name.text()).ok()
    }

    #[must_use]
    pub const fn taken(&self) -> Option<&QueryName> {
        self.taken.as_ref()
    }

    pub fn ask_overwrite(&mut self, name: QueryName) {
        self.taken = Some(name);
    }

    pub fn back_to_name(&mut self) {
        self.taken = None;
    }

    #[must_use]
    pub fn into_query(self, name: QueryName) -> (ConnectionName, SavedQuery) {
        (
            self.connection,
            SavedQuery {
                name,
                entry: self.entry,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::Stamp;
    use crate::engine::Engine;

    fn name(name: &str) -> QueryName {
        QueryName::try_new(name).expect("a valid name")
    }

    fn connection(name: &str) -> ConnectionName {
        ConnectionName::try_new(name).expect("a valid name")
    }

    fn query(called: &str, statement: &str, at: u64) -> SavedQuery {
        SavedQuery {
            name: name(called),
            entry: Entry {
                at: Some(Stamp::from_unix(at)),
                ..Entry::bare(statement, Engine::Postgres)
            },
        }
    }

    fn names(saved: &Saved, on: &str) -> Vec<String> {
        saved
            .of(&connection(on))
            .iter()
            .map(|query| query.name.as_str().to_owned())
            .collect()
    }

    #[test]
    fn a_name_is_trimmed() {
        assert_eq!(name("  pending orders ").as_str(), "pending orders");
    }

    #[test]
    fn a_blank_long_or_multi_line_name_is_refused() {
        assert!(QueryName::try_new(" \t").is_err());
        assert!(QueryName::try_new("a\nb").is_err());
        assert!(QueryName::try_new(&"x".repeat(MAX_NAME + 1)).is_err());
        assert!(QueryName::try_new(&"é".repeat(MAX_NAME)).is_ok());
    }

    #[test]
    fn names_differing_in_case_are_one_name() {
        assert_eq!(name("Orders"), name("orders"));
    }

    #[test]
    fn the_newest_save_comes_first() {
        let mut saved = Saved::default();
        saved.save(&connection("dev"), query("a", "SELECT 1;", 1));
        saved.save(&connection("dev"), query("b", "SELECT 2;", 2));

        assert_eq!(names(&saved, "dev"), ["b", "a"]);
    }

    #[test]
    fn saving_a_taken_name_replaces_it_and_moves_it_up() {
        let mut saved = Saved::default();
        saved.save(&connection("dev"), query("a", "SELECT 1;", 1));
        saved.save(&connection("dev"), query("b", "SELECT 2;", 2));
        saved.save(&connection("dev"), query("A", "SELECT 3;", 3));

        assert_eq!(names(&saved, "dev"), ["A", "b"]);
        assert_eq!(
            saved
                .find(&connection("dev"), &name("a"))
                .map(|query| query.entry.statement.as_str()),
            Some("SELECT 3;")
        );
    }

    #[test]
    fn each_connection_has_its_own_names() {
        let mut saved = Saved::default();
        saved.save(&connection("dev"), query("a", "SELECT 1;", 1));
        saved.save(&connection("prod"), query("a", "SELECT 2;", 2));

        assert_eq!(names(&saved, "dev"), ["a"]);
        assert_eq!(names(&saved, "prod"), ["a"]);
        assert!(saved.find(&connection("stage"), &name("a")).is_none());
    }

    #[test]
    fn restoring_keeps_the_first_of_a_repeated_name() {
        let mut saved = Saved::default();
        saved.restore(
            &connection("dev"),
            vec![
                query("a", "SELECT 2;", 2),
                query("A", "SELECT 1;", 1),
                query("b", "SELECT 3;", 0),
            ],
        );

        assert_eq!(names(&saved, "dev"), ["a", "b"]);
    }
}
