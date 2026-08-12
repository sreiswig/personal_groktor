//! Human-readable digests, briefs, and lab report formatting.

use chrono::NaiveDate;

use crate::schema::{Digest, DigestHorizon, Finding, LabReport, ResearchCite, Severity};

pub fn format_digest(digest: &Digest) -> String {
    let title = match digest.horizon {
        DigestHorizon::Day => format!("Brief — {}", digest.day),
        DigestHorizon::Week { start } => {
            format!("Brief — week {start} → {}", digest.day)
        }
    };

    let mut out = String::new();
    out.push_str(&format!("# {title}\n\n"));
    out.push_str(&format!(
        "_Generated {} · Confidence: {} — {_}\n\n",
        digest.generated_at.format("%Y-%m-%d %H:%M UTC"),
        digest.confidence.level,
        digest.confidence.reasons.join("; ")
    ));

    out.push_str("## Summary\n\n");
    out.push_str(&digest.summary);
    out.push_str("\n\n");

    if let Some(narrative) = &digest.llm_narrative {
        let backend = digest.llm_backend.as_deref().unwrap_or("llm");
        out.push_str(&format!("## Narrative ({backend})\n\n"));
        out.push_str(narrative);
        out.push_str("\n\n");
    }

    out.push_str("## Findings\n\n");
    if digest.findings.is_empty() {
        out.push_str("_No rule-based findings for this window._\n");
    } else {
        for f in &digest.findings {
            out.push_str(&format_finding(f));
            out.push('\n');
        }
    }

    if !digest.annotations.is_empty() {
        out.push_str("\n## Context\n\n");
        for a in &digest.annotations {
            let tags = a.tags.join(", ");
            let body = a.body.as_deref().unwrap_or("");
            let mood = a.mood.map(|m| format!(" mood={m}")).unwrap_or_default();
            let energy = a.energy.map(|e| format!(" energy={e}")).unwrap_or_default();
            out.push_str(&format!(
                "- **{}** tags: [{tags}]{mood}{energy} {body}\n",
                a.day
            ));
        }
    }

    if !digest.active_experiments.is_empty() {
        out.push_str("\n## Lab\n\n");
        for e in &digest.active_experiments {
            let arm = e
                .arm_today
                .map(|a| a.to_string())
                .unwrap_or_else(|| "unset".into());
            let idx = e
                .day_index
                .map(|i| format!(" · day {i}"))
                .unwrap_or_default();
            out.push_str(&format!(
                "- **{}** ({}){idx} — arm: {arm}\n",
                e.slug, e.title
            ));
        }
    }

    if !digest.research_bits.is_empty() {
        out.push_str("\n## Background reading\n\n");
        for c in &digest.research_bits {
            out.push_str(&format_cite(c));
            out.push('\n');
        }
    }

    out.push_str("\n---\n");
    out.push_str(
        "For personal education only. Not a medical device. Not diagnosis or treatment advice.\n",
    );
    out
}

