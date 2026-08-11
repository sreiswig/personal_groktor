//! Simple confidence scoring for briefs.

use crate::schema::{Annotation, ConfidenceLevel, DigestConfidence, Finding};

/// Score how much data backs a brief.
pub fn score_confidence(
    metric_count: usize,
    findings: &[Finding],
    annotations: &[Annotation],
    week: bool,
) -> DigestConfidence {
    let mut reasons = Vec::new();
    let mut score: i32 = 0;

    let metric_floor = if week { 14 } else { 3 };
    if metric_count == 0 {
        reasons.push("no metrics in window".into());
        score -= 2;
    } else if metric_count < metric_floor {
        reasons.push(format!("sparse metrics ({metric_count})"));
        score -= 1;
    } else {
        reasons.push(format!("{metric_count} metrics"));
        score += 1;
    }

    if findings.is_empty() {
        reasons.push("no rule findings".into());
    } else {
        reasons.push(format!("{} finding(s)", findings.len()));
        score += 1;
    }

    if annotations.is_empty() {
        reasons.push("no annotations".into());
    } else {
        reasons.push(format!("{} annotation(s)", annotations.len()));
        score += 1;
    }

    let level = if score <= 0 {
        ConfidenceLevel::Thin
    } else if score >= 3 {
        ConfidenceLevel::Rich
    } else {
        ConfidenceLevel::Ok
    };

    DigestConfidence { level, reasons }
}
