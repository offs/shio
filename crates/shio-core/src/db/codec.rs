use crate::error::{Result, ShioError};
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;

pub(super) fn invalid(field: &'static str, value: &dyn std::fmt::Display) -> ShioError {
    ShioError::DatabaseValue {
        field,
        value: value.to_string(),
    }
}

pub(super) fn encode_json<T: serde::Serialize + ?Sized>(
    field: &'static str,
    value: &T,
) -> Result<String> {
    serde_json::to_string(value).map_err(|source| ShioError::DatabaseJson { field, source })
}

pub(super) fn encode_u64(field: &'static str, value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| invalid(field, &value))
}

pub(super) fn decode_json<T: DeserializeOwned>(field: &'static str, value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(|source| ShioError::DatabaseJson { field, source })
}

pub(super) fn decode_ts(field: &'static str, value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|ts| ts.with_timezone(&Utc))
        .map_err(|source| ShioError::DatabaseTimestamp { field, source })
}

pub(super) fn decode_int<T: TryFrom<i64>>(field: &'static str, value: i64) -> Result<T> {
    T::try_from(value).map_err(|_| invalid(field, &value))
}

pub(super) fn decode_bool(field: &'static str, value: i64) -> Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(invalid(field, &value)),
    }
}

pub(super) fn decode_ratio(field: &'static str, value: f64) -> Result<f32> {
    if value.is_finite() && (0.0..=f64::from(f32::MAX)).contains(&value) {
        Ok(value as f32)
    } else {
        Err(invalid(field, &value))
    }
}

pub(super) fn decode_uuid(field: &'static str, value: &str) -> Result<uuid::Uuid> {
    uuid::Uuid::parse_str(value).map_err(|_| invalid(field, &value))
}
