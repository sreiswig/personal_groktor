//! N=1 lab: experiment lifecycle helpers and descriptive outcome analysis.
//!
//! Planning (`parse_outcome_spec`, `select_primary`, `plan_new_experiment`,
//! `plan_status`, `inclusive_days`) is pure. Store reads and writes, ids, and
//! the clock stay in the public command helpers.

mod analyze;

pub use analyze::build_lab_report;

use chrono::{DateTime, NaiveDate, Utc};
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

/// Chosen primary outcome. The first spec is primary when `primary` is missing or unknown.
fn select_primary(outcomes: &[OutcomeSpec], primary: Option<&str>) -> Vec<OutcomeSpec> {
    if outcomes.is_empty() {
        return Vec::new();
    }
    let mut chosen = 0usize;
    if let Some(p) = primary {
        let want = MetricKind::from_raw_name(p);
        if let Some(i) = outcomes.iter().position(|o| o.kind == want) {
            chosen = i;
        }
    }
    outcomes
        .iter()
        .enumerate()
        .map(|(i, o)| OutcomeSpec {
            kind: o.kind.clone(),
            direction: o.direction,
            primary: i == chosen,
        })
        .collect()
}

/// Mark the first matching outcome as primary (by kind string / alias).
pub fn mark_primary(outcomes: &mut [OutcomeSpec], primary: Option<&str>) {
    let selected = select_primary(outcomes, primary);
    for (slot, spec) in outcomes.iter_mut().zip(selected) {
        *slot = spec;
    }
}

/// Fields the caller already collected. Id and clock are supplied separately.
struct ExperimentDraft {
    slug: String,
    title: String,
    hypothesis: String,
    outcomes: Vec<OutcomeSpec>,
    primary: Option<String>,
    min_days: u32,
    started_on: Option<NaiveDate>,
    notes: Option<String>,
}

fn plan_new_experiment(draft: ExperimentDraft, id: Uuid, now: DateTime<Utc>) -> Result<Experiment> {
    if draft.outcomes.is_empty() {
        return Err(GroktorError::Message(
            "at least one --outcome is required (e.g. hrv:up)".into(),
        ));
    }
    let outcomes = select_primary(&draft.outcomes, draft.primary.as_deref());
    Ok(Experiment {
        id,
        slug: draft.slug,
        title: draft.title,
        hypothesis: draft.hypothesis,
        status: ExperimentStatus::Active,
        started_on: Some(draft.started_on.unwrap_or_else(|| now.date_naive())),
        ended_on: None,
        outcomes,
        min_days: draft.min_days,
        notes: draft.notes,
        created_at: now,
        updated_at: now,
    })
}

/// Apply a status change without reading the clock or the database.
fn plan_status(exp: Experiment, status: ExperimentStatus, now: DateTime<Utc>) -> Experiment {
    let today = now.date_naive();
    let ended_on = if matches!(
        status,
        ExperimentStatus::Completed | ExperimentStatus::Abandoned
    ) && exp.ended_on.is_none()
    {
        Some(today)
    } else {
        exp.ended_on
    };
    let started_on = if status == ExperimentStatus::Active && exp.started_on.is_none() {
        Some(today)
    } else {
        exp.started_on
    };
    Experiment {
        status,
        updated_at: now,
        ended_on,
        started_on,
        ..exp
    }
}

/// Inclusive calendar span. `to` before `from` is a typed failure.
fn inclusive_days(from: NaiveDate, to: NaiveDate) -> Result<Vec<NaiveDate>> {
    if to < from {
        return Err(GroktorError::Message(
            "--to must be on or after --from".into(),
        ));
    }
    let mut days = Vec::new();
    let mut d = from;
    while d <= to {
        days.push(d);
        d += chrono::Duration::days(1);
    }
    Ok(days)
}

