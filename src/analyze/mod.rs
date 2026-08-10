//! Rule-based anomaly detection and simple statistics.

mod rules;
mod stats;

pub use rules::run_rules;
pub use stats::{mean, std_dev, z_score};

use chrono::NaiveDate;
use std::collections::{BTreeMap, BTreeSet};

use crate::schema::{Finding, MetricKind, MetricPoint};

/// Group metric points by (kind, day), taking the last value that day if multiple.
pub fn daily_series(points: &[MetricPoint]) -> BTreeMap<MetricKind, BTreeMap<NaiveDate, f64>> {
    let mut map: BTreeMap<MetricKind, BTreeMap<NaiveDate, f64>> = BTreeMap::new();
    for p in points {
        map.entry(p.kind.clone())
            .or_default()
            .insert(p.day, p.value);
    }
    map
}

/// Days present in the dataset (sorted).
pub fn unique_days(points: &[MetricPoint]) -> Vec<NaiveDate> {
    let set: BTreeSet<_> = points.iter().map(|p| p.day).collect();
    set.into_iter().collect()
}

/// Run full analysis pipeline over stored points.
pub fn analyze(points: &[MetricPoint]) -> Vec<Finding> {
    let series = daily_series(points);
    run_rules(&series)
}
