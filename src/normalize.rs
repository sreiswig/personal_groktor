//! Map raw export rows onto the canonical metric schema.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};

use crate::error::{GroktorError, Result};
use crate::schema::{MetricKind, MetricPoint, RawMetricRow};

/// Default unit for a metric when the export omits one.
fn default_unit(kind: &MetricKind) -> &'static str {
    match kind {
        MetricKind::SleepDurationHours => "hours",
        MetricKind::SleepEfficiencyPct => "%",
        MetricKind::RestingHeartRateBpm => "bpm",
        MetricKind::HeartRateVariabilityMs => "ms",
        MetricKind::Steps => "count",
        MetricKind::ActiveCaloriesKcal => "kcal",
        MetricKind::ExerciseMinutes => "min",
        MetricKind::WeightKg => "kg",
        MetricKind::BodyFatPct => "%",
        MetricKind::ReadinessScore => "score",
        MetricKind::Spo2Pct => "%",
        MetricKind::Other(_) => "unknown",
    }
}

fn parse_day(s: &str) -> Result<NaiveDate> {
    let s = s.trim();
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(d);
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%m/%d/%Y") {
        return Ok(d);
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%d/%m/%Y") {
        return Ok(d);
    }
    // ISO datetime fragments
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc).date_naive());
    }
    Err(GroktorError::Parse(format!("unrecognized date: {s}")))
}

fn parse_timestamp(s: &str, fallback_day: NaiveDate) -> Result<DateTime<Utc>> {
    let s = s.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Ok(Utc.from_utc_datetime(&dt));
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Ok(Utc.from_utc_datetime(&dt));
    }
    // Date-only → noon UTC for stable ordering
    if let Ok(d) = parse_day(s) {
        let ndt = d
            .and_hms_opt(12, 0, 0)
            .ok_or_else(|| GroktorError::Parse(format!("invalid time for date {d}")))?;
        return Ok(Utc.from_utc_datetime(&ndt));
    }
    let ndt = fallback_day
        .and_hms_opt(12, 0, 0)
        .ok_or_else(|| GroktorError::Parse(format!("invalid fallback day {fallback_day}")))?;
    Ok(Utc.from_utc_datetime(&ndt))
}

/// Normalize a single raw row into a metric point.
pub fn normalize_row(row: &RawMetricRow) -> Result<MetricPoint> {
    let day = parse_day(&row.date)?;
    let recorded_at = match &row.timestamp {
        Some(ts) if !ts.is_empty() => parse_timestamp(ts, day)?,
        _ => parse_timestamp(&row.date, day)?,
    };

    let kind = MetricKind::from_raw_name(&row.metric);
    let unit = row
        .unit
        .clone()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| default_unit(&kind).to_string());
    let source = row
        .source
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());

    let mut point = MetricPoint::new(kind, recorded_at, day, row.value, unit, source);
    point.raw_name = Some(row.metric.clone());
    point.notes = row.notes.clone();
    Ok(point)
}

/// Normalize many rows; skip and log rows that fail to parse.
pub fn normalize_all(rows: &[RawMetricRow]) -> (Vec<MetricPoint>, Vec<String>) {
    let mut points = Vec::with_capacity(rows.len());
    let mut errors = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        match normalize_row(row) {
            Ok(p) => points.push(p),
            Err(e) => errors.push(format!("row {i}: {e}")),
        }
    }
    (points, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_common_aliases() {
        let row = RawMetricRow {
            date: "2026-08-01".into(),
            timestamp: None,
            metric: "RHR".into(),
            value: 56.0,
            unit: None,
            source: Some("oura".into()),
            notes: None,
        };
        let p = normalize_row(&row).unwrap();
        assert_eq!(p.kind, MetricKind::RestingHeartRateBpm);
        assert_eq!(p.unit, "bpm");
        assert_eq!(p.day, NaiveDate::from_ymd_opt(2026, 8, 1).unwrap());
    }
}
