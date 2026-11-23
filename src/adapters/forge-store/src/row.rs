//! Small helpers for decoding portable column types.

use std::str::FromStr;

use forge_core::{DomainError, DomainResult, Timestamp};
use sqlx::Row;
use sqlx::any::AnyRow;

pub(crate) fn text(row: &AnyRow, col: &str) -> DomainResult<String> {
    row.try_get::<String, _>(col).map_err(DomainError::internal)
}

pub(crate) fn opt_text(row: &AnyRow, col: &str) -> DomainResult<Option<String>> {
    row.try_get::<Option<String>, _>(col).map_err(DomainError::internal)
}

pub(crate) fn int(row: &AnyRow, col: &str) -> DomainResult<i64> {
    row.try_get::<i64, _>(col).map_err(DomainError::internal)
}

pub(crate) fn opt_int(row: &AnyRow, col: &str) -> DomainResult<Option<i64>> {
    row.try_get::<Option<i64>, _>(col).map_err(DomainError::internal)
}

pub(crate) fn flag(row: &AnyRow, col: &str) -> DomainResult<bool> {
    int(row, col).map(|v| v != 0)
}

pub(crate) fn ts(row: &AnyRow, col: &str) -> DomainResult<Timestamp> {
    int(row, col).map(Timestamp)
}

pub(crate) fn opt_ts(row: &AnyRow, col: &str) -> DomainResult<Option<Timestamp>> {
    opt_int(row, col).map(|v| v.map(Timestamp))
}

pub(crate) fn u32_col(row: &AnyRow, col: &str) -> DomainResult<u32> {
    int(row, col).map(|v| u32::try_from(v).unwrap_or(u32::MAX))
}

pub(crate) fn parsed<T>(row: &AnyRow, col: &str) -> DomainResult<T>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    text(row, col)?.parse().map_err(|e: T::Err| DomainError::Internal(format!("column {col}: {e}")))
}

pub(crate) fn opt_parsed<T>(row: &AnyRow, col: &str) -> DomainResult<Option<T>>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    opt_text(row, col)?
        .map(|s| s.parse().map_err(|e: T::Err| DomainError::Internal(format!("column {col}: {e}"))))
        .transpose()
}

pub(crate) fn json(row: &AnyRow, col: &str) -> DomainResult<serde_json::Value> {
    serde_json::from_str(&text(row, col)?).map_err(DomainError::internal)
}

pub(crate) const fn b(v: bool) -> i64 {
    if v { 1 } else { 0 }
}
