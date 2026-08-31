//! Day/week wellbeing brief builder.

mod confidence;

pub use confidence::score_confidence;

use chrono::{Duration, NaiveDate, Utc};

use crate::analyze;
use crate::error::{GroktorError, Result};
use crate::llm;
use crate::schema::{Annotation, Digest, DigestHorizon, ExperimentBrief, Finding, MetricPoint};
use crate::store::Store;

/// Options for building a brief.
#[derive(Debug, Clone, Default)]
pub struct BriefOptions {
    pub day: Option<NaiveDate>,
    pub week: bool,
    /// When true, recompute even if a cached digest exists.
    pub refresh: bool,
}

/// Resolve target day and build a full Digest (local summary; LLM filled by caller).
pub fn build_brief(store: &Store, opts: &BriefOptions) -> Result<Digest> {
    let day = resolve_target_day(store, opts.day)?;
    let (horizon, range_start, range_end) = if opts.week {
        let start = day - Duration::days(6);
        (DigestHorizon::Week { start }, start, day)
    } else {
        (DigestHorizon::Day, day, day)
    };

    if !opts.refresh {
        if let Some(cached) = store.get_digest(day, horizon.as_str())? {
            return Ok(cached);
        }
    }

    // Baselines need history before the window end.
    let analysis_start = range_start - Duration::days(21);
    let points = store.metrics_in_range(analysis_start, range_end)?;
    let all_findings = analyze::analyze(&points);
    let days = analyze::unique_days(&points);
    store.replace_findings_for_days(&days, &all_findings)?;

    let findings: Vec<Finding> = all_findings
        .into_iter()
        .filter(|f| f.day >= range_start && f.day <= range_end)
        .collect();

    let day_metrics: Vec<MetricPoint> = points
        .iter()
        .filter(|p| p.day >= range_start && p.day <= range_end)
        .cloned()
        .collect();
    let metric_count = day_metrics.len();

    let annotations = store.annotations_in_range(range_start, range_end)?;
    let active_experiments = experiment_briefs(store, day)?;
    let confidence = score_confidence(metric_count, &findings, &annotations, opts.week);

    let summary = local_brief_summary(
        day,
        opts.week,
        metric_count,
        &findings,
        &annotations,
        &active_experiments,
        &confidence,
    );

    Ok(Digest {
        day,
        horizon,
        generated_at: Utc::now(),
        findings,
        annotations,
        active_experiments,
        summary,
        llm_narrative: None,
        llm_backend: None,
        metric_count,
        research_bits: vec![],
        confidence,
    })
}

fn resolve_target_day(store: &Store, requested: Option<NaiveDate>) -> Result<NaiveDate> {
    if let Some(d) = requested {
        return Ok(d);
    }
    store
        .day_range()?
        .map(|(_, max)| max)
        .ok_or_else(|| GroktorError::Message("No metrics in database. Run `ingest` first.".into()))
}

fn experiment_briefs(store: &Store, day: NaiveDate) -> Result<Vec<ExperimentBrief>> {
    let mut out = Vec::new();
    for exp in store.active_experiments()? {
        let arm_today = store.arm_for_day(exp.id, day)?;
        let day_index = exp.started_on.map(|start| {
            let delta = (day - start).num_days();
            if delta >= 0 {
                Some((delta + 1) as u32)
            } else {
                None
            }
        });
        out.push(ExperimentBrief {
            slug: exp.slug,
            title: exp.title,
            day_index: day_index.flatten(),
            arm_today,
        });
    }
    Ok(out)
}

