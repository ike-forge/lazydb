//! What each connection has been asked, newest first. Keyed by name, not
//! [`ConnId`], so a reconnect keeps it. Recalled in three [`Layer`]s.
//!
//! [`ConnId`]: crate::model::ConnId

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use crate::clock::Stamp;
use crate::config::{ConnectionName, HistoryLimit};
use crate::engine::Engine;
use crate::fuzzy::Fuzzy;
use crate::line::Line;
use crate::sql::fold;
use crate::viewport::Cursor;

/// One statement as it was run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub statement: String,
    /// `None` for an entry from a file written before times were kept.
    pub at: Option<Stamp>,
    pub engine: Engine,
    /// The database a Postgres session was scoped to; `None` on an engine
    /// without databases and in older files.
    pub database: Option<String>,
}

impl Entry {
    /// An entry with nothing but its statement, as older files hold.
    #[must_use]
    pub fn bare(statement: &str, engine: Engine) -> Self {
        Self {
            statement: statement.to_owned(),
            at: None,
            engine,
            database: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    entries: Vec<Entry>,
}

impl History {
    /// A repeat of the newest, case aside, replaces it, so asking again moves
    /// its time; a blank statement is ignored. `false` when nothing changed,
    /// so the same statement in the same second costs no write.
    pub fn record(&mut self, entry: Entry, limit: HistoryLimit) -> bool {
        let statement = entry.statement.trim();

        if limit.is_off() || statement.is_empty() {
            return false;
        }
        let entry = Entry {
            statement: statement.to_owned(),
            ..entry
        };

        match self.entries.first_mut() {
            Some(newest) if *newest == entry => return false,
            Some(newest) if same(&newest.statement, &entry.statement) => *newest = entry,
            _ => self.entries.insert(0, entry),
        }

        self.entries
            .truncate(usize::try_from(limit.get()).unwrap_or(usize::MAX));
        true
    }

    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
}

/// How far back the picker looks, narrowest first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Layer {
    /// The connected entry's own.
    #[default]
    Connection,
    /// Every connection on the connected entry's engine: the statement run on
    /// staging, recalled on production.
    Engine,
    All,
}

impl Layer {
    pub const ALL: [Self; 3] = [Self::Connection, Self::Engine, Self::All];

    /// Wraps, as atuin's `ctrl-r` does.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Connection => Self::Engine,
            Self::Engine => Self::All,
            Self::All => Self::Connection,
        }
    }
}

/// An entry and the connection that ran it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recalled<'a> {
    pub connection: &'a ConnectionName,
    pub entry: &'a Entry,
}

/// Ordered, for determinism.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Histories(BTreeMap<ConnectionName, History>);

impl Histories {
    pub fn record(
        &mut self,
        connection: &ConnectionName,
        entry: Entry,
        limit: HistoryLimit,
    ) -> bool {
        self.0
            .entry(connection.clone())
            .or_default()
            .record(entry, limit)
    }

    /// Same rules as [`History::record`], so a hand-edited file still yields a
    /// valid list.
    pub fn restore(
        &mut self,
        connection: &ConnectionName,
        stored: Vec<Entry>,
        limit: HistoryLimit,
    ) {
        let mut history = History::default();

        for entry in stored.into_iter().rev() {
            history.record(entry, limit);
        }

        self.0.insert(connection.clone(), history);
    }

    #[must_use]
    pub fn of(&self, connection: &ConnectionName) -> &[Entry] {
        self.0.get(connection).map_or(&[], History::entries)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.values().all(|history| history.entries.is_empty())
    }

    /// Newest first, each statement once: a statement asked on several
    /// connections, or several times, is its newest run. An entry without a
    /// time sorts after every entry with one.
    #[must_use]
    pub fn recall(
        &self,
        layer: Layer,
        connection: Option<&ConnectionName>,
        engine: Engine,
    ) -> Vec<Recalled<'_>> {
        let mut recalled: Vec<Recalled<'_>> = self
            .0
            .iter()
            .filter(|(name, _)| layer != Layer::Connection || Some(*name) == connection)
            .flat_map(|(name, history)| {
                history.entries.iter().map(move |entry| Recalled {
                    connection: name,
                    entry,
                })
            })
            .filter(|recalled| layer != Layer::Engine || recalled.entry.engine == engine)
            .collect();

        // Stable, so equal times keep connection order, then the file's.
        recalled.sort_by_key(|recalled| std::cmp::Reverse(recalled.entry.at));

