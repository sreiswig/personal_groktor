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
        if !crate::normalize::metric_value_accepted(p.value) {
            continue;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::MetricPoint;
    use chrono::Utc;

    #[test]
    fn daily_series_drops_non_finite_values() {
        let day = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();
        let next = NaiveDate::from_ymd_opt(2026, 8, 2).unwrap();
        let points = [
            MetricPoint::new(MetricKind::Steps, Utc::now(), day, 10.0, "count", "t"),
            MetricPoint::new(MetricKind::Steps, Utc::now(), day, f64::NAN, "count", "t"),
            MetricPoint::new(
                MetricKind::HeartRateVariabilityMs,
                Utc::now(),
                day,
                f64::INFINITY,
                "ms",
                "t",
            ),
            MetricPoint::new(
                MetricKind::HeartRateVariabilityMs,
                Utc::now(),
                next,
                f64::NEG_INFINITY,
                "ms",
                "t",
            ),
        ];
        let series = daily_series(&points);
        let steps = series.get(&MetricKind::Steps).expect("finite steps kept");
        assert_eq!(steps.get(&day), Some(&10.0));
        assert!(!series.contains_key(&MetricKind::HeartRateVariabilityMs));
        assert!(series
            .values()
            .all(|by_day| by_day.values().all(|v| v.is_finite())));
    }
}
