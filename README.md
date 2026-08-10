# Personal Groktor

**Personal Groktor** turns your own health data into clear, actionable insight. An LLM (Grok by default) reviews wearable and lifestyle exports for anomalies, trends, and overall wellbeing — privately, on your terms.

## Why this exists

Health dashboards show charts. They rarely explain *what matters*. This project aims to:

- **Spot anomalies** — unusual HRV, sleep debt spikes, resting heart rate shifts, activity drop-offs
- **Summarize wellbeing** — short narratives over days/weeks, not just raw numbers
- **Stay personal** — your data, your pipeline; designed for local or self-hosted use

## What it does (planned)

1. **Ingest** health exports (e.g. Apple Health, Google Fit, Fitbit, Oura, CSV/JSON dumps)
2. **Normalize** metrics into a common schema (sleep, heart, activity, body, recovery)
3. **Analyze** with Grok (or another LLM) for:
   - Statistical / rule-based flags plus LLM interpretation
   - Cross-metric patterns (e.g. poor sleep → higher resting HR next day)
   - Plain-language wellbeing briefings
4. **Report** concise daily/weekly digests you can read in minutes

## Core principles

| Principle | Meaning |
|-----------|---------|
| Privacy first | Prefer local processing; minimize what leaves your machine |
| Explainable | Every flag should come with *why* it was raised |
| Human in the loop | Insights assist you; they are not medical advice |
| Extensible | Pluggable sources, metrics, and model backends |

## Status

Early scaffolding. Repo is initialized; ingestion, analysis, and reporting are not built yet.

## Tech direction (draft)

- **Language**: Python (data parsing + orchestration)
- **LLM**: [Grok](https://x.ai) via xAI API (other providers optional later)
- **Data**: Local files / SQLite or similar for history
- **Interface**: CLI first; optional web/dashboard later

## Getting started

```bash
git clone <your-remote-url> personal_groktor
cd personal_groktor
# Setup steps will land here once the project is scaffolded
```

## Roadmap (high level)

- [ ] Define health data schema and supported export formats
- [ ] Ingest + normalize pipeline
- [ ] Anomaly detection (rules + stats)
- [ ] Grok-powered interpretation and wellbeing summaries
- [ ] CLI digests and exportable reports
- [ ] Optional privacy modes (redaction, local-only metrics)

## Disclaimer

This project is for **personal education and self-reflection only**. It is **not** a medical device and does **not** provide diagnosis, treatment, or clinical advice. Always consult a qualified healthcare professional for medical concerns.

## License

TBD
