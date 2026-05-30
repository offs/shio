use crate::error::{Result, ShioError};
use serde::de::DeserializeOwned;

pub(super) fn encode_json<T: serde::Serialize + ?Sized>(
    field: &'static str,
    value: &T,
) -> Result<String> {
    serde_json::to_string(value).map_err(|source| ShioError::DatabaseJson { field, source })
}

pub(super) fn encode_i64(field: &'static str, value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| ShioError::DatabaseValue {
        field,
        value: value.to_string(),
    })
}

pub(super) fn encode_i32(field: &'static str, value: u32) -> Result<i32> {
    i32::try_from(value).map_err(|_| ShioError::DatabaseValue {
        field,
        value: value.to_string(),
    })
}

pub(super) fn decode_required_json<T>(field: &'static str, value: Option<&str>) -> Result<T>
where
    T: DeserializeOwned,
{
    let value = value.ok_or_else(|| ShioError::DatabaseValue {
        field,
        value: "NULL".to_string(),
    })?;
    serde_json::from_str(value).map_err(|source| ShioError::DatabaseJson { field, source })
}

pub(super) fn parse_required_ts(
    field: &'static str,
    value: &str,
) -> Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|source| ShioError::DatabaseTimestamp { field, source })
}

pub(super) fn parse_optional_ts(
    field: &'static str,
    value: Option<&str>,
) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    value.map(|ts| parse_required_ts(field, ts)).transpose()
}

pub(super) fn decode_u64(field: &'static str, value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| ShioError::DatabaseValue {
        field,
        value: value.to_string(),
    })
}

pub(super) fn decode_optional_u64(field: &'static str, value: Option<i64>) -> Result<Option<u64>> {
    value.map(|v| decode_u64(field, v)).transpose()
}

pub(super) fn decode_ratio(field: &'static str, value: f64) -> Result<f32> {
    if value.is_finite() && (0.0..=f64::from(f32::MAX)).contains(&value) {
        Ok(value as f32)
    } else {
        Err(ShioError::DatabaseValue {
            field,
            value: value.to_string(),
        })
    }
}

pub(super) fn decode_u32(field: &'static str, value: i32) -> Result<u32> {
    u32::try_from(value).map_err(|_| ShioError::DatabaseValue {
        field,
        value: value.to_string(),
    })
}

pub(super) fn decode_u8(field: &'static str, value: i32) -> Result<u8> {
    u8::try_from(value).map_err(|_| ShioError::DatabaseValue {
        field,
        value: value.to_string(),
    })
}

pub(super) fn decode_bool_i32(field: &'static str, value: i32) -> Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(ShioError::DatabaseValue {
            field,
            value: value.to_string(),
        }),
    }
}

pub(super) fn decode_bool_i64(field: &'static str, value: i64) -> Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(ShioError::DatabaseValue {
            field,
            value: value.to_string(),
        }),
    }
}

pub(super) fn require_i64(field: &'static str, value: Option<i64>) -> Result<i64> {
    value.ok_or_else(|| ShioError::DatabaseValue {
        field,
        value: "NULL".to_string(),
    })
}

pub(super) fn require_f64(field: &'static str, value: Option<f64>) -> Result<f64> {
    value.ok_or_else(|| ShioError::DatabaseValue {
        field,
        value: "NULL".to_string(),
    })
}
