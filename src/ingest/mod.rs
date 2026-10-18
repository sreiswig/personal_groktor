//! Health data ingestion from common export formats.

mod csv_source;
mod json_source;

use std::path::Path;

use crate::error::{GroktorError, Result};
use crate::schema::RawMetricRow;

pub use csv_source::load_csv;
pub use json_source::load_json;

/// Detect format from path extension and load raw metric rows.
pub fn load_path(path: &Path) -> Result<Vec<RawMetricRow>> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "csv" => load_csv(path),
        "json" => load_json(path),
        other => Err(GroktorError::UnsupportedFormat(format!(
            "expected .csv or .json, got '{other}' for {}",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn loads_csv_and_json() {
        let mut csv = NamedTempFile::with_suffix(".csv").unwrap();
        writeln!(
            csv,
            "date,metric,value,unit,source\n2026-08-01,steps,8000,count,fitbit"
        )
        .unwrap();
        let rows = load_path(csv.path()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].metric, "steps");

        let mut json = NamedTempFile::with_suffix(".json").unwrap();
        write!(
            json,
            r#"[{{"date":"2026-08-01","metric":"rhr","value":58.0,"unit":"bpm"}}]"#
        )
        .unwrap();
        let rows = load_path(json.path()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].metric, "rhr");
    }

    #[test]
    fn csv_non_finite_literals_load_then_fail_per_row() {
        let mut csv = NamedTempFile::with_suffix(".csv").unwrap();
        writeln!(
            csv,
            "date,metric,value,unit,source\n\
             2026-08-01,steps,8000,count,fitbit\n\
             2026-08-01,rhr,NaN,bpm,oura\n\
             2026-08-02,hrv,inf,ms,oura\n\
             2026-08-02,sleep_hours,-inf,hours,oura\n\
             2026-08-03,steps,9100,count,fitbit"
        )
        .unwrap();

        let rows = load_path(csv.path()).unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].value, 8000.0);
        assert!(rows[1].value.is_nan());
        assert!(rows[2].value.is_infinite() && rows[2].value.is_sign_positive());
        assert!(rows[3].value.is_infinite() && rows[3].value.is_sign_negative());
        assert_eq!(rows[4].value, 9100.0);

        let (points, errors) = crate::normalize::normalize_all(&rows);
        assert_eq!(points.len(), 2);
        assert!(points.iter().all(|p| p.value.is_finite()));
        assert_eq!(
            errors,
            vec![
                "row 1: Parse error: non-finite value NaN for metric 'rhr'".to_string(),
                "row 2: Parse error: non-finite value inf for metric 'hrv'".to_string(),
                "row 3: Parse error: non-finite value -inf for metric 'sleep_hours'".to_string(),
            ]
        );
    }
}
