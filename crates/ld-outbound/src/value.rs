//! Driver values into our own. scylla's `CqlValue` has no `Display`, so this is
//! the rendering the domain's text values carry. A blob arrives as the `0x…`
//! cqlsh prints, so it reads, edits and spells back as CQL writes it; a
//! `decimal` or `varint` as its exact digits.

use chrono::DateTime;
use ld_domain::model::Value;
use scylla::value::{CqlDate, CqlDuration, CqlTime, CqlTimestamp, CqlValue as DriverValue};

const NANOS_PER_HOUR: i64 = 3_600_000_000_000;
const NANOS_PER_MINUTE: i64 = 60_000_000_000;
const NANOS_PER_SECOND: i64 = 1_000_000_000;
const NANOS_PER_MILLI: i64 = 1_000_000;
const NANOS_PER_MICRO: i64 = 1_000;

const MONTHS_PER_YEAR: i32 = 12;

pub(crate) fn to_value(value: Option<&DriverValue>) -> Value {
    let Some(value) = value else {
        return Value::Null;
    };

    match value {
        DriverValue::Boolean(value) => Value::Boolean(*value),

        DriverValue::TinyInt(value) => Value::Int(i64::from(*value)),
        DriverValue::SmallInt(value) => Value::Int(i64::from(*value)),
        DriverValue::Int(value) => Value::Int(i64::from(*value)),
        DriverValue::BigInt(value) => Value::Int(*value),
        DriverValue::Counter(counter) => Value::Int(counter.0),

        DriverValue::Float(value) => Value::Float(f64::from(*value)),
        DriverValue::Double(value) => Value::Float(*value),

        DriverValue::Ascii(value) | DriverValue::Text(value) => Value::Text(value.clone()),
        // Not null: a legacy zero-length value in a type with no empty form.
        DriverValue::Empty => Value::Text(String::new()),

        DriverValue::Blob(bytes) => Value::Text(hex(bytes)),
        DriverValue::Decimal(decimal) => {
            Value::Numeric(bigdecimal::BigDecimal::from(decimal.clone()).to_string())
        }
        DriverValue::Varint(varint) => {
            Value::Numeric(num_bigint::BigInt::from(varint.clone()).to_string())
        }

        DriverValue::Uuid(value) => Value::Uuid(value.to_string()),
        DriverValue::Timeuuid(value) => Value::Uuid(value.to_string()),

        DriverValue::List(values) | DriverValue::Vector(values) => {
            Value::List(values.iter().map(|v| to_value(Some(v))).collect())
        }
        DriverValue::Set(values) => Value::Set(values.iter().map(|v| to_value(Some(v))).collect()),
        DriverValue::Map(entries) => map(entries),

        other => Value::Text(render(other)),
    }
}

