//! Fixture-backed pipeline: ingest → normalize → store → analyze → brief/lab.

use chrono::NaiveDate;
use personal_groktor::analyze;
use personal_groktor::brief::{build_brief, BriefOptions};
use personal_groktor::ingest;
use personal_groktor::lab;
use personal_groktor::normalize;
use personal_groktor::schema::{Arm, MetricKind};
use personal_groktor::Store;
use tempfile::TempDir;

fn load_fixtures(store: &Store) {
    let csv = ingest::load_path(std::path::Path::new("examples/sample_metrics.csv"))
        .expect("load csv fixture");
    let (points, errs) = normalize::normalize_all(&csv);
    assert!(errs.is_empty(), "csv normalize errors: {errs:?}");
    assert_eq!(points.len(), 40, "csv should yield 40 metric points");
    let n = store.upsert_metrics(&points).expect("upsert csv");
    assert_eq!(n, 40);

    let json = ingest::load_path(std::path::Path::new("examples/sample_metrics.json"))
        .expect("load json fixture");
    let (jpoints, jerrs) = normalize::normalize_all(&json);
    assert!(jerrs.is_empty(), "json normalize errors: {jerrs:?}");
    assert_eq!(jpoints.len(), 3);
    assert!(jpoints
        .iter()
        .all(|p| matches!(p.kind, MetricKind::ReadinessScore)));
    let n2 = store.upsert_metrics(&jpoints).expect("upsert json");
    assert_eq!(n2, 3);
}

#[test]
fn ingest_normalize_store_idempotent() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path().join("t.db")).unwrap();

    // Normalize once; re-upsert same MetricPoint ids to exercise INSERT OR IGNORE.
    // (A second normalize() would mint new UUIDs and would insert again.)
    let csv = ingest::load_path(std::path::Path::new("examples/sample_metrics.csv")).unwrap();
    let (points, errs) = normalize::normalize_all(&csv);
    assert!(errs.is_empty(), "csv normalize errors: {errs:?}");
    assert_eq!(points.len(), 40);
    assert_eq!(store.upsert_metrics(&points).unwrap(), 40);
    assert_eq!(
        store.upsert_metrics(&points).unwrap(),
        0,
        "second upsert of same ids should insert nothing"
    );

    let json = ingest::load_path(std::path::Path::new("examples/sample_metrics.json")).unwrap();
    let (jpoints, jerrs) = normalize::normalize_all(&json);
    assert!(jerrs.is_empty(), "json normalize errors: {jerrs:?}");
    assert_eq!(jpoints.len(), 3);
    assert_eq!(store.upsert_metrics(&jpoints).unwrap(), 3);

    assert!(store.metric_count().unwrap() >= 43);
    let range = store.day_range().unwrap().expect("day range");
    assert_eq!(range.0, NaiveDate::from_ymd_opt(2026, 7, 20).unwrap());
    assert_eq!(range.1, NaiveDate::from_ymd_opt(2026, 7, 29).unwrap());
}

#[test]
fn sample_rules_and_brief() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path().join("t.db")).unwrap();
    load_fixtures(&store);

    let start = NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 7, 29).unwrap();
    let points = store.metrics_in_range(start, end).unwrap();
    let findings = analyze::analyze(&points);
    let ids: Vec<_> = findings.iter().map(|f| f.rule_id.as_str()).collect();
    assert!(
        ids.iter().any(|id| *id == "sleep_debt_3d")
            || ids.iter().any(|id| *id == "hrv_drop")
            || ids.iter().any(|id| *id == "activity_dropoff")
            || ids.iter().any(|id| *id == "rhr_after_poor_sleep"),
        "expected sample-window rule hits, got {ids:?}"
    );

    let digest = build_brief(
        &store,
        &BriefOptions {
            day: Some(end),
            week: false,
            refresh: true,
        },
    )
    .expect("build_brief");
    assert!(digest.metric_count > 0);
    assert_eq!(digest.day, end);
}

#[test]
fn lab_happy_path() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path().join("t.db")).unwrap();
    load_fixtures(&store);

    let outcome = lab::parse_outcome_spec("hrv:up").unwrap();
    let exp = lab::start_experiment(
        &store,
        "early_bed",
        "Early bed",
        "Earlier bedtime raises HRV",
        vec![outcome],
        Some("hrv"),
        7,
        Some(NaiveDate::from_ymd_opt(2026, 7, 20).unwrap()),
        None,
    )
    .unwrap();
    assert_eq!(exp.slug, "early_bed");

    lab::assign_range(
        &store,
        "early_bed",
        NaiveDate::from_ymd_opt(2026, 7, 20).unwrap(),
        NaiveDate::from_ymd_opt(2026, 7, 24).unwrap(),
        Arm::Control,
        None,
    )
    .unwrap();
    lab::assign_range(
        &store,
        "early_bed",
        NaiveDate::from_ymd_opt(2026, 7, 25).unwrap(),
        NaiveDate::from_ymd_opt(2026, 7, 29).unwrap(),
        Arm::Intervention,
        None,
    )
    .unwrap();

    let report = lab::build_lab_report(&store, "early_bed").expect("lab report");
    let _ = report;
}
