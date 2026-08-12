//! Descriptive N=1 stats for lab reports.

use chrono::NaiveDate;
use std::collections::{BTreeMap, BTreeSet};

use crate::analyze::{mean, std_dev};
use crate::error::Result;
use crate::schema::{
    Annotation, Arm, Experiment, ExperimentDay, LabReport, OutcomeResult, OutcomeSpec,
};
use crate::store::Store;

/// Build a local lab report (no research / no LLM).
pub fn build_lab_report(store: &Store, slug: &str) -> Result<LabReport> {
    let experiment = store.require_experiment(slug)?;
    let days = store.experiment_days(experiment.id)?;
    build_from_parts(store, experiment, &days)
}

fn build_from_parts(
    store: &Store,
    experiment: Experiment,
    days: &[ExperimentDay],
) -> Result<LabReport> {
    let intervention_days: BTreeSet<NaiveDate> = days
        .iter()
        .filter(|d| d.arm == Arm::Intervention)
        .map(|d| d.day)
        .collect();
    let control_days: BTreeSet<NaiveDate> = days
        .iter()
        .filter(|d| d.arm == Arm::Control)
        .map(|d| d.day)
        .collect();

    let window = resolve_window(&experiment, days);
    let (win_start, win_end) = window;

    let mut outcomes = Vec::new();
    for spec in &experiment.outcomes {
        outcomes.push(compute_outcome(
            store,
            spec,
            &intervention_days,
            &control_days,
        )?);
    }

    let annotations = store.annotations_in_range(win_start, win_end)?;
    let confounds = detect_confounds(&annotations, &intervention_days, &control_days);

    let findings_overlap = store
        .findings_in_range(win_start, win_end)?
        .into_iter()
        .filter(|f| intervention_days.contains(&f.day) || control_days.contains(&f.day))
        .collect();

    let summary = local_lab_summary(
        &experiment,
        intervention_days.len(),
        control_days.len(),
        &outcomes,
        &confounds,
    );

    Ok(LabReport {
        experiment,
        window,
        n_intervention: intervention_days.len(),
        n_control: control_days.len(),
        outcomes,
        confounds,
        findings_overlap,
        summary,
        llm_narrative: None,
        research_refs: vec![],
    })
}

fn resolve_window(experiment: &Experiment, days: &[ExperimentDay]) -> (NaiveDate, NaiveDate) {
    if let (Some(min), Some(max)) = (
        days.iter().map(|d| d.day).min(),
        days.iter().map(|d| d.day).max(),
    ) {
        return (min, max);
    }
    let start = experiment
        .started_on
        .unwrap_or_else(|| chrono::Utc::now().date_naive());
    let end = experiment.ended_on.unwrap_or(start);
    (start, end)
}

fn compute_outcome(
    store: &Store,
    spec: &OutcomeSpec,
    intervention: &BTreeSet<NaiveDate>,
    control: &BTreeSet<NaiveDate>,
) -> Result<OutcomeResult> {
    let all_days: Vec<NaiveDate> = intervention.iter().chain(control.iter()).copied().collect();
    let (start, end) = match (all_days.iter().min(), all_days.iter().max()) {
        (Some(a), Some(b)) => (*a, *b),
        _ => {
            return Ok(empty_outcome(spec));
        }
    };

    let series = store.metric_series(&spec.kind, start, end)?;
    let series_map: BTreeMap<NaiveDate, f64> = series.into_iter().collect();

    let iv: Vec<f64> = intervention
        .iter()
        .filter_map(|d| series_map.get(d).copied())
        .collect();
    let cv: Vec<f64> = control
        .iter()
        .filter_map(|d| series_map.get(d).copied())
        .collect();

    let mean_i = mean_opt(&iv);
    let mean_c = mean_opt(&cv);
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

    Ok(OutcomeResult {
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
    })
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

fn mean_opt(xs: &[f64]) -> Option<f64> {
    mean(xs)
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

fn direction_hint(o: &OutcomeResult) -> String {
    let Some(delta) = o.delta else {
        return String::new();
    };
    let aligned = match o.direction {
        crate::schema::Direction::Up => delta > 0.0,
        crate::schema::Direction::Down => delta < 0.0,
        crate::schema::Direction::Change => delta.abs() > 0.0,
    };
    if aligned {
        " — directionally consistent with hypothesis".into()
    } else if delta == 0.0 {
        " — little difference".into()
    } else {
        " — opposite of hypothesized direction".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Direction, ExperimentStatus, MetricKind, MetricPoint, OutcomeSpec};
    use chrono::Utc;
    use tempfile::TempDir;
    use uuid::Uuid;

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
