//! Descriptive N=1 stats for lab reports.
//!
//! [`build_lab_report`] is the effect edge (SQLite + clock). [`assemble_lab_report`]
//! only transforms the records it is given.

use chrono::{NaiveDate, Utc};
use std::collections::{BTreeMap, BTreeSet};

use crate::analyze::{mean, std_dev};
use crate::error::Result;
use crate::schema::{
    Annotation, Arm, Direction, Experiment, ExperimentDay, Finding, LabReport, MetricKind,
    OutcomeResult, OutcomeSpec,
};
use crate::store::Store;

type DailySeries = BTreeMap<NaiveDate, f64>;

/// Immutable inputs for a descriptive N=1 report.
struct LabReportInput {
    experiment: Experiment,
    days: Vec<ExperimentDay>,
    /// Daily values for outcome kinds, already loaded by the caller.
    series: BTreeMap<MetricKind, DailySeries>,
    annotations: Vec<Annotation>,
    findings: Vec<Finding>,
    /// Used only when the experiment has no arm days and no `started_on`.
    today: NaiveDate,
}

/// Load rows and the clock, then score the experiment with [`assemble_lab_report`].
pub fn build_lab_report(store: &Store, slug: &str) -> Result<LabReport> {
    let experiment = store.require_experiment(slug)?;
    let days = store.experiment_days(experiment.id)?;
    let today = Utc::now().date_naive();
    let (start, end) = report_window(&experiment, &days, today);
    let input = LabReportInput {
        series: load_series(store, &experiment.outcomes, &days)?,
        annotations: store.annotations_in_range(start, end)?,
        findings: store.findings_in_range(start, end)?,
        experiment,
        days,
        today,
    };
    Ok(assemble_lab_report(&input))
}

fn load_series(
    store: &Store,
    outcomes: &[OutcomeSpec],
    days: &[ExperimentDay],
) -> Result<BTreeMap<MetricKind, DailySeries>> {
    let intervention = arm_days(days, Arm::Intervention);
    let control = arm_days(days, Arm::Control);
    let mut dates = intervention.iter().chain(control.iter()).copied();
    let Some(first) = dates.next() else {
        return Ok(BTreeMap::new());
    };
    let (start, end) = dates.fold((first, first), |(lo, hi), d| (lo.min(d), hi.max(d)));

    let mut series = BTreeMap::new();
    let kinds: BTreeSet<MetricKind> = outcomes.iter().map(|o| o.kind.clone()).collect();
    for kind in kinds {
        let rows = store.metric_series(&kind, start, end)?;
        series.insert(kind, rows.into_iter().collect());
    }
    Ok(series)
}

fn assemble_lab_report(input: &LabReportInput) -> LabReport {
    let intervention_days = arm_days(&input.days, Arm::Intervention);
    let control_days = arm_days(&input.days, Arm::Control);
    let window = report_window(&input.experiment, &input.days, input.today);

    let empty_series = DailySeries::new();
    let outcomes: Vec<OutcomeResult> = input
        .experiment
        .outcomes
        .iter()
        .map(|spec| {
            let series = input.series.get(&spec.kind).unwrap_or(&empty_series);
            compute_outcome(spec, series, &intervention_days, &control_days)
        })
        .collect();

    let confounds = detect_confounds(&input.annotations, &intervention_days, &control_days);
    let findings_overlap = input
        .findings
        .iter()
        .filter(|f| intervention_days.contains(&f.day) || control_days.contains(&f.day))
        .cloned()
        .collect();

    let summary = local_lab_summary(
        &input.experiment,
        intervention_days.len(),
        control_days.len(),
        &outcomes,
        &confounds,
    );

    LabReport {
        experiment: input.experiment.clone(),
        window,
        n_intervention: intervention_days.len(),
        n_control: control_days.len(),
        outcomes,
        confounds,
        findings_overlap,
        summary,
        llm_narrative: None,
        research_refs: vec![],
    }
}

fn arm_days(days: &[ExperimentDay], arm: Arm) -> BTreeSet<NaiveDate> {
    days.iter()
        .filter(|d| d.arm == arm)
        .map(|d| d.day)
        .collect()
}

fn report_window(
    experiment: &Experiment,
    days: &[ExperimentDay],
    today: NaiveDate,
) -> (NaiveDate, NaiveDate) {
    if let (Some(min), Some(max)) = (
        days.iter().map(|d| d.day).min(),
        days.iter().map(|d| d.day).max(),
    ) {
        return (min, max);
    }
    let start = experiment.started_on.unwrap_or(today);
    let end = experiment.ended_on.unwrap_or(start);
    (start, end)
}

