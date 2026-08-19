//! Optional LLM backends for narrative interpretation.

mod grok;

pub use grok::{
    resolve_grok_config, resolve_local_config, ChatClient, ChatConfig, GrokClient, LlmBackend,
};

use crate::schema::{
    Annotation, ConfidenceLevel, Digest, DigestConfidence, DigestHorizon, Finding, MetricPoint,
};

/// Build a deterministic local summary without calling an LLM.
pub fn local_summary(day: chrono::NaiveDate, findings: &[Finding], metric_count: usize) -> String {
    if findings.is_empty() {
        return format!(
            "Wellbeing brief for {day}: {metric_count} metric(s) on record. \
             No rule-based flags. Keep an eye on sleep, resting HR, and activity consistency."
        );
    }

    let high = findings
        .iter()
        .filter(|f| matches!(f.severity, crate::schema::Severity::High))
        .count();
    let medium = findings
        .iter()
        .filter(|f| matches!(f.severity, crate::schema::Severity::Medium))
        .count();

    let mut lines = vec![format!(
        "Wellbeing brief for {day}: {metric_count} metric(s); {} finding(s) \
         ({high} high, {medium} medium).",
        findings.len()
    )];

    for f in findings.iter().take(8) {
        lines.push(format!("- [{}] {}: {}", f.severity, f.title, f.detail));
    }
    if findings.len() > 8 {
        lines.push(format!("- …and {} more", findings.len() - 8));
    }
    lines.push("Insights are for personal reflection only — not medical advice.".into());
    lines.join("\n")
}

/// Prompt body shared with any LLM backend.
pub fn build_prompt(day_metrics: &[MetricPoint], findings: &[Finding]) -> String {
    let mut metrics_block = String::new();
    for m in day_metrics {
        metrics_block.push_str(&format!(
            "- {} = {:.2} {} (source: {})\n",
            m.kind, m.value, m.unit, m.source
        ));
    }
    let mut findings_block = String::new();
    for f in findings {
        findings_block.push_str(&format!("- [{}] {} — {}\n", f.severity, f.title, f.detail));
    }
    if findings_block.is_empty() {
        findings_block.push_str("(none)\n");
    }

    format!(
        r#"You are Personal Groktor, a careful health-data assistant.
Given wearable metrics and rule-based findings, write a short wellbeing briefing.

Rules:
- 4–8 sentences, plain language
- Explain what stands out and why (reference the numbers)
- Note cross-metric patterns when present
- Do NOT diagnose or prescribe treatment
- End with one gentle, practical suggestion (sleep, movement, recovery)

Metrics:
{metrics_block}
Findings:
{findings_block}
"#
    )
}

/// Attach optional LLM narrative onto a digest skeleton (day horizon, no annotations).
pub fn digest_from_parts(
    day: chrono::NaiveDate,
    findings: Vec<Finding>,
    metric_count: usize,
    llm_narrative: Option<String>,
) -> Digest {
    digest_from_parts_full(day, findings, vec![], metric_count, llm_narrative, None)
}

/// Full digest constructor used by brief/digest commands.
pub fn digest_from_parts_full(
    day: chrono::NaiveDate,
    findings: Vec<Finding>,
    annotations: Vec<Annotation>,
    metric_count: usize,
    llm_narrative: Option<String>,
    llm_backend: Option<String>,
) -> Digest {
    let summary = local_summary(day, &findings, metric_count);
    Digest {
        day,
        horizon: DigestHorizon::Day,
        generated_at: chrono::Utc::now(),
        findings,
        annotations,
        active_experiments: vec![],
        summary,
        llm_narrative,
        llm_backend,
        metric_count,
        research_bits: vec![],
        confidence: DigestConfidence {
            level: ConfidenceLevel::Ok,
            reasons: vec!["legacy digest path".into()],
        },
    }
}