        let mut seen = BTreeSet::new();
        recalled.retain(|recalled| seen.insert(folded(&recalled.entry.statement)));
        recalled
    }
}

/// The open picker. The query takes keys at once, as atuin's does; the cursor
/// counts in what it left.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Picker {
    pub layer: Layer,
    pub query: Line,
    pub cursor: Cursor,
}

/// A recalled entry as the picker lists it: `ranges` are the query's hits in
/// `label`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranked<'a> {
    pub recalled: Recalled<'a>,
    pub label: String,
    pub ranges: Vec<Range<usize>>,
}

/// Best hit first, newest first among equals; in recall order without a
/// query.
#[must_use]
pub fn ranked<'a>(recalled: Vec<Recalled<'a>>, query: &str) -> Vec<Ranked<'a>> {
    let labelled = recalled.into_iter().map(|recalled| {
        let label = one_line(&recalled.entry.statement);
        (recalled, label)
    });

    let Some(fuzzy) = Fuzzy::new(query) else {
        return labelled
            .map(|(recalled, label)| Ranked {
                recalled,
                label,
                ranges: Vec::new(),
            })
            .collect();
    };

    let mut hits: Vec<(i64, Ranked<'a>)> = labelled
        .filter_map(|(recalled, label)| {
            let hit = fuzzy.find(&label)?;
            Some((
                hit.score,
                Ranked {
                    recalled,
                    label,
                    ranges: hit.ranges,
                },
            ))
        })
        .collect();

    // Stable: equal scores keep recall order, which is newest first.
    hits.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    hits.into_iter().map(|(_, ranked)| ranked).collect()
}