fn compute_outcome(
    spec: &OutcomeSpec,
    series: &DailySeries,
    intervention: &BTreeSet<NaiveDate>,
    control: &BTreeSet<NaiveDate>,
) -> OutcomeResult {
    if intervention.is_empty() && control.is_empty() {
        return empty_outcome(spec);
    }

    let iv: Vec<f64> = intervention
        .iter()
        .filter_map(|d| series.get(d).copied())
        .collect();
    let cv: Vec<f64> = control
        .iter()
        .filter_map(|d| series.get(d).copied())
        .collect();

    let mean_i = mean(&iv);
    let mean_c = mean(&cv);
    let med_i = median_opt(&iv);
    let med_c = median_opt(&cv);
    let delta = match (mean_i, mean_c) {
        (Some(a), Some(b)) => Some(a - b),
        _ => None,
    };
    let effect_size = if iv.len() >= 5 && cv.len() >= 5 {
        cohens_d(&iv, &cv)
    } else {
        None
    };

    OutcomeResult {
        kind: spec.kind.clone(),
        direction: spec.direction,
        primary: spec.primary,
        n_intervention: iv.len(),
        n_control: cv.len(),
        mean_intervention: mean_i,
        mean_control: mean_c,
        median_intervention: med_i,
        median_control: med_c,
        delta,
        effect_size,
    }
}

fn empty_outcome(spec: &OutcomeSpec) -> OutcomeResult {
    OutcomeResult {
        kind: spec.kind.clone(),
        direction: spec.direction,
        primary: spec.primary,
        n_intervention: 0,
        n_control: 0,
        mean_intervention: None,
        mean_control: None,
        median_intervention: None,
        median_control: None,
        delta: None,
        effect_size: None,
    }
}

fn median_opt(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let mut v = xs.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = v.len() / 2;
    if v.len() % 2 == 0 {
        Some((v[mid - 1] + v[mid]) / 2.0)
    } else {
        Some(v[mid])
    }
}

/// Pooled-SD Cohen's d (descriptive only).
fn cohens_d(a: &[f64], b: &[f64]) -> Option<f64> {
    let ma = mean(a)?;
    let mb = mean(b)?;
    let sa = std_dev(a)?;
    let sb = std_dev(b)?;
    let na = a.len() as f64;
    let nb = b.len() as f64;
    let pooled = (((na - 1.0) * sa * sa + (nb - 1.0) * sb * sb) / (na + nb - 2.0)).sqrt();
    if pooled < 1e-12 {
        return None;
    }
    Some((ma - mb) / pooled)
}

fn detect_confounds(
    annotations: &[Annotation],
    intervention: &BTreeSet<NaiveDate>,
    control: &BTreeSet<NaiveDate>,
) -> Vec<String> {
    let confound_tags = ["sick", "alcohol", "travel", "stress_high", "night_shift"];
    let mut out = Vec::new();
    for tag in confound_tags {
        let on_iv: usize = annotations
            .iter()
            .filter(|a| intervention.contains(&a.day) && a.tags.iter().any(|t| t == tag))
            .count();
        let on_c: usize = annotations
            .iter()
            .filter(|a| control.contains(&a.day) && a.tags.iter().any(|t| t == tag))
            .count();
        if on_iv > 0 || on_c > 0 {
            out.push(format!(
                "tag `{tag}` on {on_iv} intervention day(s) and {on_c} control day(s)"
            ));
        }
    }
    out
}

fn local_lab_summary(
    experiment: &Experiment,
    n_iv: usize,
    n_c: usize,
    outcomes: &[OutcomeResult],
    confounds: &[String],
) -> String {
    let mut lines = vec![format!(
        "Lab `{}` ({}) — {} intervention day(s), {} control day(s). Hypothesis: {}",
        experiment.slug, experiment.status, n_iv, n_c, experiment.hypothesis
    )];

    if n_iv == 0 || n_c == 0 {
        lines.push("Need both intervention and control arm days before comparing outcomes.".into());
    }

    for o in outcomes {
        let flag = if o.primary { " [primary]" } else { "" };
        let delta_s = o
            .delta
            .map(|d| format!("{d:+.2}"))
            .unwrap_or_else(|| "n/a".into());
        let mean_i = o
            .mean_intervention
            .map(|v| format!("{v:.2}"))
            .unwrap_or_else(|| "—".into());
        let mean_c = o
            .mean_control
            .map(|v| format!("{v:.2}"))
            .unwrap_or_else(|| "—".into());
        let dir_hint = direction_hint(o);
        lines.push(format!(
            "- {}{flag}: mean intervention={mean_i} (n={}), control={mean_c} (n={}), Δ={delta_s}{}{}",
            o.kind.as_str(),
            o.n_intervention,
            o.n_control,
            o.effect_size
                .map(|e| format!(", d≈{e:.2}"))
                .unwrap_or_default(),
            dir_hint
        ));
    }

    if !confounds.is_empty() {
        lines.push("Possible confounds:".into());
        for c in confounds {
            lines.push(format!("- {c}"));
        }
    }

    if n_iv + n_c < experiment.min_days as usize {
        lines.push(format!(
            "Below min_days target ({}): keep logging arm days for a more stable personal read.",
            experiment.min_days
        ));
    }

    lines.push("Descriptive N=1 only — not a clinical trial; not medical advice.".into());
    lines.join("\n")
}