/// Keys of any type keep their own value: rendered to text, two could
/// collide.
fn map(entries: &[(DriverValue, DriverValue)]) -> Value {
    Value::Map(
        entries
            .iter()
            .map(|(key, value)| (to_value(Some(key)), to_value(Some(value))))
            .collect(),
    )
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes.iter().fold("0x".to_owned(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

fn render(value: &DriverValue) -> String {
    match value {
        DriverValue::Inet(address) => address.to_string(),
        DriverValue::Date(date) => date_text(*date),
        DriverValue::Time(time) => time_text(*time),
        DriverValue::Timestamp(timestamp) => timestamp_text(*timestamp),
        DriverValue::Duration(duration) => duration_text(*duration),
        DriverValue::Tuple(values) => {
            let rendered: Vec<String> = values
                .iter()
                .map(|value| match value {
                    Some(value) => flat(value),
                    None => "null".to_owned(),
                })
                .collect();
            format!("({})", rendered.join(", "))
        }
        DriverValue::UserDefinedType { fields, .. } => {
            let rendered: Vec<String> = fields
                .iter()
                .map(|(name, value)| match value {
                    Some(value) => format!("{name}: {}", flat(value)),
                    None => format!("{name}: null"),
                })
                .collect();
            format!("{{{}}}", rendered.join(", "))
        }
        DriverValue::Map(entries) => {
            let rendered: Vec<String> = entries
                .iter()
                .map(|(key, value)| format!("{}: {}", flat(key), flat(value)))
                .collect();
            format!("{{{}}}", rendered.join(", "))
        }
        // `#[non_exhaustive]`: an unseen variant still gets a name.
        other => format!("{other:?}"),
    }
}

/// Through our own enum, so nesting renders as a column would.
fn flat(value: &DriverValue) -> String {
    ld_domain::cell::text(&to_value(Some(value)))
}

/// UTC, the only zone a Cassandra timestamp has.
fn timestamp_text(CqlTimestamp(millis): CqlTimestamp) -> String {
    DateTime::from_timestamp_millis(millis).map_or_else(
        || millis.to_string(),
        |at| at.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
    )
}

/// Days since `-5877641-06-23`, putting the epoch at 2^31.
fn date_text(CqlDate(days): CqlDate) -> String {
    let since_epoch = i64::from(days) - (1 << 31);

    since_epoch
        .checked_mul(86_400)
        .and_then(DateTime::from_timestamp_secs)
        .map_or_else(
            || since_epoch.to_string(),
            |at| at.format("%Y-%m-%d").to_string(),
        )
}

/// As `cqlsh` prints a `time`.
fn time_text(CqlTime(nanos): CqlTime) -> String {
    let hours = nanos / NANOS_PER_HOUR;
    let minutes = (nanos % NANOS_PER_HOUR) / NANOS_PER_MINUTE;
    let seconds = (nanos % NANOS_PER_MINUTE) / NANOS_PER_SECOND;
    let rest = nanos % NANOS_PER_SECOND;

    format!("{hours:02}:{minutes:02}:{seconds:02}.{rest:09}")
}

/// Months, days and nanoseconds stay separate: their lengths are not fixed.
fn duration_text(
    CqlDuration {
        months,
        days,
        nanoseconds,
    }: CqlDuration,
) -> String {
    let negative = months < 0 || days < 0 || nanoseconds < 0;
    let (months, days, nanos) = (
        months.unsigned_abs(),
        days.unsigned_abs(),
        nanoseconds.unsigned_abs(),
    );

    let mut out = String::new();
    if negative {
        out.push('-');
    }

    let mut push = |value: u64, unit: &str| {
        if value > 0 {
            out.push_str(&value.to_string());
            out.push_str(unit);
        }
    };

    push(u64::from(months / MONTHS_PER_YEAR.unsigned_abs()), "y");
    push(u64::from(months % MONTHS_PER_YEAR.unsigned_abs()), "mo");
    push(u64::from(days), "d");
    push(nanos / NANOS_PER_HOUR.unsigned_abs(), "h");
    push(
        (nanos % NANOS_PER_HOUR.unsigned_abs()) / NANOS_PER_MINUTE.unsigned_abs(),
        "m",
    );
    push(
        (nanos % NANOS_PER_MINUTE.unsigned_abs()) / NANOS_PER_SECOND.unsigned_abs(),
        "s",
    );
    push(
        (nanos % NANOS_PER_SECOND.unsigned_abs()) / NANOS_PER_MILLI.unsigned_abs(),
        "ms",
    );
    push(
        (nanos % NANOS_PER_MILLI.unsigned_abs()) / NANOS_PER_MICRO.unsigned_abs(),
        "us",
    );
    push(nanos % NANOS_PER_MICRO.unsigned_abs(), "ns");

    if out.is_empty() || out == "-" {
        return "0s".to_owned();
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use scylla::value::{Counter, CqlDecimal, CqlVarint};
    use std::net::{IpAddr, Ipv4Addr};

    fn some(value: &DriverValue) -> Value {
        to_value(Some(value))
    }

    #[test]
    fn a_missing_column_is_a_null_rather_than_an_empty_string() {
        assert_eq!(to_value(None), Value::Null);
        assert_eq!(
            some(&DriverValue::Empty),
            Value::Text(String::new()),
            "an empty value is distinct from a null (a Cassandra legacy)"
        );
    }

    #[test]
    fn every_width_of_integer_lands_in_one_variant() {
        assert_eq!(some(&DriverValue::TinyInt(-1)), Value::Int(-1));
        assert_eq!(some(&DriverValue::SmallInt(-1)), Value::Int(-1));
        assert_eq!(some(&DriverValue::Int(-1)), Value::Int(-1));
        assert_eq!(some(&DriverValue::BigInt(-1)), Value::Int(-1));
        assert_eq!(some(&DriverValue::Counter(Counter(7))), Value::Int(7));
    }

    #[test]
    fn both_floating_point_widths_land_in_one_variant() {
        assert_eq!(some(&DriverValue::Float(0.5)), Value::Float(0.5));
        assert_eq!(some(&DriverValue::Double(0.5)), Value::Float(0.5));
    }

    #[test]
    fn a_vector_is_a_list_and_a_set_stays_a_set() {
        let values = vec![DriverValue::Int(1)];

        assert_eq!(
            some(&DriverValue::Vector(values.clone())),
            Value::List(vec![Value::Int(1)])
        );
        assert_eq!(
            some(&DriverValue::Set(values)),
            Value::Set(vec![Value::Int(1)])
        );
    }

    #[test]
    fn a_map_keeps_its_keys_typed_and_its_order() {
        let entries = vec![
            (DriverValue::Int(2), DriverValue::Text("two".to_owned())),
            (DriverValue::Int(1), DriverValue::Text("one".to_owned())),
        ];

        assert_eq!(
            some(&DriverValue::Map(entries)),
            Value::Map(vec![
                (Value::Int(2), Value::Text("two".to_owned())),
                (Value::Int(1), Value::Text("one".to_owned())),
            ])
        );
    }

    #[test]
    fn a_blob_reads_as_cqlsh_prints_it() {
        assert_eq!(
            some(&DriverValue::Blob(vec![0x00, 0x0f, 0xff])),
            Value::Text("0x000fff".to_owned())
        );
    }

    #[test]
    fn a_timestamp_is_readable_rather_than_a_count_of_milliseconds() {
        assert_eq!(
            some(&DriverValue::Timestamp(CqlTimestamp(1_700_000_000_000))),
            Value::Text("2023-11-14T22:13:20.000Z".to_owned())
        );
    }

    #[test]
    fn a_date_counts_from_the_epoch_at_two_to_the_thirty_first() {
        assert_eq!(
            some(&DriverValue::Date(CqlDate(1 << 31))),
            Value::Text("1970-01-01".to_owned())
        );
        assert_eq!(
            some(&DriverValue::Date(CqlDate((1 << 31) + 1))),
            Value::Text("1970-01-02".to_owned())
        );
    }

    #[test]
    fn a_time_is_printed_to_the_nanosecond() {
        assert_eq!(
            some(&DriverValue::Time(CqlTime(
                13 * NANOS_PER_HOUR + 45 * NANOS_PER_MINUTE + 7 * NANOS_PER_SECOND + 123
            ))),
            Value::Text("13:45:07.000000123".to_owned())
        );
    }

    #[test]
    fn a_duration_keeps_months_days_and_nanoseconds_apart() {
        // A month is not thirty days, nor a day always twenty-four hours.
        assert_eq!(
            duration_text(CqlDuration {
                months: 14,
                days: 3,
                nanoseconds: 4 * NANOS_PER_HOUR + 5 * NANOS_PER_MINUTE,
            }),
            "1y2mo3d4h5m"
        );
    }

    #[test]
    fn a_negative_duration_carries_one_sign_for_the_whole_value() {
        assert_eq!(
            duration_text(CqlDuration {
                months: -1,
                days: -2,
                nanoseconds: 0,
            }),
            "-1mo2d"
        );
    }

    #[test]
    fn a_zero_duration_still_says_something() {
        assert_eq!(
            duration_text(CqlDuration {
                months: 0,
                days: 0,
                nanoseconds: 0,
            }),
            "0s"
        );
    }

    #[test]
    fn sub_second_duration_components_each_get_their_unit() {
        assert_eq!(
            duration_text(CqlDuration {
                months: 0,
                days: 0,
                nanoseconds: NANOS_PER_MILLI + 2 * NANOS_PER_MICRO + 3,
            }),
            "1ms2us3ns"
        );
    }

    #[test]
    fn arbitrary_precision_numbers_survive_the_crossing() {
        let varint = CqlVarint::from_signed_bytes_be(vec![0x01, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(
            some(&DriverValue::Varint(varint)),
            Value::Numeric("1099511627776".to_owned())
        );

        let decimal = CqlDecimal::from_signed_be_bytes_and_exponent(vec![0x01, 0x2d], 2);
        assert_eq!(
            some(&DriverValue::Decimal(decimal)),
            Value::Numeric("3.01".to_owned())
        );
    }

    #[test]
    fn an_address_renders_as_an_address() {
        assert_eq!(
            some(&DriverValue::Inet(IpAddr::V4(Ipv4Addr::LOCALHOST))),
            Value::Text("127.0.0.1".to_owned())
        );
    }

    #[test]
    fn a_tuple_keeps_its_arity_including_the_nulls() {
        assert_eq!(
            some(&DriverValue::Tuple(vec![
                Some(DriverValue::Int(1)),
                None,
                Some(DriverValue::Text("x".to_owned())),
            ])),
            Value::Text("(1, null, x)".to_owned())
        );
    }

    #[test]
    fn a_user_defined_type_names_its_fields() {
        assert_eq!(
            some(&DriverValue::UserDefinedType {
                keyspace: "typezoo".to_owned(),
                name: "address".to_owned(),
                fields: vec![
                    (
                        "street".to_owned(),
                        Some(DriverValue::Text("Main".to_owned()))
                    ),
                    ("number".to_owned(), None),
                ],
            }),
            Value::Text("{street: Main, number: null}".to_owned())
        );
    }

    #[test]
    fn a_collection_nested_in_a_tuple_renders_the_way_a_column_of_it_would() {
        assert_eq!(
            some(&DriverValue::Tuple(vec![Some(DriverValue::List(vec![
                DriverValue::Int(1),
                DriverValue::Int(2),
            ]))])),
            Value::Text("([1, 2])".to_owned())
        );
    }
}
