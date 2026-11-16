//! Spelling a value the server returned back into a literal that addresses
//! it.
//!
//! Postgres: one form fits every type, the text Postgres printed, quoted and
//! cast to the declared type (`'8f14…'::uuid`, `'\x0a'::bytea`,
//! `'{1,2}'::integer[]`), so Postgres parses it back with its own input
//! function. Integers and booleans are written bare.
//!
//! Cassandra has no cast from text, so each type is spelled as CQL writes it:
//! strings and dates quoted, numbers, uuids and blobs bare, collections
//! bracketed.

use crate::engine::Engine;
use crate::model::Value;

/// A value that addresses a row, for a primary key's `WHERE`. `None` where a
/// re-spelling may not compare equal: a float printed and read back can
/// differ, and no key holds `NULL`. Cassandra also refuses `varint` and
/// `decimal`, whose rendering this program did not produce, and collections,
/// which cannot be keys.
#[must_use]
pub fn key(engine: Engine, data_type: &str, value: &Value) -> Option<String> {
    match (engine, value) {
        (_, Value::Null | Value::Float(_))
        | (Engine::Cassandra, Value::Numeric(_) | Value::List(_) | Value::Set(_) | Value::Map(_)) => {
            None
        }
        (_, other) => Some(of(engine, data_type, other)),
    }
}

/// Any value, for a `SET` or a `VALUES` list.
#[must_use]
pub fn of(engine: Engine, data_type: &str, value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_owned(),
        Value::Int(number) => number.to_string(),
        Value::Boolean(flag) => flag.to_string(),
        other => match engine {
            Engine::Postgres => cast(&crate::cell::text(other), data_type),
            Engine::Cassandra => cql(data_type, other),
        },
    }
}

/// `standard_conforming_strings` is on by default since 9.1, so a backslash
/// is literal and only the quote needs doubling.
#[must_use]
pub fn cast(text: &str, data_type: &str) -> String {
    format!("{}::{data_type}", quoted(text))
}

#[must_use]
pub fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// Lower case, `frozen<…>` removed: `system_schema` reports the declared case.
#[must_use]
pub fn normalised(cql_type: &str) -> String {
    let trimmed = cql_type.trim().to_ascii_lowercase();

    trimmed
        .strip_prefix("frozen<")
        .and_then(|inner| inner.strip_suffix('>'))
        .map_or(trimmed.clone(), |inner| inner.trim().to_owned())
}

/// The element type of `list<int>` or `set<text>`, the key and value types
/// of `map<text, int>`; empty where the type says nothing.
fn elements(cql_type: &str) -> (String, String) {
    let normal = normalised(cql_type);
    let inner = ["list<", "set<", "map<"]
        .iter()
        .find_map(|open| normal.strip_prefix(open)?.strip_suffix('>'))
        .unwrap_or_default();

    // The first comma outside brackets splits a map's two types.
    let mut depth = 0_usize;
    for (at, character) in inner.char_indices() {
        match character {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                return (
                    inner[..at].trim().to_owned(),
                    inner[at + 1..].trim().to_owned(),
                );
            }
            _ => {}
        }
    }
    (inner.trim().to_owned(), inner.trim().to_owned())
}

fn cql(data_type: &str, value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_owned(),
        Value::Int(number) => number.to_string(),
        Value::Boolean(flag) => flag.to_string(),
        // CQL's own spellings of what Rust spells `NaN` and `inf`.
        Value::Float(number) => crate::cell::text(&Value::Float(*number)),
        Value::Numeric(digits) | Value::Uuid(digits) => digits.clone(),
        Value::Blob(_) => crate::cell::text(value),
        // A blob arrives as the `0x…` cqlsh prints, a wide number as its
        // digits: both are bare in CQL.
        Value::Text(text) => match normalised(data_type).as_str() {
            "blob" | "varint" | "decimal" | "counter" => text.clone(),
            _ => quoted(text),
        },
        Value::Json(text) | Value::Array(text) => quoted(text),
        Value::List(values) => {
            let (element, _) = elements(data_type);
            sequence(values.iter().map(|value| cql(&element, value)), '[', ']')
        }
        Value::Set(values) => {
            let (element, _) = elements(data_type);
            sequence(values.iter().map(|value| cql(&element, value)), '{', '}')
        }
        Value::Map(entries) => {
            let (key, element) = elements(data_type);
            sequence(
                entries
                    .iter()
                    .map(|(k, v)| format!("{}: {}", cql(&key, k), cql(&element, v))),
                '{',
                '}',
            )
        }
    }
}

