//! N=1 lab: experiment lifecycle helpers and descriptive outcome analysis.

mod analyze;

pub use analyze::build_lab_report;

use chrono::{NaiveDate, Utc};
use uuid::Uuid;

use crate::error::{GroktorError, Result};
use crate::schema::{
    Arm, Direction, Experiment, ExperimentDay, ExperimentStatus, MetricKind, OutcomeSpec,
};
use crate::store::Store;

/// Parse outcome specs from CLI strings like `hrv:up` or `sleep_duration_hours:down`.
pub fn parse_outcome_spec(s: &str) -> Result<OutcomeSpec> {
    let (kind_s, dir_s) = if let Some((k, d)) = s.split_once(':') {
        (k, d)
    } else {
        (s, "change")
    };
    let kind = MetricKind::from_raw_name(kind_s.trim());
    let direction = Direction::parse(dir_s.trim()).ok_or_else(|| {
        GroktorError::Parse(format!(
            "unknown direction in outcome `{s}` (use up|down|change)"
        ))
    })?;
    Ok(OutcomeSpec {
        kind,
        direction,
        primary: false,
    })
}

/// Mark the first matching outcome as primary (by kind string / alias).
pub fn mark_primary(outcomes: &mut [OutcomeSpec], primary: Option<&str>) {
    if outcomes.is_empty() {
        return;
    }
    for x in outcomes.iter_mut() {
        x.primary = false;
    }
    if let Some(p) = primary {
        let want = MetricKind::from_raw_name(p);
        if let Some(o) = outcomes.iter_mut().find(|o| o.kind == want) {
            o.primary = true;
            return;
        }
    }
    // Default: first outcome is primary.
    outcomes[0].primary = true;
}

#[allow(clippy::too_many_arguments)]
pub fn start_experiment(
    store: &Store,
    slug: &str,
    title: &str,
    hypothesis: &str,
    mut outcomes: Vec<OutcomeSpec>,
    primary: Option<&str>,
    min_days: u32,
    started_on: Option<NaiveDate>,
    notes: Option<String>,
) -> Result<Experiment> {
    if store.experiment_by_slug(slug)?.is_some() {
        return Err(GroktorError::Message(format!(
            "experiment `{slug}` already exists"
        )));
    }
    if outcomes.is_empty() {
        return Err(GroktorError::Message(
            "at least one --outcome is required (e.g. hrv:up)".into(),
        ));
    }
    mark_primary(&mut outcomes, primary);
    let now = Utc::now();
    let exp = Experiment {
        id: Uuid::new_v4(),
        slug: slug.to_string(),
        title: title.to_string(),
        hypothesis: hypothesis.to_string(),
        status: ExperimentStatus::Active,
        started_on: Some(started_on.unwrap_or_else(|| now.date_naive())),
        ended_on: None,
        outcomes,
        min_days,
        notes,
        created_at: now,
        updated_at: now,
    };
    store.insert_experiment(&exp)?;
    Ok(exp)
}

pub fn set_status(store: &Store, slug: &str, status: ExperimentStatus) -> Result<Experiment> {
    let mut exp = store.require_experiment(slug)?;
    exp.status = status;
    exp.updated_at = Utc::now();
    if matches!(
        status,
        ExperimentStatus::Completed | ExperimentStatus::Abandoned
    ) && exp.ended_on.is_none()
    {
        exp.ended_on = Some(Utc::now().date_naive());
    }
    if status == ExperimentStatus::Active && exp.started_on.is_none() {
        exp.started_on = Some(Utc::now().date_naive());
    }
    store.update_experiment(&exp)?;
    Ok(exp)
}

pub fn assign_day(
    store: &Store,
    slug: &str,
    day: NaiveDate,
    arm: Arm,
    note: Option<String>,
) -> Result<ExperimentDay> {
    let exp = store.require_experiment(slug)?;
    let ed = ExperimentDay {
        experiment_id: exp.id,
        day,
        arm,
        note,
    };
    store.upsert_experiment_day(&ed)?;
    Ok(ed)
}

pub fn assign_range(
    store: &Store,
    slug: &str,
    from: NaiveDate,
    to: NaiveDate,
    arm: Arm,
    note: Option<String>,
) -> Result<usize> {
    if to < from {
        return Err(GroktorError::Message(
            "--to must be on or after --from".into(),
        ));
    }
    let exp = store.require_experiment(slug)?;
    let mut n = 0usize;
    let mut d = from;
    while d <= to {
        store.upsert_experiment_day(&ExperimentDay {
            experiment_id: exp.id,
            day: d,
            arm,
            note: note.clone(),
        })?;
        n += 1;
        d += chrono::Duration::days(1);
    }
    Ok(n)
}
