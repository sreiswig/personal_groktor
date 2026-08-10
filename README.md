# Personal Groktor

**Personal Groktor** turns your own health data into clear, actionable insight. An LLM (Grok by default) reviews wearable and lifestyle exports for anomalies, trends, and overall wellbeing — privately, on your terms.

## Why this exists

Health dashboards show charts. They rarely explain *what matters*. This project aims to:

- **Spot anomalies** — unusual HRV, sleep debt spikes, resting heart rate shifts, activity drop-offs
- **Summarize wellbeing** — short narratives over days/weeks, not just raw numbers
- **Stay personal** — your data, your pipeline; designed for local or self-hosted use

## What it does

1. **Ingest** health exports (CSV / JSON; more formats later)
2. **Normalize** metrics into a common schema (sleep, heart, activity, body, recovery)
3. **Analyze** with explainable rules (stats + patterns) and optional Grok interpretation
4. **Report** concise daily digests you can read in minutes

## Core principles

| Principle | Meaning |
|-----------|---------|
| Privacy first | Prefer local processing; minimize what leaves your machine |
| Explainable | Every flag should come with *why* it was raised |
| Human in the loop | Insights assist you; they are not medical advice |
| Extensible | Pluggable sources, metrics, and model backends |

## Status

Rust CLI scaffold is in place:

- [x] Canonical health metric schema
- [x] CSV / JSON ingest + normalization
- [x] Local SQLite storage
- [x] Rule-based anomaly detection (z-score, sleep debt, RHR after poor sleep, HRV drop, activity drop-off)
- [x] Local digests + optional xAI Grok narrative
- [ ] Broader export parsers (Apple Health XML, Fitbit bulk, etc.) — see [`open_source_devices/apis/`](open_source_devices/apis/)
- [ ] Open device adapters (PineTime, Bangle.js, Gadgetbridge, BLE HR, …) — see [`open_source_devices/`](open_source_devices/)
- [ ] Weekly rollups and exportable reports polish
- [ ] Optional privacy modes (redaction, local-only metrics)

## Tech stack

- **Language**: Rust
- **CLI**: [clap](https://docs.rs/clap)
- **Storage**: SQLite ([rusqlite](https://docs.rs/rusqlite), local file)
- **LLM**: [Grok](https://x.ai) via xAI API (`XAI_API_KEY`)

## Getting started

### Requirements

- Rust 1.75+ (edition 2021)
- Optional: an [xAI API key](https://console.x.ai/) for Grok narratives

### Build

```bash
git clone <your-remote-url> personal_groktor
cd personal_groktor
cargo build --release
```

### Quick demo with sample data

```bash
# Import sample wearable-style export
cargo run -- ingest examples/sample_metrics.csv

# Optional second source
cargo run -- ingest examples/sample_metrics.json

# Database status
cargo run -- status

# Rule-based analysis
cargo run -- analyze

# Daily digest (latest day; local summary, no API call)
cargo run -- digest

# Digest for a specific day + Grok narrative
export XAI_API_KEY=your_key_here
cargo run -- digest --day 2026-07-29 --llm --out /tmp/groktor-digest.md
```

### CLI overview

| Command | Purpose |
|---------|---------|
| `ingest <path>` | Load `.csv` or `.json` export into SQLite |
| `analyze [--from DATE] [--to DATE]` | Run rule engine; store findings |
| `digest [--day DATE] [--llm] [--out PATH]` | Print / write a wellbeing brief |
| `status` | Show DB path, metric count, date range |

Global option: `--db PATH` (or `GROKTOR_DB`) to override the default database location  
(`~/.local/share/personal_groktor/data.db` on Linux).

### Expected CSV / JSON shape

CSV headers:

```text
date,metric,value,unit,source
2026-07-20,sleep_duration_hours,7.2,hours,oura
2026-07-20,resting_heart_rate_bpm,56,bpm,oura
```

JSON: an array of objects, or `{ "metrics": [ ... ] }` with the same fields.  
Common metric aliases (`rhr`, `hrv`, `steps`, …) map to the canonical schema automatically.

### Environment

| Variable | Meaning |
|----------|---------|
| `XAI_API_KEY` | xAI API key (required for `--llm`) |
| `XAI_BASE_URL` | API base (default `https://api.x.ai/v1`) |
| `XAI_MODEL` | Model id (default `grok-3`) |
| `GROKTOR_DB` | SQLite path override |
| `RUST_LOG` | Tracing filter (e.g. `debug`) |

## Project layout

```text
src/
  main.rs          CLI entry
  lib.rs           Library root
  schema.rs        Metric / finding types
  ingest/          CSV & JSON loaders
  normalize.rs     Raw → canonical metrics
  store.rs         SQLite persistence
  analyze/         Stats + explainable rules
  llm/             Local summary + Grok client
  report.rs        Digest formatting
docs/              Design docs (brief, lab, research agent)
examples/          Sample exports for a dry run
open_source_devices/   Planned device hooks + vendor API inventory
open_medical_papers/   Free/OA medical literature sources (sources.txt)
data_estimates/        Storage estimates (personal health + research agent)
```

## Design docs

Personal product direction (solo use: **brief + lab + research agent**, Amazfit / Google Health primary, universal local hooks) is captured under [`docs/`](docs/):

| Doc | Topic |
|-----|--------|
| [docs/schema_cli.md](docs/schema_cli.md) | Schema extensions, brief/lab CLI, config, LLM backends |
| [docs/research_agent_v0.md](docs/research_agent_v0.md) | Literature harvest, FTS, authenticity, brief/lab attach |

## Data sources (planned)

| Path | Location | Role |
|------|----------|------|
| Generic CSV / JSON | `ingest` today | Manual exports, DIY scripts |
| **Open devices & local bridges** | [`open_source_devices/`](open_source_devices/) | PineTime, Bangle.js, Gadgetbridge, BLE HR, ESP32, Nightscout, … |
| **Vendor APIs & official exports** | [`open_source_devices/apis/`](open_source_devices/apis/) | Apple Health, Fitbit, Oura, Garmin, … |

Both paths normalize into the same schema; open hardware is preferred for ongoing capture, APIs/exports for historical backfill.

## Roadmap (high level)

- [ ] Annotations + N=1 lab + `brief` CLI (see [`docs/schema_cli.md`](docs/schema_cli.md))
- [ ] Research agent v0 — harvest, FTS, attach to brief/lab (see [`docs/research_agent_v0.md`](docs/research_agent_v0.md))
- [ ] Amazfit / Gadgetbridge + Google Health Connect export adapters
- [ ] Universal local hook polish (watch-folder / drop dir)
- [ ] Pluggable LLM backends (Grok + local OpenAI-compatible)
- [ ] Weekly brief, digest cache, confidence scoring
- [ ] Configurable rule thresholds
- [ ] Lightweight web UI later (CLI-first)

## Disclaimer

This project is for **personal education and self-reflection only**. It is **not** a medical device and does **not** provide diagnosis, treatment, or clinical advice. Always consult a qualified healthcare professional for medical concerns.

## License

MIT (see `Cargo.toml`)
