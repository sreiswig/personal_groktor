//! Explainable rule-based flags over daily metric series.

use chrono::NaiveDate;
use std::collections::BTreeMap;
use uuid::Uuid;

use super::stats::{mean, z_score};
use crate::schema::{Finding, MetricKind, Severity};

const BASELINE_DAYS: usize = 14;
const Z_THRESHOLD: f64 = 2.0;

type Series = BTreeMap<MetricKind, BTreeMap<NaiveDate, f64>>;

pub fn run_rules(series: &Series) -> Vec<Finding> {
    let mut findings = Vec::new();

    for (kind, by_day) in series {
        findings.extend(zscore_outliers(kind, by_day));
    }

    findings.extend(sleep_debt(series));
    findings.extend(rhr_up_after_poor_sleep(series));
    findings.extend(activity_dropoff(series));
    findings.extend(hrv_drop(series));

    findings.sort_by(|a, b| a.day.cmp(&b.day).then(a.title.cmp(&b.title)));
    findings
}

fn baseline_before(by_day: &BTreeMap<NaiveDate, f64>, day: NaiveDate) -> Vec<f64> {
    by_day
        .iter()
        .filter(|(d, _)| **d < day)
        .rev()
        .take(BASELINE_DAYS)
        .map(|(_, v)| *v)
        .collect()
}

fn zscore_outliers(kind: &MetricKind, by_day: &BTreeMap<NaiveDate, f64>) -> Vec<Finding> {
    // Only flag clinically relevant continuous metrics with z-score.
    let interesting = matches!(
        kind,
        MetricKind::RestingHeartRateBpm
            | MetricKind::HeartRateVariabilityMs
            | MetricKind::SleepDurationHours
            | MetricKind::Steps
            | MetricKind::ReadinessScore
    );
    if !interesting {
        return Vec::new();
    }

    let mut out = Vec::new();
    for (day, &value) in by_day {
        let base = baseline_before(by_day, *day);
        if base.len() < 5 {
            continue;
        }
        let Some(z) = z_score(value, &base) else {
            continue;
        };
        if z.abs() < Z_THRESHOLD {
            continue;
        }
        let direction = if z > 0.0 { "elevated" } else { "depressed" };
        let severity = if z.abs() >= 3.0 {
            Severity::High
        } else {
            Severity::Medium
        };
        let baseline = mean(&base);
        out.push(Finding {
            id: Uuid::new_v4(),
            day: *day,
            kind: kind.clone(),
            severity,
            title: format!("{kind} {direction}"),
            detail: format!(
                "{kind} was {value:.1} on {day} (z={z:.2} vs prior {BASELINE_DAYS}-day baseline). \
                 Not medical advice — context matters (illness, travel, training)."
            ),
            value: Some(value),
            baseline,
            rule_id: "zscore_outlier".into(),
        });
    }
    out
}

fn sleep_debt(series: &Series) -> Vec<Finding> {
    let Some(sleep) = series.get(&MetricKind::SleepDurationHours) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let days: Vec<_> = sleep.keys().copied().collect();
    for window in days.windows(3) {
        let vals: Vec<f64> = window
            .iter()
            .filter_map(|d| sleep.get(d).copied())
            .collect();
        if vals.len() < 3 {
            continue;
        }
        if vals.iter().all(|&v| v < 6.5) {
            let day = *window.last().unwrap();
            let avg = mean(&vals).unwrap_or(0.0);
            out.push(Finding {
                id: Uuid::new_v4(),
                day,
                kind: MetricKind::SleepDurationHours,
                severity: Severity::Medium,
                title: "Multi-day short sleep".into(),
                detail: format!(
                    "Sleep under 6.5h for 3 consecutive days ending {day} (avg {avg:.1}h). \
                     Sleep debt can raise resting HR and lower HRV."
                ),
                value: Some(avg),
                baseline: Some(7.5),
                rule_id: "sleep_debt_3d".into(),
            });
        }
    }
    out
}

