//! One Postgres value as JSON. The simple query protocol sends every value
//! as the server's own text, so a type needs a rule here only where JSON has
//! a better form: numbers, booleans, bytes and the two timestamps. The rest
//! (text, json, uuid, arrays, ranges, intervals, enums) is the text psql
//! would print.

use serde_json::Value;
use tokio_postgres::types::{Kind as TypeKind, Type};

use crate::db;

/// The type as `pg_type` names it, an array as its member's with `[]`
/// (`int4[]`, where the catalog says `_int4`).
pub(super) fn type_name(column: &Type) -> String {
    match column.kind() {
        TypeKind::Array(member) => format!("{}[]", member.name()),
        _ => column.name().to_owned(),
    }
}

pub(super) fn cell(column: &Type, text: Option<&str>) -> Value {
    let Some(text) = text else {
        return Value::Null;
    };
    let string = || Value::String(text.to_owned());
    match *column {
        Type::BOOL => Value::Bool(text == "t"),
        Type::INT2 | Type::INT4 | Type::INT8 | Type::OID => {
            text.parse().map_or_else(|_| string(), db::int)
        }
        // NaN and the infinities stay text, as `decimal` keeps any non-integer.
        Type::NUMERIC => db::decimal(text.to_owned()),
        Type::FLOAT4 | Type::FLOAT8 => text.parse().map_or_else(|_| string(), db::float),
        // bytea_output = hex, the default since 9.0; escape output stays text.
        Type::BYTEA => text
            .strip_prefix("\\x")
            .map_or_else(string, |hex| Value::String(format!("0x{hex}"))),
        Type::TIMESTAMP => Value::String(timestamp(text)),
        Type::TIMESTAMPTZ => Value::String(offset(timestamp(text))),
        _ => string(),
    }
}

/// ISO output (`DateStyle = ISO`, the default) as RFC 3339:
/// `2024-05-17 13:45:30.5` is `2024-05-17T13:45:30.5`. `infinity` and a
/// BC date are left as the server wrote them.
fn timestamp(text: &str) -> String {
    let iso = text.len() >= 19
        && text.as_bytes()[10] == b' '
        && text.as_bytes()[..4].iter().all(u8::is_ascii_digit)
        && !text.ends_with(" BC");
    if iso {
        format!("{}T{}", &text[..10], &text[11..])
    } else {
        text.to_owned()
    }
}

/// A `timestamptz` carries the session's offset as `+02` or `+05:30`;
/// RFC 3339 wants the minutes too.
fn offset(text: String) -> String {
    let Some(at) = text.get(19..).and_then(|tail| tail.rfind(['+', '-'])) else {
        return text;
    };
    if text.len() - (19 + at) == 3 {
        format!("{text}:00")
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn numbers_booleans_and_bytes_become_json_and_everything_else_stays_the_servers_text() {
        let cases = [
            (Type::INT4, "-7", json!(-7)),
            (
                Type::INT8,
                "9223372036854775807",
                json!("9223372036854775807"),
            ),
            (Type::NUMERIC, "12345.6789", json!("12345.6789")),
            (Type::NUMERIC, "42", json!(42)),
            (Type::NUMERIC, "NaN", json!("NaN")),
            (Type::FLOAT8, "12345678901.234", json!(12_345_678_901.234)),
            (Type::FLOAT4, "0.1", json!(0.1)),
            (Type::FLOAT8, "Infinity", Value::Null),
            (Type::BOOL, "t", json!(true)),
            (Type::BOOL, "f", json!(false)),
            (Type::BYTEA, "\\x0102ff", json!("0x0102ff")),
            (
                Type::UUID,
                "6f9619ff-8b86-d011-b42d-00c04fc964ff",
                json!("6f9619ff-8b86-d011-b42d-00c04fc964ff"),
            ),
            (Type::JSONB, "{\"a\": 1}", json!("{\"a\": 1}")),
            (Type::INT4_ARRAY, "{1,2,3}", json!("{1,2,3}")),
            (Type::INTERVAL, "1 day 02:03:04", json!("1 day 02:03:04")),
        ];
        for (column, text, want) in cases {
            assert_eq!(cell(&column, Some(text)), want, "{column} {text}");
        }
        assert_eq!(cell(&Type::INT4, None), Value::Null);
    }

    #[test]
    fn timestamps_are_rfc_3339_with_the_offset_written_out() {
        assert_eq!(
            cell(&Type::TIMESTAMP, Some("2024-05-17 13:45:30.123456")),
            json!("2024-05-17T13:45:30.123456")
        );
        assert_eq!(
            cell(&Type::TIMESTAMPTZ, Some("2024-05-17 11:45:30+00")),
            json!("2024-05-17T11:45:30+00:00")
        );
        assert_eq!(
            cell(&Type::TIMESTAMPTZ, Some("2024-05-17 17:15:30.5+05:30")),
            json!("2024-05-17T17:15:30.5+05:30")
        );
        assert_eq!(
            cell(&Type::TIMESTAMPTZ, Some("2024-05-17 08:45:30-03")),
            json!("2024-05-17T08:45:30-03:00")
        );
        for text in ["infinity", "-infinity", "0044-03-15 10:00:00 BC"] {
            assert_eq!(cell(&Type::TIMESTAMP, Some(text)), json!(text));
        }
        assert_eq!(cell(&Type::DATE, Some("2024-05-17")), json!("2024-05-17"));
    }

    #[test]
    fn an_array_type_is_named_by_its_member() {
        assert_eq!(type_name(&Type::INT4_ARRAY), "int4[]");
        assert_eq!(type_name(&Type::TIMESTAMPTZ), "timestamptz");
    }
}
