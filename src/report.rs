//! Human-readable digests and report formatting.

use chrono::NaiveDate;

use crate::schema::{Digest, Finding, Severity};

pub fn format_digest(digest: &Digest) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Personal Groktor — {}\n\n", digest.day));
    out.push_str(&format!(
        "_Generated {}_\n\n",
        digest.generated_at.format("%Y-%m-%d %H:%M UTC")
    ));
    out.push_str("## Summary\n\n");
    out.push_str(&digest.summary);
    out.push_str("\n\n");

    if let Some(narrative) = &digest.llm_narrative {
        out.push_str("## Grok narrative\n\n");
        out.push_str(narrative);
        out.push_str("\n\n");
    }

    out.push_str("## Findings\n\n");
    if digest.findings.is_empty() {
        out.push_str("_No rule-based findings for this day._\n");
    } else {
        for f in &digest.findings {
            out.push_str(&format_finding(f));
            out.push('\n');
        }
    }

    out.push_str("\n---\n");
    out.push_str(
        "For personal education only. Not a medical device. Not diagnosis or treatment advice.\n",
    );
    out
}

fn format_finding(f: &Finding) -> String {
    let badge = match f.severity {
        Severity::High => "HIGH",
        Severity::Medium => "MED",
        Severity::Low => "LOW",
        Severity::Info => "INFO",
    };
    format!("- **[{badge}] {}** — {}\n  _(rule: `{}`)_", f.title, f.detail, f.rule_id)
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