pub fn format_lab_report(report: &LabReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Lab — {}\n\n", report.experiment.slug));
    out.push_str(&format!("**{}**\n\n", report.experiment.title));
    out.push_str(&format!(
        "_Status: {} · Window: {} → {} · Intervention days: {} · Control days: {_}\n\n",
        report.experiment.status,
        report.window.0,
        report.window.1,
        report.n_intervention,
        report.n_control
    ));

    out.push_str("## Hypothesis\n\n");
    out.push_str(&report.experiment.hypothesis);
    out.push_str("\n\n");

    out.push_str("## Design\n\n");
    out.push_str(&format!(
        "- Min days target: {}\n- Outcomes:\n",
        report.experiment.min_days
    ));
    for o in &report.experiment.outcomes {
        let prim = if o.primary { " (primary)" } else { "" };
        out.push_str(&format!(
            "  - {} · direction {}{prim}\n",
            o.kind.as_str(),
            o.direction
        ));
    }
    out.push('\n');

    out.push_str("## Outcomes\n\n");
    if report.outcomes.is_empty() {
        out.push_str("_No outcomes defined._\n");
    } else {
        out.push_str("| Metric | Arm n (I/C) | Mean I | Mean C | Δ | d |\n");
        out.push_str("|--------|-------------|--------|--------|---|---|\n");
        for o in &report.outcomes {
            let prim = if o.primary { " *" } else { "" };
            out.push_str(&format!(
                "| {}{prim} | {}/{} | {} | {} | {} | {} |\n",
                o.kind.as_str(),
                o.n_intervention,
                o.n_control,
                fmt_opt(o.mean_intervention),
                fmt_opt(o.mean_control),
                fmt_opt_signed(o.delta),
                fmt_opt(o.effect_size),
            ));
        }
        out.push_str("\n_\\* primary outcome. Effect size d shown only when n≥5 per arm. Descriptive N=1 only._\n");
    }

    out.push_str("\n## Summary\n\n");
    out.push_str(&report.summary);
    out.push_str("\n\n");

    if !report.confounds.is_empty() {
        out.push_str("## Confounds\n\n");
        for c in &report.confounds {
            out.push_str(&format!("- {c}\n"));
        }
        out.push('\n');
    }

    if !report.findings_overlap.is_empty() {
        out.push_str("## Findings in window\n\n");
        for f in &report.findings_overlap {
            out.push_str(&format_finding(f));
            out.push('\n');
        }
        out.push('\n');
    }

    if let Some(narrative) = &report.llm_narrative {
        out.push_str("## Narrative\n\n");
        out.push_str(narrative);
        out.push_str("\n\n");
    }

    if !report.research_refs.is_empty() {
        out.push_str("## Related open literature\n\n");
        for c in &report.research_refs {
            out.push_str(&format_cite(c));
            out.push('\n');
        }
        out.push('\n');
    }

    out.push_str("---\n");
    out.push_str(
        "For personal education only. Not a medical device. Not diagnosis or treatment advice.\n",
    );
    out
}

fn fmt_opt(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.2}")).unwrap_or_else(|| "—".into())
}

fn fmt_opt_signed(v: Option<f64>) -> String {
    v.map(|x| format!("{x:+.2}")).unwrap_or_else(|| "—".into())
}

fn format_finding(f: &Finding) -> String {
    let badge = match f.severity {
        Severity::High => "HIGH",
        Severity::Medium => "MED",
        Severity::Low => "LOW",
        Severity::Info => "INFO",
    };
    format!(
        "- **[{badge}] {}** — {}\n  _(rule: `{}`)_",
        f.title, f.detail, f.rule_id
    )
}

fn format_cite(c: &ResearchCite) -> String {
    let year = c
        .year
        .map(|y| y.to_string())
        .unwrap_or_else(|| "n.d.".into());
    let mut badges = vec![format!("[{year}]")];
    if c.retracted {
        badges.push("[RETRACTED]".into());
    } else if c.is_preprint {
        badges.push("[preprint]".into());
    } else {
        badges.push("[journal]".into());
    }
    if c.oa_url.is_some() {
        badges.push("[OA]".into());
    }
    let venue = c
        .venue
        .as_deref()
        .map(|v| format!(" ({v})"))
        .unwrap_or_default();
    format!("- {} {}{venue}", badges.join(""), c.title)
}

pub fn format_findings_table(findings: &[Finding]) -> String {
    if findings.is_empty() {
        return "No findings.".into();
    }
    let mut out = String::from("Day        | Severity | Finding\n");
    out.push_str("-----------|----------|--------\n");
    for f in findings {
        out.push_str(&format!(
            "{} | {:8} | {}\n",
            f.day,
            f.severity.as_str(),
            f.title
        ));
    }
    out
}

/// Pick the latest day with data if `day` is None.
pub fn resolve_day(
    requested: Option<NaiveDate>,
    available: Option<(NaiveDate, NaiveDate)>,
) -> Option<NaiveDate> {
    if let Some(d) = requested {
        return Some(d);
    }
    available.map(|(_, max)| max)
}
