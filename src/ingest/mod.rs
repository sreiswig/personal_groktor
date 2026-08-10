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
}