/// One row per statement.
#[must_use]
pub fn one_line(statement: &str) -> String {
    statement.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Folded as SQL whatever the engine: both languages fold unquoted names and
/// keywords alike, so one statement is one row across engines too.
fn folded(statement: &str) -> String {
    fold(Engine::Postgres, statement)
}

fn same(left: &str, right: &str) -> bool {
    folded(left) == folded(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limit(rows: u32) -> HistoryLimit {
        HistoryLimit::try_new(rows).expect("in range")
    }

    fn name(name: &str) -> ConnectionName {
        ConnectionName::try_new(name).expect("a valid name")
    }

    fn at(statement: &str, seconds: u64) -> Entry {
        Entry {
            at: Some(Stamp::from_unix(seconds)),
            ..Entry::bare(statement, Engine::Postgres)
        }
    }

    fn on(engine: Engine, statement: &str, seconds: u64) -> Entry {
        Entry {
            engine,
            ..at(statement, seconds)
        }
    }

    fn statements(history: &History) -> Vec<&str> {
        history
            .entries()
            .iter()
            .map(|entry| entry.statement.as_str())
            .collect()
    }

    fn recorded(statements: &[&str], rows: u32) -> History {
        let mut history = History::default();

        for statement in statements {
            history.record(Entry::bare(statement, Engine::Postgres), limit(rows));
        }

        history
    }

    #[test]
    fn the_newest_statement_comes_first() {
        let history = recorded(&["SELECT 1;", "SELECT 2;"], 10);

        assert_eq!(statements(&history), ["SELECT 2;", "SELECT 1;"]);
    }

    #[test]
    fn a_statement_asked_again_after_another_is_kept_again() {
        let history = recorded(&["SELECT 1;", "SELECT 2;", "SELECT 1;"], 10);

        assert_eq!(
            statements(&history),
            ["SELECT 1;", "SELECT 2;", "SELECT 1;"]
        );
    }

    #[test]
    fn a_repeat_differing_in_keyword_or_name_case_replaces_the_newest() {
        let history = recorded(
            &["SELECT * FROM Shop.Orders;", "select * from shop.orders;"],
            10,
        );

        assert_eq!(statements(&history), ["select * from shop.orders;"]);
    }

    #[test]
    fn a_repeat_of_the_newest_moves_its_time() {
        let mut history = History::default();
        history.record(at("SELECT 1;", 100), limit(10));
        history.record(at("SELECT 1;", 200), limit(10));

        assert_eq!(history.entries(), [at("SELECT 1;", 200)]);
    }

    #[test]
    fn case_inside_a_literal_or_a_quoted_name_is_a_different_statement() {
        let history = recorded(
            &[
                "SELECT * FROM t WHERE a = 'X';",
                "SELECT * FROM t WHERE a = 'x';",
                "SELECT * FROM \"T\";",
                "SELECT * FROM \"t\";",
            ],
            10,
        );

        assert_eq!(history.entries().len(), 4);
    }

    #[test]
    fn surrounding_whitespace_does_not_make_a_statement_new() {
        let history = recorded(&["SELECT 1;", "  SELECT 1;\n"], 10);

        assert_eq!(statements(&history), ["SELECT 1;"]);
    }

    #[test]
    fn a_blank_statement_is_not_an_entry() {
        let history = recorded(&[" \n\t"], 10);

        assert!(history.entries().is_empty());
    }

    #[test]
    fn the_oldest_entries_fall_off_past_the_limit() {
        let history = recorded(&["SELECT 1;", "SELECT 2;", "SELECT 3;", "SELECT 4;"], 3);

        assert_eq!(
            statements(&history),
            ["SELECT 4;", "SELECT 3;", "SELECT 2;"]
        );
    }

    #[test]
    fn lowering_the_limit_trims_on_the_next_record() {
        let mut history = recorded(&["SELECT 1;", "SELECT 2;", "SELECT 3;", "SELECT 4;"], 10);
        history.record(Entry::bare("SELECT 5;", Engine::Postgres), limit(2));

        assert_eq!(statements(&history), ["SELECT 5;", "SELECT 4;"]);
    }

    #[test]
    fn each_connection_keeps_its_own() {
        let mut histories = Histories::default();
        histories.record(&name("dev"), at("SELECT 1;", 1), limit(10));
        histories.record(&name("prod"), at("SELECT 2;", 2), limit(10));

        assert_eq!(histories.of(&name("dev")), [at("SELECT 1;", 1)]);
        assert_eq!(histories.of(&name("prod")), [at("SELECT 2;", 2)]);
        assert!(histories.of(&name("stage")).is_empty());
    }

    #[test]
    fn recording_says_whether_anything_changed() {
        let mut history = History::default();
        let mut record = |entry| history.record(entry, limit(10));

        assert!(record(at("SELECT 1;", 1)));
        assert!(!record(at("SELECT 1;", 1)), "the same run");
        assert!(record(at("SELECT 1;", 2)), "a later run");
        assert!(record(at("SELECT 2;", 3)));
        assert!(record(at("SELECT 1;", 4)), "not the newest");
        assert!(record(at("select 1;", 4)), "case replaced");
        assert!(!record(at("  ", 5)));
    }

    #[test]
    fn restoring_keeps_a_stores_order_and_applies_the_limit() {
        let mut histories = Histories::default();
        let stored = ["SELECT 3;", "SELECT 2;", "select 2;", "SELECT 1;"]
            .map(|statement| Entry::bare(statement, Engine::Postgres))
            .to_vec();

        histories.restore(&name("dev"), stored, limit(2));

        assert_eq!(
            histories
                .of(&name("dev"))
                .iter()
                .map(|entry| entry.statement.as_str())
                .collect::<Vec<_>>(),
            ["SELECT 3;", "SELECT 2;"]
        );
    }

    #[test]
    fn a_statement_over_several_lines_is_one_row() {
        assert_eq!(
            one_line("SELECT *\n  FROM shop.orders\n  WHERE id = 1;"),
            "SELECT * FROM shop.orders WHERE id = 1;"
        );
    }

    fn layered() -> Histories {
        let mut histories = Histories::default();
        let mut record = |connection: &str, entry| {
            histories.record(&name(connection), entry, limit(10));
        };

        record("staging", on(Engine::Postgres, "SELECT 1;", 10));
        record("prod", on(Engine::Postgres, "SELECT 2;", 20));
        record("events", on(Engine::Cassandra, "SELECT 3;", 30));
        record("staging", on(Engine::Postgres, "SELECT 4;", 40));
        histories
    }

    fn recalled(histories: &Histories, layer: Layer, connection: &str) -> Vec<(String, String)> {
        histories
            .recall(layer, Some(&name(connection)), Engine::Postgres)
            .into_iter()
            .map(|recalled| {
                (
                    recalled.connection.as_str().to_owned(),
                    recalled.entry.statement.clone(),
                )
            })
            .collect()
    }

    fn pairs(of: &[(&str, &str)]) -> Vec<(String, String)> {
        of.iter()
            .map(|(connection, statement)| ((*connection).to_owned(), (*statement).to_owned()))
            .collect()
    }

    #[test]
    fn the_connection_layer_is_that_connections_own() {
        assert_eq!(
            recalled(&layered(), Layer::Connection, "staging"),
            pairs(&[("staging", "SELECT 4;"), ("staging", "SELECT 1;")])
        );
    }

    #[test]
    fn the_engine_layer_interleaves_every_connection_on_the_engine_by_time() {
        assert_eq!(
            recalled(&layered(), Layer::Engine, "staging"),
            pairs(&[
                ("staging", "SELECT 4;"),
                ("prod", "SELECT 2;"),
                ("staging", "SELECT 1;"),
            ])
        );
    }

    #[test]
    fn the_last_layer_is_everything() {
        assert_eq!(
            recalled(&layered(), Layer::All, "staging"),
            pairs(&[
                ("staging", "SELECT 4;"),
                ("events", "SELECT 3;"),
                ("prod", "SELECT 2;"),
                ("staging", "SELECT 1;"),
            ])
        );
    }

    #[test]
    fn a_statement_asked_on_several_connections_is_recalled_once_as_its_newest_run() {
        let mut histories = layered();
        histories.record(
            &name("prod"),
            on(Engine::Postgres, "select 1;", 50),
            limit(10),
        );

        assert_eq!(
            recalled(&histories, Layer::Engine, "staging"),
            pairs(&[
                ("prod", "select 1;"),
                ("staging", "SELECT 4;"),
                ("prod", "SELECT 2;"),
            ])
        );
    }

    #[test]
    fn a_statement_asked_twice_on_one_connection_is_recalled_once() {
        let histories = {
            let mut histories = Histories::default();
            for (statement, seconds) in [("SELECT 1;", 1), ("SELECT 2;", 2), ("SELECT 1;", 3)] {
                histories.record(&name("dev"), at(statement, seconds), limit(10));
            }
            histories
        };

        assert_eq!(
            recalled(&histories, Layer::Connection, "dev"),
            pairs(&[("dev", "SELECT 1;"), ("dev", "SELECT 2;")])
        );
    }

    #[test]
    fn an_entry_without_a_time_sorts_after_those_with_one() {
        let mut histories = Histories::default();
        histories.restore(
            &name("old"),
            vec![Entry::bare("SELECT 'old';", Engine::Postgres)],
            limit(10),
        );
        histories.record(&name("new"), at("SELECT 'new';", 1), limit(10));

        assert_eq!(
            recalled(&histories, Layer::All, "new"),
            pairs(&[("new", "SELECT 'new';"), ("old", "SELECT 'old';")])
        );
    }

    #[test]
    fn without_a_connection_the_connection_layer_is_empty() {
        assert!(
            layered()
                .recall(Layer::Connection, None, Engine::Postgres)
                .is_empty()
        );
    }

    fn ranked_labels(histories: &Histories, query: &str) -> Vec<String> {
        ranked(histories.recall(Layer::All, None, Engine::Postgres), query)
            .into_iter()
            .map(|ranked| ranked.label)
            .collect()
    }

    #[test]
    fn without_a_query_everything_is_listed_newest_first() {
        assert_eq!(
            ranked_labels(&layered(), ""),
            ["SELECT 4;", "SELECT 3;", "SELECT 2;", "SELECT 1;"]
        );
    }

    #[test]
    fn a_query_keeps_what_it_matches_best_first_then_newest() {
        let mut histories = Histories::default();
        for (statement, seconds) in [
            ("SELECT * FROM orders;", 1),
            ("SELECT o, r, d FROM users;", 2),
            ("DELETE FROM orders;", 3),
            ("SELECT 1;", 4),
        ] {
            histories.record(&name("dev"), at(statement, seconds), limit(10));
        }

        assert_eq!(
            ranked_labels(&histories, "ord"),
            [
                "DELETE FROM orders;",
                "SELECT * FROM orders;",
                "SELECT o, r, d FROM users;"
            ]
        );
    }

    #[test]
    fn a_ranked_row_is_one_line_with_the_hits_in_it() {
        let mut histories = Histories::default();
        histories.record(&name("dev"), at("SELECT *\n  FROM orders;", 1), limit(10));

        let rows = ranked(
            histories.recall(Layer::All, None, Engine::Postgres),
            "orders",
        );

        assert_eq!(rows[0].label, "SELECT * FROM orders;");
        assert_eq!(&rows[0].label[rows[0].ranges[0].clone()], "orders");
    }

    #[test]
    fn layers_cycle_back_to_the_narrowest() {
        assert_eq!(
            Layer::ALL.map(Layer::next),
            [Layer::Engine, Layer::All, Layer::Connection]
        );
    }
}
