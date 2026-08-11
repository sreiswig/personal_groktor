//! Fixture integration tests over examples/sample_metrics.{csv,json}.
//!
//! Covers ingest → normalize → store → analyze → brief → lab using public APIs.

use std::path::PathBuf;

use chrono::NaiveDate;
use tempfile::TempDir;

use personal_groktor::analyze;
use personal_groktor::brief::{self, BriefOptions};
use personal_groktor::ingest;
use personal_groktor::lab;
use personal_groktor::normalize;
use personal_groktor::schema::{Arm, MetricKind};
use personal_groktor::store::Store;

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples").join(name)
}

fn d(y: i32, m: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, day).expect("valid date")
}

fn load_fixture_points() -> Vec<personal_groktor::schema::MetricPoint> {
    let mut rows = ingest::load_path(&example("sample_metrics.csv")).expect("csv");
    rows.extend(ingest::load_path(&example("sample_metrics.json")).expect("json"));
    let (points, errors) = normalize::normalize_all(&rows);
    assert!(
        errors.is_empty(),
        "normalize errors on sample fixtures: {errors:?}"
    );
    points
}

#[test]
fn ingest_and_normalize_fixture_counts() {
    let csv_rows = ingest::load_path(&example("sample_metrics.csv")).unwrap();
    let json_rows = ingest::load_path(&example("sample_metrics.json")).unwrap();
    assert_eq!(csv_rows.len(), 40, "sample CSV has 10 days × 4 metrics");
    assert_eq!(json_rows.len(), 3, "sample JSON has 3 readiness rows");

    let mut rows = csv_rows;
    rows.extend(json_rows);
    let (points, errors) = normalize::normalize_all(&rows);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(points.len(), 43);

    let sleep = points
        .iter()
        .filter(|p| p.kind == MetricKind::SleepDurationHours)
        .count();
    let readiness = points
        .iter()
        .filter(|p| p.kind == MetricKind::ReadinessScore)
        .count();
    assert_eq!(sleep, 10);
    assert_eq!(readiness, 3);
}

#[test]
fn store_tempdir_upsert_and_range() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path().join("pipeline.db")).unwrap();
    let points = load_fixture_points();

    let n = store.upsert_metrics(&points).unwrap();
    assert_eq!(n, points.len());
    // Idempotent on same ids
    assert_eq!(store.upsert_metrics(&points).unwrap(), 0);
    assert_eq!(store.metric_count().unwrap(), 43);

    let day28 = d(2026, 7, 28);
    let day29 = d(2026, 7, 29);
    let range = store.metrics_in_range(day28, day29).unwrap();
    // CSV: 4 metrics/day × 2 days + JSON readiness on both days = 10
    assert_eq!(range.len(), 10);
    assert!(range.iter().all(|p| p.day == day28 || p.day == day29));

    let (min, max) = store.day_range().unwrap().expect("day range");
    assert_eq!(min, d(2026, 7, 20));
    assert_eq!(max, day29);
}

#[test]
fn analyze_rule_ids_around_july_28_29() {
    let points = load_fixture_points();
    let findings = analyze::analyze(&points);
    assert!(!findings.is_empty());

    let around: Vec<_> = findings
        .iter()
        .filter(|f| f.day == d(2026, 7, 28) || f.day == d(2026, 7, 29))
        .collect();
    assert!(
        !around.is_empty(),
        "expected findings on 2026-07-28/29, got: {:?}",
        findings
            .iter()
            .map(|f| (&f.day, f.rule_id.as_str()))
            .collect::<Vec<_>>()
    );

    let ids: std::collections::BTreeSet<_> =
        around.iter().map(|f| f.rule_id.as_str()).collect();

    // Sample fixtures intentionally degrade sleep / RHR / HRV / steps late July.
    for expected in [
        "sleep_debt_3d",
        "rhr_after_poor_sleep",
        "activity_dropoff",
        "hrv_drop",
    ] {
        assert!(
            ids.contains(expected),
            "missing rule_id `{expected}` among {ids:?}"
        );
    }

    assert!(
        findings.iter().any(|f| f.day == d(2026, 7, 29) && f.rule_id == "sleep_debt_3d"),
        "sleep debt should flag the third short-sleep day (2026-07-29)"
    );
}

#[test]
fn brief_for_2026_07_29() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path().join("brief.db")).unwrap();
    store.upsert_metrics(&load_fixture_points()).unwrap();

    let digest = brief::build_brief(
        &store,
        &BriefOptions {
            day: Some(d(2026, 7, 29)),
            week: false,
            refresh: true,
        },
    )
    .unwrap();

    assert_eq!(digest.day, d(2026, 7, 29));
    assert_eq!(digest.metric_count, 5); // 4 CSV + readiness JSON
    assert!(
        !digest.findings.is_empty(),
        "brief for degraded day should include findings"
    );
    assert!(
        digest.summary.contains("2026-07-29"),
        "summary should mention target day: {}",
        digest.summary
    );
    assert!(store.finding_count().unwrap() > 0);
}

#[test]
fn lab_start_assign_and_report() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path().join("lab.db")).unwrap();
    store.upsert_metrics(&load_fixture_points()).unwrap();

    let outcomes = vec![
        lab::parse_outcome_spec("hrv:up").unwrap(),
        lab::parse_outcome_spec("sleep_duration_hours:up").unwrap(),
    ];
    let exp = lab::start_experiment(
        &store,
        "recovery_focus",
        "Recovery focus week",
        "Protecting sleep raises next-day HRV",
        outcomes,
        Some("hrv"),
        7,
        Some(d(2026, 7, 20)),
        None,
    )
    .unwrap();
    assert_eq!(exp.slug, "recovery_focus");
    assert_eq!(exp.outcomes.iter().filter(|o| o.primary).count(), 1);

    // Control early week, intervention on the stressed late days.
    let n_ctrl = lab::assign_range(
        &store,
        "recovery_focus",
        d(2026, 7, 20),
        d(2026, 7, 26),
        Arm::Control,
        None,
    )
    .unwrap();
    assert_eq!(n_ctrl, 7);

    lab::assign_day(
        &store,
        "recovery_focus",
        d(2026, 7, 28),
        Arm::Intervention,
        Some("stressed day".into()),
    )
    .unwrap();
    lab::assign_day(
        &store,
        "recovery_focus",
        d(2026, 7, 29),
        Arm::Intervention,
        None,
    )
    .unwrap();

    assert_eq!(
        store.arm_for_day(exp.id, d(2026, 7, 28)).unwrap(),
        Some(Arm::Intervention)
    );

    let report = lab::build_lab_report(&store, "recovery_focus").unwrap();
    assert_eq!(report.n_control, 7);
    assert_eq!(report.n_intervention, 2);
    assert!(!report.outcomes.is_empty());
    assert!(
        report.summary.contains("recovery_focus"),
        "lab summary should name the experiment: {}",
        report.summary
    );
}
