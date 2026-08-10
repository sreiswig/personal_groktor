use std::fs::File;
use std::path::Path;

use crate::error::Result;
use crate::schema::RawMetricRow;

pub fn load_csv(path: &Path) -> Result<Vec<RawMetricRow>> {
    let file = File::open(path)?;
    let mut rdr = csv::Reader::from_reader(file);
    let mut rows = Vec::new();
    for result in rdr.deserialize() {
        let row: RawMetricRow = result?;
        rows.push(row);
    }
    tracing::info!(path = %path.display(), count = rows.len(), "loaded CSV metrics");
    Ok(rows)
}