/// How an observed mean difference sits against the hypothesized direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectionRead {
    Consistent,
    Opposite,
    LittleDifference,
    NoDelta,
}

fn direction_read(direction: Direction, delta: Option<f64>) -> DirectionRead {
    let Some(delta) = delta else {
        return DirectionRead::NoDelta;
    };
    let aligned = match direction {
        Direction::Up => delta > 0.0,
        Direction::Down => delta < 0.0,
        Direction::Change => delta.abs() > 0.0,
    };
    if aligned {
        DirectionRead::Consistent
    } else if delta == 0.0 {
        DirectionRead::LittleDifference
    } else {
        DirectionRead::Opposite
    }
}

fn direction_hint(o: &OutcomeResult) -> String {
    match direction_read(o.direction, o.delta) {
        DirectionRead::Consistent => " — directionally consistent with hypothesis".into(),
        DirectionRead::LittleDifference => " — little difference".into(),
        DirectionRead::Opposite => " — opposite of hypothesized direction".into(),
        DirectionRead::NoDelta => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{ExperimentStatus, MetricKind, MetricPoint, OutcomeSpec, Severity};
    use chrono::{DateTime, TimeZone, Utc};
    use tempfile::TempDir;
    use uuid::Uuid;

    fn day(offset: i64) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 7, 1).unwrap() + chrono::Duration::days(offset)
    }

    fn fixed_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
    }

    fn experiment(
        outcomes: Vec<OutcomeSpec>,
        min_days: u32,
        started_on: Option<NaiveDate>,
    ) -> Experiment {
        let now = fixed_now();
        Experiment {
            id: Uuid::nil(),
            slug: "hrv_test".into(),
            title: "HRV test".into(),
            hypothesis: "Intervention raises HRV".into(),
            status: ExperimentStatus::Active,
            started_on,
            ended_on: None,
            outcomes,
            min_days,
            notes: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn exp_day(offset: i64, arm: Arm) -> ExperimentDay {
        ExperimentDay {
            experiment_id: Uuid::nil(),
            day: day(offset),
            arm,
            note: None,
        }
    }

    fn annotation(offset: i64, tag: &str) -> Annotation {
        Annotation {
            id: Uuid::nil(),
            day: day(offset),
            recorded_at: fixed_now(),
            tags: vec![tag.into()],
            body: None,
            mood: None,
            energy: None,
            experiment_id: None,
            source: "manual".into(),
        }
    }

    fn finding(offset: i64, id: u128) -> Finding {
        Finding {
            id: Uuid::from_u128(id),
            day: day(offset),
            kind: MetricKind::HeartRateVariabilityMs,
            severity: Severity::Medium,
            title: "HRV drop".into(),
            detail: "test".into(),
            value: Some(30.0),
            baseline: Some(50.0),
            rule_id: "hrv_drop".into(),
        }
    }

    fn hrv_up() -> OutcomeSpec {
        OutcomeSpec {
            kind: MetricKind::HeartRateVariabilityMs,
            direction: Direction::Up,
            primary: true,
        }
    }

    #[test]
    fn assemble_lab_report_is_deterministic_and_scores_arms() {
        let mut series = BTreeMap::new();
        let mut hrv = BTreeMap::new();
        for i in 0..5 {
            hrv.insert(day(i), 40.0 + (i as f64) * 2.0);
        }
        for i in 5..10 {
            hrv.insert(day(i), 60.0 + ((i - 5) as f64) * 2.0);
        }
        series.insert(MetricKind::HeartRateVariabilityMs, hrv);

        let mut days = Vec::new();
        for i in 0..5 {
            days.push(exp_day(i, Arm::Control));
        }
        for i in 5..11 {
            days.push(exp_day(i, Arm::Intervention));
        }
        days.push(exp_day(11, Arm::Exclude));

        let input = LabReportInput {
            experiment: experiment(vec![hrv_up()], 20, Some(day(0))),
            days,
            series,
            annotations: vec![annotation(0, "sick"), annotation(5, "alcohol")],
            findings: vec![finding(0, 1), finding(10, 2), finding(11, 3)],
            today: NaiveDate::from_ymd_opt(1999, 3, 4).unwrap(),
        };

        let once = assemble_lab_report(&input);
        let twice = assemble_lab_report(&input);
        assert_eq!(once.summary, twice.summary);
        assert_eq!(once.window, twice.window);
        assert_eq!(once.confounds, twice.confounds);
        assert_eq!(once.n_intervention, twice.n_intervention);
        assert_eq!(once.n_control, twice.n_control);

        assert_eq!(once.window, (day(0), day(11)));
        assert_eq!(once.n_intervention, 6);
        assert_eq!(once.n_control, 5);
        let o = &once.outcomes[0];
        assert_eq!(o.n_intervention, 5);
        assert_eq!(o.n_control, 5);
        assert!((o.mean_intervention.unwrap() - 64.0).abs() < 1e-9);
        assert!((o.mean_control.unwrap() - 44.0).abs() < 1e-9);
        assert!((o.median_intervention.unwrap() - 64.0).abs() < 1e-9);
        assert!((o.median_control.unwrap() - 44.0).abs() < 1e-9);
        assert!((o.delta.unwrap() - 20.0).abs() < 1e-9);
        let expected_d = 20.0 / 10.0_f64.sqrt();
        assert!((o.effect_size.unwrap() - expected_d).abs() < 1e-9);
        assert_eq!(o.effect_size, twice.outcomes[0].effect_size);

        assert_eq!(
            once.confounds,
            vec![
                "tag `sick` on 0 intervention day(s) and 1 control day(s)".to_string(),
                "tag `alcohol` on 1 intervention day(s) and 0 control day(s)".to_string(),
            ]
        );
        let overlap: Vec<u128> = once
            .findings_overlap
            .iter()
            .map(|f| f.id.as_u128())
            .collect();
        assert_eq!(overlap, vec![1, 2]);
        assert!(once.summary.contains("directionally consistent"));
        assert!(once.summary.contains("[primary]"));
        assert!(once.summary.contains("Below min_days target (20)"));
        assert!(once.summary.contains("Descriptive N=1 only"));
    }

    #[test]
    fn empty_window_uses_injected_today_not_the_clock() {
        let today = NaiveDate::from_ymd_opt(1999, 3, 4).unwrap();
        let input = LabReportInput {
            experiment: experiment(vec![hrv_up()], 14, None),
            days: vec![],
            series: BTreeMap::new(),
            annotations: vec![],
            findings: vec![],
            today,
        };
        let report = assemble_lab_report(&input);
        assert_eq!(report.window, (today, today));
        let started = NaiveDate::from_ymd_opt(2024, 2, 1).unwrap();
        let ended = NaiveDate::from_ymd_opt(2024, 2, 10).unwrap();
        let mut bounded = input.experiment.clone();
        bounded.started_on = Some(started);
        bounded.ended_on = Some(ended);
        let bounded_report = assemble_lab_report(&LabReportInput {
            experiment: bounded,
            days: vec![],
            series: BTreeMap::new(),
            annotations: vec![],
            findings: vec![],
            today,
        });
        assert_eq!(bounded_report.window, (started, ended));
        assert_eq!(report.n_intervention, 0);
        assert_eq!(report.n_control, 0);
        assert!(report.outcomes[0].delta.is_none());
        assert!(report.outcomes[0].effect_size.is_none());
        assert!(report
            .summary
            .contains("Need both intervention and control arm days"));
        assert!(!report.summary.contains("directionally"));
        assert_eq!(report.summary, assemble_lab_report(&input).summary);
    }

    #[test]
    fn effect_size_requires_five_per_arm_and_nonzero_spread() {
        let spec = hrv_up();
        let mut thin = BTreeMap::new();
        for i in 0..4 {
            thin.insert(day(i), 10.0 + i as f64);
            thin.insert(day(10 + i), 20.0 + i as f64);
        }
        let iv: BTreeSet<_> = (0..4).map(day).collect();
        let cv: BTreeSet<_> = (10..14).map(day).collect();
        let thin_result = compute_outcome(&spec, &thin, &iv, &cv);
        assert!(thin_result.delta.is_some());
        assert!(thin_result.effect_size.is_none());

        let mut flat = BTreeMap::new();
        let iv: BTreeSet<_> = (0..5).map(day).collect();
        let cv: BTreeSet<_> = (10..15).map(day).collect();
        for d in &iv {
            flat.insert(*d, 10.0);
        }
        for d in &cv {
            flat.insert(*d, 30.0);
        }
        let flat_result = compute_outcome(&spec, &flat, &iv, &cv);
        assert!((flat_result.delta.unwrap() - (10.0 - 30.0)).abs() < 1e-9);
        assert!(flat_result.effect_size.is_none());
    }

    #[test]
    fn direction_read_classifies_delta() {
        assert_eq!(
            direction_read(Direction::Up, Some(1.0)),
            DirectionRead::Consistent
        );
        assert_eq!(
            direction_read(Direction::Up, Some(-1.0)),
            DirectionRead::Opposite
        );
        assert_eq!(
            direction_read(Direction::Up, Some(0.0)),
            DirectionRead::LittleDifference
        );
        assert_eq!(
            direction_read(Direction::Down, Some(-0.5)),
            DirectionRead::Consistent
        );
        assert_eq!(
            direction_read(Direction::Down, Some(0.5)),
            DirectionRead::Opposite
        );
        assert_eq!(
            direction_read(Direction::Change, Some(0.0)),
            DirectionRead::LittleDifference
        );
        assert_eq!(
            direction_read(Direction::Change, Some(2.0)),
            DirectionRead::Consistent
        );
        assert_eq!(direction_read(Direction::Up, None), DirectionRead::NoDelta);

        let opposite = OutcomeResult {
            kind: MetricKind::HeartRateVariabilityMs,
            direction: Direction::Up,
            primary: false,
            n_intervention: 1,
            n_control: 1,
            mean_intervention: Some(10.0),
            mean_control: Some(20.0),
            median_intervention: Some(10.0),
            median_control: Some(20.0),
            delta: Some(-10.0),
            effect_size: None,
        };
        assert_eq!(
            direction_hint(&opposite),
            " — opposite of hypothesized direction"
        );
        let flat = OutcomeResult {
            delta: Some(0.0),
            mean_intervention: Some(5.0),
            mean_control: Some(5.0),
            ..opposite.clone()
        };
        assert_eq!(direction_hint(&flat), " — little difference");
    }

    #[test]
    fn lab_report_delta() {
        let dir = TempDir::new().unwrap();
        let store = Store::open(dir.path().join("t.db")).unwrap();
        let start = NaiveDate::from_ymd_opt(2026, 7, 1).unwrap();

        // Control days: lower HRV
        for i in 0..5 {
            let day = start + chrono::Duration::days(i);
            store
                .upsert_metrics(&[MetricPoint::new(
                    MetricKind::HeartRateVariabilityMs,
                    Utc::now(),
                    day,
                    40.0 + i as f64,
                    "ms",
                    "test",
                )])
                .unwrap();
        }
        // Intervention: higher HRV
        for i in 5..10 {
            let day = start + chrono::Duration::days(i);
            store
                .upsert_metrics(&[MetricPoint::new(
                    MetricKind::HeartRateVariabilityMs,
                    Utc::now(),
                    day,
                    55.0 + i as f64,
                    "ms",
                    "test",
                )])
                .unwrap();
        }

        let now = Utc::now();
        let exp = Experiment {
            id: Uuid::new_v4(),
            slug: "hrv_test".into(),
            title: "HRV test".into(),
            hypothesis: "Intervention raises HRV".into(),
            status: ExperimentStatus::Active,
            started_on: Some(start),
            ended_on: None,
            outcomes: vec![OutcomeSpec {
                kind: MetricKind::HeartRateVariabilityMs,
                direction: Direction::Up,
                primary: true,
            }],
            min_days: 10,
            notes: None,
            created_at: now,
            updated_at: now,
        };
        store.insert_experiment(&exp).unwrap();
        for i in 0..5 {
            store
                .upsert_experiment_day(&ExperimentDay {
                    experiment_id: exp.id,
                    day: start + chrono::Duration::days(i),
                    arm: Arm::Control,
                    note: None,
                })
                .unwrap();
        }
        for i in 5..10 {
            store
                .upsert_experiment_day(&ExperimentDay {
                    experiment_id: exp.id,
                    day: start + chrono::Duration::days(i),
                    arm: Arm::Intervention,
                    note: None,
                })
                .unwrap();
        }

        let report = build_lab_report(&store, "hrv_test").unwrap();
        assert_eq!(report.n_intervention, 5);
        assert_eq!(report.n_control, 5);
        let o = &report.outcomes[0];
        assert!(o.delta.unwrap() > 0.0);
        assert!(o.effect_size.is_some());
    }
}