fn local_brief_summary(
    day: NaiveDate,
    week: bool,
    metric_count: usize,
    findings: &[Finding],
    annotations: &[Annotation],
    experiments: &[ExperimentBrief],
    confidence: &crate::schema::DigestConfidence,
) -> String {
    let horizon = if week { "week ending" } else { "day" };
    let mut lines = vec![format!(
        "Wellbeing brief for {horizon} {day}: {metric_count} metric(s); \
         {} finding(s); confidence {}.",
        findings.len(),
        confidence.level
    )];

    if !confidence.reasons.is_empty() {
        lines.push(format!("  ({})", confidence.reasons.join("; ")));
    }

    if findings.is_empty() {
        lines.push(
            "No rule-based flags. Keep an eye on sleep, resting HR, and activity consistency."
                .into(),
        );
    } else {
        let high = findings
            .iter()
            .filter(|f| matches!(f.severity, crate::schema::Severity::High))
            .count();
        let medium = findings
            .iter()
            .filter(|f| matches!(f.severity, crate::schema::Severity::Medium))
            .count();
        lines.push(format!("Flags: {high} high, {medium} medium."));
        for f in findings.iter().take(8) {
            lines.push(format!("- [{}] {}: {}", f.severity, f.title, f.detail));
        }
        if findings.len() > 8 {
            lines.push(format!("- …and {} more", findings.len() - 8));
        }
    }

    if !annotations.is_empty() {
        let tags: Vec<_> = annotations
            .iter()
            .flat_map(|a| a.tags.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        lines.push(format!("Context tags: {}.", tags.join(", ")));
    }

    if !experiments.is_empty() {
        for e in experiments {
            let arm = e
                .arm_today
                .map(|a| a.to_string())
                .unwrap_or_else(|| "no arm logged".into());
            let idx = e.day_index.map(|i| format!(" day {i}")).unwrap_or_default();
            lines.push(format!("Lab: {} ({}){idx} — {arm}.", e.slug, e.title));
        }
    }

    lines.push("Insights are for personal reflection only — not medical advice.".into());
    lines.join("\n")
}

/// Metrics for the brief day (or week window) — used for LLM prompt.
pub fn metrics_for_digest(store: &Store, digest: &Digest) -> Result<Vec<MetricPoint>> {
    let (start, end) = match digest.horizon {
        DigestHorizon::Day => (digest.day, digest.day),
        DigestHorizon::Week { start } => (start, digest.day),
    };
    store.metrics_in_range(start, end)
}

/// Build LLM prompt including annotations and lab status.
pub fn build_brief_prompt(digest: &Digest, day_metrics: &[MetricPoint]) -> String {
    let mut base = llm::build_prompt(day_metrics, &digest.findings);

    if !digest.annotations.is_empty() {
        base.push_str("\nContext annotations:\n");
        for a in &digest.annotations {
            let tags = a.tags.join(", ");
            let body = a.body.as_deref().unwrap_or("");
            let mood = a.mood.map(|m| format!(" mood={m}")).unwrap_or_default();
            let energy = a.energy.map(|e| format!(" energy={e}")).unwrap_or_default();
            base.push_str(&format!(
                "- {}: tags=[{tags}]{mood}{energy} {body}\n",
                a.day
            ));
        }
    }

    if !digest.active_experiments.is_empty() {
        base.push_str("\nActive experiments:\n");
        for e in &digest.active_experiments {
            let arm = e
                .arm_today
                .map(|a| a.to_string())
                .unwrap_or_else(|| "unset".into());
            base.push_str(&format!("- {} — arm today: {arm}\n", e.slug));
        }
    }

    base.push_str(&format!(
        "\nConfidence: {} — {}\n",
        digest.confidence.level,
        digest.confidence.reasons.join("; ")
    ));
    base
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::MetricKind;
    use tempfile::TempDir;

    fn seed_store() -> (TempDir, Store, NaiveDate) {
        let dir = TempDir::new().unwrap();
        let store = Store::open(dir.path().join("t.db")).unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();
        store
            .upsert_metrics(&[MetricPoint::new(
                MetricKind::Steps,
                Utc::now(),
                day,
                8000.0,
                "count",
                "test",
            )])
            .unwrap();
        (dir, store, day)
    }

    #[test]
    fn default_refresh_is_false() {
        assert!(!BriefOptions::default().refresh);
    }

    #[test]
    fn cache_hit_skips_recompute_and_refresh_forces_new() {
        let (_dir, store, day) = seed_store();
        let cached_opts = BriefOptions {
            day: Some(day),
            week: false,
            refresh: false,
        };
        let first = build_brief(&store, &cached_opts).unwrap();
        store.upsert_digest(&first).unwrap();

        // A later metric must not appear on a cache hit (proves analyze was skipped).
        store
            .upsert_metrics(&[MetricPoint::new(
                MetricKind::RestingHeartRateBpm,
                Utc::now(),
                day,
                56.0,
                "bpm",
                "test",
            )])
            .unwrap();

        let hit = build_brief(&store, &cached_opts).unwrap();
        assert_eq!(hit.generated_at, first.generated_at);
        assert_eq!(hit.metric_count, first.metric_count);
        assert_eq!(hit.summary, first.summary);

        let refreshed = build_brief(
            &store,
            &BriefOptions {
                day: Some(day),
                week: false,
                refresh: true,
            },
        )
        .unwrap();
        assert_ne!(refreshed.generated_at, first.generated_at);
        assert!(refreshed.metric_count > first.metric_count);
    }

    #[test]
    fn upsert_digest_surfaces_db_errors() {
        let (_dir, store, day) = seed_store();
        let digest = build_brief(
            &store,
            &BriefOptions {
                day: Some(day),
                week: false,
                refresh: true,
            },
        )
        .unwrap();
        let conn = rusqlite::Connection::open(store.path()).unwrap();
        conn.execute_batch("DROP TABLE digests;").unwrap();
        drop(conn);
        let err = store.upsert_digest(&digest).unwrap_err();
        assert!(matches!(err, GroktorError::Db(_)));
    }

    #[test]
    fn cache_hit_keeps_stored_llm_backend_and_narrative() {
        let (_dir, store, day) = seed_store();
        let mut first = build_brief(
            &store,
            &BriefOptions {
                day: Some(day),
                week: false,
                refresh: true,
            },
        )
        .unwrap();
        first.llm_backend = Some("local".into());
        first.llm_narrative = Some("spark brief".into());
        store.upsert_digest(&first).unwrap();

        let hit = build_brief(
            &store,
            &BriefOptions {
                day: Some(day),
                week: false,
                refresh: false,
            },
        )
        .unwrap();
        assert_eq!(hit.llm_backend.as_deref(), Some("local"));
        assert_eq!(hit.llm_narrative.as_deref(), Some("spark brief"));
        assert_eq!(hit.generated_at, first.generated_at);
    }
}
