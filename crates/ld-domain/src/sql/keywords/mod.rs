//! Vocabulary each engine reads as grammar, one table set per engine.

mod cassandra;
mod postgres;

use crate::engine::Engine;

/// Upper case and sorted, for bisection and a stable completion order.
#[derive(Debug)]
pub(crate) struct Vocabulary {
    pub keywords: &'static [&'static str],
    /// A subset of `keywords`, so types stay grammar everywhere; this is what
    /// `Expect::Type` offers.
    pub types: &'static [&'static str],
    /// The words that continue or end a column list, and nothing else.
    pub column_words: &'static [&'static str],
    /// What a `WITH` takes. Not keywords: ordinary identifiers elsewhere.
    pub properties: &'static [&'static str],
    /// Keywords that mean values, so drawn as literals.
    pub value_words: &'static [&'static str],
}

const POSTGRES: Vocabulary = Vocabulary {
    keywords: postgres::KEYWORDS,
    types: postgres::TYPES,
    column_words: postgres::COLUMN_WORDS,
    properties: postgres::PROPERTIES,
    value_words: postgres::VALUE_WORDS,
};

const CASSANDRA: Vocabulary = Vocabulary {
    keywords: cassandra::KEYWORDS,
    types: cassandra::TYPES,
    column_words: cassandra::COLUMN_WORDS,
    properties: cassandra::PROPERTIES,
    value_words: cassandra::VALUE_WORDS,
};

pub(crate) const fn of(engine: Engine) -> &'static Vocabulary {
    match engine {
        Engine::Postgres => &POSTGRES,
        Engine::Cassandra => &CASSANDRA,
    }
}

pub(crate) fn is_keyword(engine: Engine, word: &str) -> bool {
    lookup(of(engine).keywords, word)
}

pub(crate) fn is_type(engine: Engine, word: &str) -> bool {
    lookup(of(engine).types, word)
}

pub(crate) fn is_value_word(engine: Engine, word: &str) -> bool {
    lookup(of(engine).value_words, word)
}

fn lookup(table: &[&str], word: &str) -> bool {
    let upper = word.to_ascii_uppercase();

    table.binary_search(&upper.as_str()).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables(engine: Engine) -> [&'static [&'static str]; 5] {
        let words = of(engine);
        [
            words.keywords,
            words.column_words,
            words.value_words,
            words.types,
            words.properties,
        ]
    }

    #[test]
    fn the_tables_are_sorted_so_a_lookup_can_bisect() {
        for engine in Engine::ALL {
            for table in tables(engine) {
                let mut sorted = table.to_vec();
                sorted.sort_unstable();
                assert_eq!(table, sorted, "the table is walked and bisected in order");
            }
        }
    }

    #[test]
    fn a_keyword_is_recognised_in_any_case() {
        assert!(is_keyword(Engine::Postgres, "select"));
        assert!(is_keyword(Engine::Cassandra, "select"));
        assert!(is_keyword(Engine::Postgres, "SELECT"));
        assert!(is_keyword(Engine::Postgres, "SeLeCt"));
    }

    #[test]
    fn a_name_is_not_a_keyword() {
        for engine in Engine::ALL {
            assert!(!is_keyword(engine, "orders"));
            assert!(!is_keyword(engine, ""));
        }
    }

    #[test]
    fn every_type_and_column_word_is_also_grammar() {
        for engine in Engine::ALL {
            let words = of(engine);
            for word in words.types.iter().chain(words.column_words) {
                assert!(
                    is_keyword(engine, word),
                    "{engine:?}: {word} is offered and must still highlight as grammar"
                );
            }
        }
    }

    #[test]
    fn a_property_is_a_name_everywhere_but_its_own_clause() {
        for engine in Engine::ALL {
            for word in of(engine).properties {
                assert!(
                    !is_keyword(engine, word),
                    "{engine:?}: {word} may be a column name, and colouring it as grammar would lie"
                );
            }
        }
    }

    #[test]
    fn the_value_words_are_not_in_the_keyword_table() {
        for engine in Engine::ALL {
            for word in of(engine).value_words {
                assert!(
                    !is_keyword(engine, word),
                    "{engine:?}: {word} is a literal, and one word cannot be both"
                );
                assert!(is_value_word(engine, word));
            }
        }
    }

    #[test]
    fn a_word_one_engine_reserves_may_name_a_column_in_the_other() {
        assert!(is_keyword(Engine::Cassandra, "KEYSPACE"));
        assert!(!is_keyword(Engine::Postgres, "KEYSPACE"));
        assert!(is_keyword(Engine::Postgres, "RETURNING"));
        assert!(!is_keyword(Engine::Cassandra, "RETURNING"));
    }
}
