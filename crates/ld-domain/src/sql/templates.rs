//! Statement shapes, offered whole where a statement begins.

use crate::engine::Engine;

/// Split at the first hole, so the caret cannot point outside the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Template {
    /// Each hole an empty `<>`.
    pub label: &'static str,
    pub head: &'static str,
    pub tail: &'static str,
}

impl Template {
    pub(crate) fn keyword(self) -> &'static str {
        let end = self
            .head
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(self.head.len());
        &self.head[..end]
    }

    pub(crate) fn text(self) -> String {
        format!("{}{}", self.head, self.tail)
    }

    pub(crate) fn caret(self) -> usize {
        self.head.chars().count()
    }
}

/// Data statements first: the panel exists to read rows. No functions,
/// aggregates or views: a skeleton of a code body is worse than an empty line.
/// A transaction or batch is the wrapper alone.
pub(crate) const fn of(engine: Engine) -> &'static [Template] {
    match engine {
        Engine::Postgres => POSTGRES,
        Engine::Cassandra => CASSANDRA,
    }
}

const POSTGRES: &[Template] = &[
    Template {
        label: "SELECT <> FROM <>",
        head: "SELECT ",
        tail: " FROM ;",
    },
    Template {
        label: "INSERT INTO <> (<>) VALUES (<>)",
        head: "INSERT INTO ",
        tail: " () VALUES ();",
    },
    Template {
        label: "UPDATE <> SET <> WHERE <>",
        head: "UPDATE ",
        tail: " SET  WHERE ;",
    },
    Template {
        label: "DELETE FROM <> WHERE <>",
        head: "DELETE FROM ",
        tail: " WHERE ;",
    },
    Template {
        label: "BEGIN; <> COMMIT;",
        head: "BEGIN;\n",
        tail: "\nCOMMIT;",
    },
    Template {
        label: "CREATE SCHEMA <>",
        head: "CREATE SCHEMA ",
        tail: ";",
    },
    Template {
        label: "CREATE TABLE <> (id … PRIMARY KEY, <>)",
        head: "CREATE TABLE ",
        tail: " (id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, );",
    },
    Template {
        label: "ALTER TABLE <> ADD COLUMN <> <>",
        head: "ALTER TABLE ",
        tail: " ADD COLUMN  ;",
    },
    Template {
        label: "CREATE INDEX <> ON <> (<>)",
        head: "CREATE INDEX ",
        tail: " ON  ();",
    },
];

const CASSANDRA: &[Template] = &[
    Template {
        label: "SELECT <> FROM <>",
        head: "SELECT ",
        tail: " FROM ;",
    },
    Template {
        label: "INSERT INTO <> (<>) VALUES (<>)",
        head: "INSERT INTO ",
        tail: " () VALUES ();",
    },
    Template {
        label: "UPDATE <> SET <> WHERE <>",
        head: "UPDATE ",
        tail: " SET  WHERE ;",
    },
    Template {
        label: "DELETE FROM <> WHERE <>",
        head: "DELETE FROM ",
        tail: " WHERE ;",
    },
    Template {
        label: "BEGIN BATCH <> APPLY BATCH",
        head: "BEGIN BATCH\n",
        tail: "\nAPPLY BATCH;",
    },
    Template {
        label: "CREATE KEYSPACE <> WITH replication = {…}",
        head: "CREATE KEYSPACE ",
        tail: " WITH replication = {'class': 'SimpleStrategy', 'replication_factor': 1};",
    },
    Template {
        label: "CREATE TABLE <> (<> PRIMARY KEY)",
        head: "CREATE TABLE ",
        tail: " ( PRIMARY KEY);",
    },
    Template {
        label: "ALTER TABLE <> ADD <> <>",
        head: "ALTER TABLE ",
        tail: " ADD  ;",
    },
    Template {
        label: "CREATE INDEX <> ON <> (<>)",
        head: "CREATE INDEX ",
        tail: " ON  ();",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::keywords::is_keyword;

    fn every() -> impl Iterator<Item = (Engine, &'static Template)> {
        Engine::ALL
            .into_iter()
            .flat_map(|engine| of(engine).iter().map(move |template| (engine, template)))
    }

    #[test]
    fn every_template_opens_with_a_keyword_a_prefix_can_reach() {
        for (engine, template) in every() {
            let keyword = template.keyword();

            assert!(is_keyword(engine, keyword), "{keyword} is not grammar");
            assert!(
                template.label.starts_with(keyword),
                "{:?} is matched on {keyword} and must be seen to be",
                template.label
            );
        }
    }

    #[test]
    fn every_template_terminates_its_statement() {
        for (_, template) in every() {
            assert!(
                template.text().ends_with(';'),
                "{:?} would run into whatever is typed after it",
                template.label
            );
        }
    }

    #[test]
    fn the_caret_lands_inside_the_text_it_was_split_from() {
        for (_, template) in every() {
            assert!(template.caret() <= template.text().chars().count());
        }
    }

    /// A template that typed everything it drew would be a keyword completion.
    #[test]
    fn every_label_marks_a_hole_and_the_caret_lands_in_the_first_one() {
        for (_, template) in every() {
            assert!(
                template.label.contains("<>"),
                "{:?} has nothing to fill",
                template.label
            );
            assert!(
                template.tail.starts_with([' ', '\n', ')', ';']),
                "{:?} would leave the caret mid-word",
                template.label
            );
        }
    }
}