#[allow(clippy::too_many_arguments)]
pub fn start_experiment(
    store: &Store,
    slug: &str,
    title: &str,
    hypothesis: &str,
    outcomes: Vec<OutcomeSpec>,
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
    let exp = plan_new_experiment(
        ExperimentDraft {
            slug: slug.to_string(),
            title: title.to_string(),
            hypothesis: hypothesis.to_string(),
            outcomes,
            primary: primary.map(str::to_string),
            min_days,
            started_on,
            notes,
        },
        Uuid::new_v4(),
        Utc::now(),
    )?;
    store.insert_experiment(&exp)?;
    Ok(exp)
}

pub fn set_status(store: &Store, slug: &str, status: ExperimentStatus) -> Result<Experiment> {
    let exp = store.require_experiment(slug)?;
    let exp = plan_status(exp, status, Utc::now());
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
    let days = inclusive_days(from, to)?;
    let exp = store.require_experiment(slug)?;
    for day in &days {
        store.upsert_experiment_day(&ExperimentDay {
            experiment_id: exp.id,
            day: *day,
            arm,
            note: note.clone(),
        })?;
    }
    Ok(days.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 15, 18, 30, 0).unwrap()
    }

    fn spec(kind: MetricKind, direction: Direction) -> OutcomeSpec {
        OutcomeSpec {
            kind,
            direction,
            primary: false,
        }
    }

    fn bare_experiment() -> Experiment {
        let stamp = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        Experiment {
            id: Uuid::nil(),
            slug: "early_bed".into(),
            title: "Earlier bedtime".into(),
            hypothesis: "Bed by 22:30 raises HRV".into(),
            status: ExperimentStatus::Active,
            started_on: None,
            ended_on: None,
            outcomes: vec![spec(MetricKind::HeartRateVariabilityMs, Direction::Up)],
            min_days: 14,
            notes: None,
            created_at: stamp,
            updated_at: stamp,
        }
    }

    #[test]
    fn parse_outcome_spec_accepts_aliases_and_rejects_unknown_direction() {
        let ok = parse_outcome_spec("  hrv:up ").unwrap();
        assert_eq!(ok.kind, MetricKind::HeartRateVariabilityMs);
        assert_eq!(ok.direction, Direction::Up);
        assert!(!ok.primary);

        let bare = parse_outcome_spec("steps").unwrap();
        assert_eq!(bare.kind, MetricKind::Steps);
        assert_eq!(bare.direction, Direction::Change);

        let err = parse_outcome_spec("hrv:sideways").unwrap_err();
        assert!(matches!(err, GroktorError::Parse(_)));
        assert!(err.to_string().contains("unknown direction"));
        assert!(err.to_string().contains("hrv:sideways"));
    }

    #[test]
    fn select_primary_is_stable_and_falls_back_to_first() {
        let outcomes = vec![
            spec(MetricKind::Steps, Direction::Up),
            spec(MetricKind::RestingHeartRateBpm, Direction::Down),
        ];
        let marked = select_primary(&outcomes, Some("RHR"));
        assert!(!marked[0].primary);
        assert!(marked[1].primary);
        assert_eq!(marked[1].kind, MetricKind::RestingHeartRateBpm);
        let again = select_primary(&outcomes, Some("RHR"));
        assert!(specs_eq(&again, &marked));

        let fallback = select_primary(&outcomes, Some("not-a-metric"));
        assert!(fallback[0].primary);
        assert!(!fallback[1].primary);

        let mut in_place = outcomes.clone();
        mark_primary(&mut in_place, Some("rhr"));
        assert!(specs_eq(&in_place, &marked));
        assert!(select_primary(&[], None).is_empty());
    }

    fn specs_eq(a: &[OutcomeSpec], b: &[OutcomeSpec]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|(x, y)| {
                x.kind == y.kind && x.direction == y.direction && x.primary == y.primary
            })
    }

    #[test]
    fn plan_new_experiment_rejects_empty_outcomes_and_stamps_supplied_clock() {
        let id = Uuid::from_u128(7);
        let stamp = now();
        let err = plan_new_experiment(
            ExperimentDraft {
                slug: "early_bed".into(),
                title: "Earlier bedtime".into(),
                hypothesis: "HRV up".into(),
                outcomes: vec![],
                primary: None,
                min_days: 14,
                started_on: None,
                notes: None,
            },
            id,
            stamp,
        )
        .unwrap_err();
        assert!(matches!(err, GroktorError::Message(_)));
        assert!(err.to_string().contains("at least one --outcome"));

        let draft = ExperimentDraft {
            slug: "early_bed".into(),
            title: "Earlier bedtime".into(),
            hypothesis: "HRV up".into(),
            outcomes: vec![
                spec(MetricKind::HeartRateVariabilityMs, Direction::Up),
                spec(MetricKind::SleepDurationHours, Direction::Up),
            ],
            primary: Some("sleep_hours".into()),
            min_days: 14,
            started_on: None,
            notes: Some("pilot".into()),
        };
        let exp = plan_new_experiment(
            ExperimentDraft {
                slug: draft.slug.clone(),
                title: draft.title.clone(),
                hypothesis: draft.hypothesis.clone(),
                outcomes: draft.outcomes.clone(),
                primary: draft.primary.clone(),
                min_days: draft.min_days,
                started_on: draft.started_on,
                notes: draft.notes.clone(),
            },
            id,
            stamp,
        )
        .unwrap();
        let again = plan_new_experiment(draft, id, stamp).unwrap();
        assert_eq!(exp.id, again.id);
        assert_eq!(exp.created_at, again.created_at);
        assert_eq!(exp.started_on, again.started_on);
        assert_eq!(exp.started_on, Some(stamp.date_naive()));
        assert_eq!(exp.status, ExperimentStatus::Active);
        assert!(exp.outcomes[1].primary);
        assert!(!exp.outcomes[0].primary);
        assert_eq!(exp.notes.as_deref(), Some("pilot"));

        let explicit = plan_new_experiment(
            ExperimentDraft {
                slug: "early_bed".into(),
                title: "Earlier bedtime".into(),
                hypothesis: "HRV up".into(),
                outcomes: vec![spec(MetricKind::Steps, Direction::Change)],
                primary: None,
                min_days: 7,
                started_on: Some(day(2020, 1, 2)),
                notes: None,
            },
            id,
            stamp,
        )
        .unwrap();
        assert_eq!(explicit.started_on, Some(day(2020, 1, 2)));
    }

    #[test]
    fn plan_status_uses_injected_instant_and_does_not_overwrite_bounds() {
        let stamp = now();
        let today = stamp.date_naive();
        let started = plan_status(bare_experiment(), ExperimentStatus::Active, stamp);
        assert_eq!(started.started_on, Some(today));
        assert_eq!(started.updated_at, stamp);
        assert!(started.ended_on.is_none());

        let paused = plan_status(bare_experiment(), ExperimentStatus::Paused, stamp);
        assert!(paused.started_on.is_none());
        assert!(paused.ended_on.is_none());
        assert_eq!(paused.status, ExperimentStatus::Paused);

        let completed = plan_status(bare_experiment(), ExperimentStatus::Completed, stamp);
        assert_eq!(completed.ended_on, Some(today));
        let kept = plan_status(
            Experiment {
                ended_on: Some(day(2024, 5, 1)),
                ..completed.clone()
            },
            ExperimentStatus::Abandoned,
            stamp,
        );
        assert_eq!(kept.ended_on, Some(day(2024, 5, 1)));
        assert_eq!(kept.status, ExperimentStatus::Abandoned);
        assert_eq!(kept.updated_at, stamp);

        let already = plan_status(
            Experiment {
                started_on: Some(day(2024, 1, 1)),
                ..bare_experiment()
            },
            ExperimentStatus::Active,
            stamp,
        );
        assert_eq!(already.started_on, Some(day(2024, 1, 1)));
    }

    #[test]
    fn inclusive_days_rejects_inverted_span() {
        let from = day(2026, 7, 20);
        let to = day(2026, 7, 22);
        let days = inclusive_days(from, to).unwrap();
        assert_eq!(days, vec![from, day(2026, 7, 21), to]);
        assert_eq!(inclusive_days(from, from).unwrap(), vec![from]);

        let err = inclusive_days(to, from).unwrap_err();
        assert!(matches!(err, GroktorError::Message(_)));
        assert!(err.to_string().contains("--to must be on or after --from"));
    }
}