fn sequence(items: impl Iterator<Item = String>, open: char, close: char) -> String {
    format!("{open}{}{close}", items.collect::<Vec<_>>().join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_quoted_cast_and_an_embedded_quote_doubled() {
        assert_eq!(
            of(Engine::Postgres, "text", &Value::Text("o'hara".to_owned())),
            "'o''hara'::text",
            "a name holding a quote must not end the literal"
        );
    }

    #[test]
    fn every_other_type_is_its_text_cast_to_the_declared_type() {
        assert_eq!(
            of(
                Engine::Postgres,
                "uuid",
                &Value::Uuid("8f14e45f-0000".to_owned())
            ),
            "'8f14e45f-0000'::uuid"
        );
        assert_eq!(
            of(Engine::Postgres, "bytea", &Value::Blob(vec![0xde, 0xad])),
            "'\\xdead'::bytea"
        );
        assert_eq!(
            of(
                Engine::Postgres,
                "integer[]",
                &Value::Array("{1,2}".to_owned())
            ),
            "'{1,2}'::integer[]"
        );
        assert_eq!(
            of(
                Engine::Postgres,
                "numeric(10,2)",
                &Value::Numeric("1.50".to_owned())
            ),
            "'1.50'::numeric(10,2)",
            "the digits as Postgres printed them"
        );
        assert_eq!(
            of(
                Engine::Postgres,
                "timestamp with time zone",
                &Value::Text("2026-01-01 10:00:00+00".to_owned())
            ),
            "'2026-01-01 10:00:00+00'::timestamp with time zone"
        );
    }

    #[test]
    fn integers_and_booleans_are_bare_and_null_is_null() {
        assert_eq!(of(Engine::Postgres, "bigint", &Value::Int(-7)), "-7");
        assert_eq!(
            of(Engine::Postgres, "boolean", &Value::Boolean(true)),
            "true"
        );
        assert_eq!(of(Engine::Postgres, "text", &Value::Null), "NULL");
    }

    #[test]
    fn a_key_refuses_what_may_not_compare_equal_when_read_back() {
        assert_eq!(
            key(Engine::Postgres, "double precision", &Value::Float(0.1)),
            None
        );
        assert_eq!(key(Engine::Postgres, "integer", &Value::Null), None);
        assert_eq!(
            key(Engine::Postgres, "integer", &Value::Int(3)).as_deref(),
            Some("3")
        );
    }

    fn text(text: &str) -> Value {
        Value::Text(text.to_owned())
    }

    #[test]
    fn cql_quotes_strings_and_dates_and_leaves_numbers_uuids_and_blobs_bare() {
        let cql = |data_type, value: &Value| of(Engine::Cassandra, data_type, value);
        assert_eq!(cql("text", &text("o'hara")), "'o''hara'");
        assert_eq!(
            cql("timestamp", &text("2026-01-01T10:00:00.000Z")),
            "'2026-01-01T10:00:00.000Z'"
        );
        assert_eq!(cql("uuid", &Value::Uuid("8f14e45f".to_owned())), "8f14e45f");
        assert_eq!(cql("blob", &text("0x0aff")), "0x0aff");
        assert_eq!(cql("decimal", &Value::Numeric("1.50".to_owned())), "1.50");
        assert_eq!(cql("double", &Value::Float(f64::NAN)), "NaN");
    }

    #[test]
    fn a_cql_collection_spells_each_element_by_its_declared_type() {
        let cql = |data_type, value: &Value| of(Engine::Cassandra, data_type, value);
        assert_eq!(
            cql("list<text>", &Value::List(vec![text("a"), text("b")])),
            "['a', 'b']"
        );
        assert_eq!(
            cql("frozen<set<int>>", &Value::Set(vec![Value::Int(1)])),
            "{1}"
        );
        assert_eq!(
            cql(
                "map<text, frozen<list<int>>>",
                &Value::Map(vec![(text("k"), Value::List(vec![Value::Int(2)]))])
            ),
            "{'k': [2]}"
        );
    }

    #[test]
    fn a_cql_key_refuses_what_it_did_not_render_itself() {
        let key = |data_type, value: &Value| key(Engine::Cassandra, data_type, value);
        assert_eq!(key("varint", &Value::Numeric("10".to_owned())), None);
        assert_eq!(key("list<int>", &Value::List(Vec::new())), None);
        assert_eq!(key("text", &text("a")).as_deref(), Some("'a'"));
    }
}
