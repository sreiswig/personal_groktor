use std::fs;
use std::path::Path;

use crate::error::{GroktorError, Result};
use crate::schema::RawMetricRow;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum JsonPayload {
    Array(Vec<RawMetricRow>),
    Wrapped { metrics: Vec<RawMetricRow> },
}

pub fn load_json(path: &Path) -> Result<Vec<RawMetricRow>> {
    let text = fs::read_to_string(path)?;
    let payload: JsonPayload = serde_json::from_str(&text).map_err(|e| {
        GroktorError::Parse(format!(
            "JSON must be an array of metric rows or {{ \"metrics\": [...] }}: {e}"
        ))
    })?;
    let rows = match payload {
        JsonPayload::Array(rows) => rows,
        JsonPayload::Wrapped { metrics } => metrics,
    };
    tracing::info!(path = %path.display(), count = rows.len(), "loaded JSON metrics");
    Ok(rows)
}
