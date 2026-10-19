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
    let series = finite_series(series);
    let mut findings = Vec::new();

    for (kind, by_day) in &series {
        findings.extend(zscore_outliers(kind, by_day));
    }

    findings.extend(sleep_debt(&series));
    findings.extend(rhr_up_after_poor_sleep(&series));
    findings.extend(activity_dropoff(&series));
    findings.extend(hrv_drop(&series));

    findings.sort_by(|a, b| a.day.cmp(&b.day).then(a.title.cmp(&b.title)));
    findings
}

/// Drop NaN / ±inf before comparisons. `-inf < threshold` is true, so a
/// non-finite sample would otherwise become a real alert.
fn finite_series(series: &Series) -> Series {
    series
        .iter()
        .filter_map(|(kind, by_day)| {
            let by_day: BTreeMap<NaiveDate, f64> = by_day
                .iter()
                .filter(|(_, value)| crate::normalize::metric_value_accepted(**value))
                .map(|(day, value)| (*day, *value))
                .collect();
            if by_day.is_empty() {
                None
            } else {
                Some((kind.clone(), by_day))
            }
        })
        .collect()
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
    use super::super::stats::std_dev;
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

    #[test]
    fn non_finite_samples_do_not_alert_and_do_not_hide_a_finite_drop() {
        let mut hrv = BTreeMap::new();
        for day in 1..=7 {
            hrv.insert(d(day), 50.0);
        }
        // `-inf < 0.8 * baseline` is true, so this used to alert on its own
        // and poison the next day's mean (`m <= 0` then skips a real drop).
        hrv.insert(d(8), f64::NEG_INFINITY);
        hrv.insert(d(9), f64::NAN);
        hrv.insert(d(10), f64::INFINITY);
        hrv.insert(d(11), 10.0);
        let mut series = BTreeMap::new();
        series.insert(MetricKind::HeartRateVariabilityMs, hrv);
        let findings = run_rules(&series);
        let drops: Vec<_> = findings
            .iter()
            .filter(|f| f.rule_id == "hrv_drop")
            .collect();
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].day, d(11));
        assert_eq!(drops[0].value, Some(10.0));
        assert!(findings.iter().all(|f| {
            f.value
                .into_iter()
                .chain(f.baseline)
                .all(|v| !v.is_nan() && v.is_finite())
        }));
    }

    /// Deterministic stand-in for proptest/quickcheck (neither is a dev-dependency).
    struct Lcg(u64);

    impl Lcg {
        fn next_unit(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((self.0 >> 11) as f64) / ((1_u64 << 53) as f64)
        }
    }

    fn day_from(offset: i64) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() + chrono::Duration::days(offset)
    }

    fn series_of(kind: MetricKind, values: &[(i64, f64)]) -> Series {
        let mut by_day = BTreeMap::new();
        for (offset, value) in values {
            by_day.insert(day_from(*offset), *value);
        }
        let mut series = Series::new();
        series.insert(kind, by_day);
        series
    }

    fn assert_no_nan(findings: &[Finding]) {
        for f in findings {
            if let Some(v) = f.value {
                assert!(!v.is_nan(), "{} value is NaN", f.rule_id);
            }
            if let Some(b) = f.baseline {
                assert!(!b.is_nan(), "{} baseline is NaN", f.rule_id);
            }
        }
    }

    fn expect_rule(series: &Series, rule_id: &str, day: NaiveDate) {
        let findings = run_rules(series);
        assert!(
            findings
                .iter()
                .any(|f| f.rule_id == rule_id && f.day == day),
            "missing {rule_id} on {day}; got {:?}",
            findings
                .iter()
                .map(|f| (f.day, f.rule_id.as_str()))
                .collect::<Vec<_>>()
        );
        assert_no_nan(&findings);
    }

    fn oracle_mean(values: &[f64]) -> Option<f64> {
        if values.is_empty() || values.iter().any(|v| !v.is_finite()) {
            return None;
        }
        Some(values.iter().sum::<f64>() / values.len() as f64)
    }

    fn oracle_z(value: f64, baseline: &[f64]) -> Option<f64> {
        if baseline.len() < 2 || !value.is_finite() || baseline.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let m = oracle_mean(baseline)?;
        let var =
            baseline.iter().map(|v| (v - m).powi(2)).sum::<f64>() / (baseline.len() as f64 - 1.0);
        let s = var.sqrt();
        if !s.is_finite() || s < 1e-9 {
            return None;
        }
        let z = (value - m) / s;
        z.is_finite().then_some(z)
    }

    fn should_hrv_drop(baseline: &[f64], value: f64) -> bool {
        if baseline.len() < 7 || !value.is_finite() {
            return false;
        }
        matches!(oracle_mean(baseline), Some(m) if m > 0.0 && value < m * 0.8)
    }

    fn should_activity_drop(baseline: &[f64], value: f64) -> bool {
        if baseline.len() < 7 || !value.is_finite() {
            return false;
        }
        matches!(
            oracle_mean(baseline),
            Some(m) if m >= 3000.0 && value < m * 0.5
        )
    }

    fn should_sleep_debt(values: &[f64]) -> bool {
        values.len() >= 3 && values.iter().all(|v| v.is_finite() && *v < 6.5)
    }

    fn fill_constant(n: usize, value: f64) -> Vec<(i64, f64)> {
        (0..n).map(|i| (i as i64, value)).collect()
    }

    #[test]
    fn finite_inputs_never_silently_drop_an_alert() {
        let extremes = [
            0.0,
            -0.0,
            1e308,
            -1e308,
            f64::MIN_POSITIVE,
            -f64::MIN_POSITIVE,
            f64::MAX,
            f64::MIN,
        ];

        // Z-score: modest spread so every extreme is a real finite outlier.
        let z_base = [10.0, 11.0, 12.0, 13.0, 14.0, 15.0];
        for value in extremes {
            let z = oracle_z(value, &z_base).unwrap_or_else(|| {
                panic!("expected a finite z-score for {value} against {z_base:?}")
            });
            assert!(z.abs() >= 2.0, "{value} z={z} should clear the threshold");
            let mut rows: Vec<(i64, f64)> = z_base
                .iter()
                .enumerate()
                .map(|(i, v)| (i as i64, *v))
                .collect();
            let day = rows.len() as i64;
            rows.push((day, value));
            expect_rule(
                &series_of(MetricKind::RestingHeartRateBpm, &rows),
                "zscore_outlier",
                day_from(day),
            );
            let got = z_score(value, &z_base).unwrap_or_else(|| {
                panic!("z_score dropped a finite outlier {value}");
            });
            assert!(!got.is_nan());
            assert!((got - z).abs() <= 1e-6 * z.abs().max(1.0));
        }

        // HRV drop, including a subnormal baseline and an overflowing 1e308 baseline.
        for level in [50.0, 1.0, f64::MIN_POSITIVE, 1e308] {
            let baseline = vec![level; 7];
            for value in [0.0, -1e308, f64::MIN_POSITIVE, level * 0.5, 1e308] {
                if !should_hrv_drop(&baseline, value) {
                    continue;
                }
                let mut rows = fill_constant(7, level);
                rows.push((7, value));
                expect_rule(
                    &series_of(MetricKind::HeartRateVariabilityMs, &rows),
                    "hrv_drop",
                    day_from(7),
                );
            }
        }

        // Activity drop-off.
        for level in [8000.0, 3000.0, 1e308] {
            let baseline = vec![level; 7];
            for value in [0.0, f64::MIN_POSITIVE, -1e308, f64::MIN, level * 0.49] {
                if !should_activity_drop(&baseline, value) {
                    continue;
                }
                let mut rows = fill_constant(7, level);
                rows.push((7, value));
                expect_rule(
                    &series_of(MetricKind::Steps, &rows),
                    "activity_dropoff",
                    day_from(7),
                );
            }
        }

        // Three short-sleep days, including extremes below 6.5h.
        let short = [
            0.0,
            -0.0,
            6.49,
            5.5,
            1e-10,
            f64::MIN_POSITIVE,
            -f64::MIN_POSITIVE,
            -1e308,
            f64::MIN,
        ];
        for value in short {
            let vals = [value, value, value];
            assert!(
                should_sleep_debt(&vals),
                "{value} should count as short sleep"
            );
            let rows = fill_constant(3, value);
            expect_rule(
                &series_of(MetricKind::SleepDurationHours, &rows),
                "sleep_debt_3d",
                day_from(2),
            );
        }

        let mut lcg = Lcg(0xC0FFEE_u64);
        for trial in 0..24 {
            let mut z_rows = vec![(0_i64, 40.0), (1, 60.0)];
            for i in 2..6 {
                z_rows.push((i, 40.0 + lcg.next_unit() * 20.0));
            }
            let base: Vec<f64> = z_rows.iter().map(|(_, v)| *v).collect();
            let outlier = -50.0 - lcg.next_unit() * 100.0;
            let z = oracle_z(outlier, &base).expect("lcg baseline should have a finite z");
            assert!(z.abs() >= 2.0);
            z_rows.push((6, outlier));
            expect_rule(
                &series_of(MetricKind::ReadinessScore, &z_rows),
                "zscore_outlier",
                day_from(6),
            );

            let level = 1.0 + lcg.next_unit() * 80.0;
            let low = level * 0.5;
            assert!(should_hrv_drop(&[level; 7], low));
            let mut hrv = fill_constant(7, level);
            hrv.push((7, low));
            expect_rule(
                &series_of(MetricKind::HeartRateVariabilityMs, &hrv),
                "hrv_drop",
                day_from(7),
            );

            let steps_level = 3000.0 + lcg.next_unit() * 12_000.0;
            assert!(should_activity_drop(&[steps_level; 7], 0.0));
            let mut steps = fill_constant(7, steps_level);
            steps.push((7, 0.0));
            expect_rule(
                &series_of(MetricKind::Steps, &steps),
                "activity_dropoff",
                day_from(7),
            );

            let a = lcg.next_unit() * 6.4;
            let b = -lcg.next_unit() * 4.0;
            let c = lcg.next_unit() * 5.0;
            assert!(should_sleep_debt(&[a, b, c]));
            let origin = trial as i64;
            expect_rule(
                &series_of(
                    MetricKind::SleepDurationHours,
                    &[(origin, a), (origin + 1, b), (origin + 2, c)],
                ),
                "sleep_debt_3d",
                day_from(origin + 2),
            );
        }

        // Analytics on finite inputs, including the extremes above, never return NaN.
        let mut corpus = extremes.to_vec();
        corpus.extend([1.0, -1.0, 5.0, 6.49, 40.0, 50.0, 8000.0, 3000.0]);
        let mut sweep = Lcg(0xA11C_E5EED);
        for _ in 0..48 {
            let u = sweep.next_unit();
            corpus.push(u);
            corpus.push(u * 100.0 - 50.0);
            corpus.push((u - 0.5) * 2.0 * 1e308);
        }
        assert!(corpus.iter().all(|v| v.is_finite()));
        for window in corpus.windows(8) {
            if let Some(m) = mean(window) {
                assert!(!m.is_nan(), "mean NaN for {window:?}");
            }
            if let Some(s) = std_dev(window) {
                assert!(!s.is_nan(), "std_dev NaN for {window:?}");
            }
            for &value in &corpus {
                if let Some(z) = z_score(value, window) {
                    assert!(!z.is_nan(), "z_score NaN for {value} vs {window:?}");
                }
            }
        }
    }
}
