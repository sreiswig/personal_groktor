//! Map raw export rows onto the canonical metric schema.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};

use crate::error::{GroktorError, Result};
use crate::schema::{MetricKind, MetricPoint, RawMetricRow};

/// Make an untrusted string safe for a single log line: escape control
/// characters (\n, \r, \t, ESC, ...) and cap the length.
pub(crate) fn sanitize_for_log(s: &str) -> String {
    const MAX: usize = 64;
    let mut out = String::with_capacity(s.len().min(MAX) + 8);
    for (i, c) in s.chars().enumerate() {
        if i == MAX {
            out.push('\u{2026}');
            break;
        }
        if c.is_control() {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

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
    Err(GroktorError::Parse(format!(
        "unrecognized date: {}",
        sanitize_for_log(s)
    )))
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
    // Non-finite values parse as f64 (CSV `NaN` / `inf` / `-inf`) but poison
    // means and make rule comparisons silently false. Reject the row instead.
    if !row.value.is_finite() {
        let value = row.value;
        let metric = sanitize_for_log(&row.metric);
        return Err(GroktorError::Parse(format!(
            "non-finite value {value} for metric '{metric}'"
        )));
    }

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

    fn raw(date: &str, metric: &str, value: f64) -> RawMetricRow {
        RawMetricRow {
            date: date.into(),
            timestamp: None,
            metric: metric.into(),
            value,
            unit: None,
            source: None,
            notes: None,
        }
    }

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

    #[test]
    fn rejects_non_finite_values() {
        for (value, shown) in [
            (f64::NAN, "NaN"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
        ] {
            let row = raw("2026-08-01", "resting_hr", value);
            let err = normalize_row(&row).unwrap_err().to_string();
            assert_eq!(
                err,
                format!("Parse error: non-finite value {shown} for metric 'resting_hr'")
            );
        }
    }

    #[test]
    fn normalize_all_reports_non_finite_rows_and_keeps_finite_ones() {
        let rows = vec![
            raw("2026-08-01", "steps", 8000.0),
            raw("2026-08-01", "rhr", f64::NAN),
            raw("2026-08-02", "hrv", f64::INFINITY),
            raw("not-a-date", "steps", 1.0),
            raw("2026-08-03", "sleep_hours", f64::NEG_INFINITY),
            raw("2026-08-03", "steps", 9100.0),
        ];
        let (points, errors) = normalize_all(&rows);
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].value, 8000.0);
        assert_eq!(points[0].kind, MetricKind::Steps);
        assert_eq!(points[1].value, 9100.0);
        assert_eq!(
            errors,
            vec![
                "row 1: Parse error: non-finite value NaN for metric 'rhr'".to_string(),
                "row 2: Parse error: non-finite value inf for metric 'hrv'".to_string(),
                "row 3: Parse error: unrecognized date: not-a-date".to_string(),
                "row 4: Parse error: non-finite value -inf for metric 'sleep_hours'".to_string(),
            ]
        );
    }

    #[test]
    fn non_finite_metric_name_cannot_inject_log_lines() {
        let row = raw("2026-08-01", "rhr\n2026 INFO forged\x1b[2J", f64::NAN);
        let err = normalize_row(&row).unwrap_err().to_string();
        assert!(!err.contains('\n'), "error must stay on one line: {err:?}");
        assert!(
            !err.contains('\x1b'),
            "error must not contain a raw ESC: {err:?}"
        );
        assert!(
            err.contains(r"rhr\n2026 INFO forged\u{1b}[2J"),
            "controls should be escaped, not dropped: {err:?}"
        );
    }

    #[test]
    fn unrecognized_date_escapes_embedded_newline() {
        let row = raw("2026-08-01\nINFO forged", "steps", 1.0);
        let err = normalize_row(&row).unwrap_err().to_string();
        assert!(!err.contains('\n'), "error must stay on one line: {err:?}");
        assert!(
            err.contains(r"unrecognized date: 2026-08-01\nINFO forged"),
            "newline in the date should be escaped: {err:?}"
        );
    }

    #[test]
    fn sanitize_for_log_passes_plain_ascii() {
        assert_eq!(sanitize_for_log("resting_hr"), "resting_hr");
    }

    #[test]
    fn sanitize_for_log_escapes_cr_lf_tab() {
        assert_eq!(sanitize_for_log("a\r\n\tb"), r"a\r\n\tb");
    }

    #[test]
    fn sanitize_for_log_truncates_at_64_with_ellipsis() {
        let sixty_four = "a".repeat(64);
        assert_eq!(sanitize_for_log(&sixty_four), sixty_four);

        let out = sanitize_for_log(&"b".repeat(80));
        assert_eq!(out.chars().count(), 65);
        assert!(out.ends_with('\u{2026}'));
        assert_eq!(out.chars().take(64).collect::<String>(), "b".repeat(64));
    }

    #[test]
    fn sanitize_for_log_counts_chars_not_bytes() {
        // 'é' is two UTF-8 bytes. Forty of them exceed 64 bytes but stay under
        // the 64-character cap, so the string is returned unchanged.
        let under = "é".repeat(40);
        assert!(under.len() > 64);
        assert_eq!(under.chars().count(), 40);
        assert_eq!(sanitize_for_log(&under), under);

        // Sixty-five multibyte chars truncate on the character boundary.
        let over = "é".repeat(65);
        let out = sanitize_for_log(&over);
        assert_eq!(out.chars().count(), 65);
        assert!(out.ends_with('\u{2026}'));
        assert_eq!(out.chars().take(64).collect::<String>(), "é".repeat(64));
    }

    #[test]
    fn sanitize_for_log_empty_string() {
        assert_eq!(sanitize_for_log(""), "");
    }
}