fn rhr_up_after_poor_sleep(series: &Series) -> Vec<Finding> {
    let (Some(sleep), Some(rhr)) = (
        series.get(&MetricKind::SleepDurationHours),
        series.get(&MetricKind::RestingHeartRateBpm),
    ) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (day, &rhr_val) in rhr {
        // previous calendar day sleep
        let prev = *day - chrono::Duration::days(1);
        let Some(&sleep_h) = sleep.get(&prev) else {
            continue;
        };
        if sleep_h >= 6.5 {
            continue;
        }
        let base = baseline_before(rhr, *day);
        if base.len() < 5 {
            continue;
        }
        let Some(m) = mean(&base) else {
            continue;
        };
        if rhr_val < m + 3.0 {
            continue;
        }
        out.push(Finding {
            id: Uuid::new_v4(),
            day: *day,
            kind: MetricKind::RestingHeartRateBpm,
            severity: Severity::Medium,
            title: "Elevated RHR after short sleep".into(),
            detail: format!(
                "Resting HR {rhr_val:.0} bpm on {day} (~{:.1} above recent baseline) after \
                 only {sleep_h:.1}h sleep on {prev}. Classic recovery stress pattern.",
                rhr_val - m
            ),
            value: Some(rhr_val),
            baseline: Some(m),
            rule_id: "rhr_after_poor_sleep".into(),
        });
    }
    out
}

fn activity_dropoff(series: &Series) -> Vec<Finding> {
    let Some(steps) = series.get(&MetricKind::Steps) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (day, &value) in steps {
        let base = baseline_before(steps, *day);
        if base.len() < 7 {
            continue;
        }
        let Some(m) = mean(&base) else {
            continue;
        };
        if m < 3000.0 {
            continue;
        }
        // Drop of >50% vs baseline
        if value < m * 0.5 {
            out.push(Finding {
                id: Uuid::new_v4(),
                day: *day,
                kind: MetricKind::Steps,
                severity: Severity::Low,
                title: "Activity drop-off".into(),
                detail: format!(
                    "Steps {value:.0} on {day} is under half of recent average ({m:.0}). \
                     Could be rest day, illness, or schedule change."
                ),
                value: Some(value),
                baseline: Some(m),
                rule_id: "activity_dropoff".into(),
            });
        }
    }
    out
}

fn hrv_drop(series: &Series) -> Vec<Finding> {
    let Some(hrv) = series.get(&MetricKind::HeartRateVariabilityMs) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (day, &value) in hrv {
        let base = baseline_before(hrv, *day);
        if base.len() < 7 {
            continue;
        }
        let Some(m) = mean(&base) else {
            continue;
        };
        if m <= 0.0 {
            continue;
        }
        // >20% below baseline
        if value < m * 0.8 {
            let severity = if value < m * 0.7 {
                Severity::High
            } else {
                Severity::Medium
            };
            out.push(Finding {
                id: Uuid::new_v4(),
                day: *day,
                kind: MetricKind::HeartRateVariabilityMs,
                severity,
                title: "HRV drop".into(),
                detail: format!(
                    "HRV {value:.0} ms on {day} is {:.0}% below recent average ({m:.0} ms). \
                     Often tracks stress, illness, alcohol, or hard training.",
                    (1.0 - value / m) * 100.0
                ),
                value: Some(value),
                baseline: Some(m),
                rule_id: "hrv_drop".into(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn d(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, day).unwrap()
    }

    #[test]
    fn detects_sleep_debt() {
        let mut sleep = BTreeMap::new();
        for day in 1..=5 {
            sleep.insert(d(day), 5.5);
        }
        let mut series = BTreeMap::new();
        series.insert(MetricKind::SleepDurationHours, sleep);
        let findings = run_rules(&series);
        assert!(findings.iter().any(|f| f.rule_id == "sleep_debt_3d"));
    }

    #[test]
    fn detects_hrv_drop() {
        let mut hrv = BTreeMap::new();
        for day in 1..=10 {
            hrv.insert(d(day), 50.0);
        }
        hrv.insert(d(11), 30.0);
        let mut series = BTreeMap::new();
        series.insert(MetricKind::HeartRateVariabilityMs, hrv);
        let findings = run_rules(&series);
        assert!(findings.iter().any(|f| f.rule_id == "hrv_drop"));
    }
}
