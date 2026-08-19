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
- [x] Local digests / **brief** + optional xAI Grok narrative
- [x] Annotations (`note`) + N=1 lab (`lab start|day|report`) + digest cache tables
- [x] Day and rolling-week briefs with confidence + context
- [ ] Broader export parsers (Apple Health XML, Fitbit bulk, etc.) — see [`open_source_devices/apis/`](open_source_devices/apis/)
- [ ] Open device adapters (PineTime, Bangle.js, Gadgetbridge, BLE HR, …) — see [`open_source_devices/`](open_source_devices/)
- [ ] Research agent (literature harvest / FTS) — see [`docs/research_agent_v0.md`](docs/research_agent_v0.md)
- [ ] Optional privacy modes (redaction, local-only metrics)

## Tech stack

- **Language**: Rust
- **CLI**: [clap](https://docs.rs/clap)
- **Storage**: SQLite ([rusqlite](https://docs.rs/rusqlite), local file)
- **LLM**: [Grok](https://x.ai) via xAI API, or any local OpenAI-compatible server (`--llm-backend local`)

## Getting started

### Requirements

- Rust 1.75+ (edition 2021)
- Optional: an [xAI API key](https://console.x.ai/) for Grok narratives, or a local OpenAI-compatible server (vLLM, Ollama, …)

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

# Daily brief (latest day; local summary, no API call)
cargo run -- brief
# `digest` remains an alias of `brief`

# Note + N=1 lab
cargo run -- note --day 2026-07-28 --tag alcohol -m "dinner out"
cargo run -- lab start early_bed \
  --title "Earlier bedtime" \
  --hypothesis "Bed by 22:30 improves next-day HRV" \
  --outcome hrv:up --primary hrv
cargo run -- lab day early_bed --day 2026-07-28 --arm intervention
cargo run -- lab report early_bed

# Brief for a specific day + Grok narrative (default backend)
export XAI_API_KEY=your_key_here
cargo run -- brief --day 2026-07-29 --llm --out /tmp/groktor-brief.md
cargo run -- brief --week --day 2026-07-29

# Same on-switch, local OpenAI-compatible server (e.g. Spark vLLM)
export GROKTOR_LLM_BASE=http://127.0.0.1:8000/v1
export GROKTOR_LLM_MODEL=your-served-model
# optional: GROKTOR_LLM_API_KEY (defaults to bearer token `local`)
cargo run -- brief --llm --llm-backend local
```

### CLI overview

| Command | Purpose |
|---------|---------|
| `ingest <path>` | Load `.csv` or `.json` export into SQLite |
| `analyze [--from DATE] [--to DATE]` | Run rule engine; store findings |
| `brief [--day DATE] [--week] [--llm] [--llm-backend grok\|local] [--out PATH]` | Day/week wellbeing brief |
| `digest …` | Alias of `brief` (compat) |
| `note [--tag …] [-m …]` / `note list` | Log or list annotations |
| `lab start\|day\|list\|show\|report\|…` | N=1 experiments |
| `status` | DB path, metrics, notes, labs |

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
| `XAI_API_KEY` | xAI API key (required for `--llm` with default `--llm-backend grok`) |
| `XAI_BASE_URL` | Grok API base (default `https://api.x.ai/v1`) |
| `XAI_MODEL` | Grok model id (default `grok-3`) |
| `GROKTOR_LLM_BASE` / `LLM_API_BASE` | Local OpenAI-compatible base URL (required for `--llm-backend local`; no default) |
| `GROKTOR_LLM_MODEL` / `LLM_MODEL` | Local model id (required for `--llm-backend local`) |
| `GROKTOR_LLM_API_KEY` / `LLM_API_KEY` | Local bearer token (optional; defaults to `local`) |
| `GROKTOR_DB` | SQLite path override |
| `RUST_LOG` | Tracing filter (e.g. `debug`) |

`--llm` is the on-switch for `brief` / `digest`. `--llm-backend` selects the endpoint (default `grok`). Existing Grok usage is unchanged.

For `--llm-backend local`, Groktor POSTs `{base}/chat/completions` and never falls back to a public URL. Point `GROKTOR_LLM_BASE` at a LAN or loopback server such as Spark vLLM (`http://127.0.0.1:8000/v1`).

## Project layout

```text
src/
  main.rs          CLI entry
  lib.rs           Library root
  schema.rs        Metric, finding, annotation, experiment, digest types
  ingest/          CSV & JSON loaders
  normalize.rs     Raw → canonical metrics
  store.rs         SQLite persistence (health DB)
  analyze/         Stats + explainable rules
  brief/           Day/week brief builder + confidence
  lab/             N=1 experiment helpers + descriptive report
  llm/             Local summary + OpenAI-compatible client (Grok + local)
  report.rs        Brief / lab markdown formatting
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

- [x] Annotations + N=1 lab + `brief` CLI (phases A–C in [`docs/schema_cli.md`](docs/schema_cli.md))
- [ ] Research agent v0 — harvest, FTS, attach to brief/lab (see [`docs/research_agent_v0.md`](docs/research_agent_v0.md))
- [ ] Amazfit / Gadgetbridge + Google Health Connect export adapters
- [ ] Universal local hook polish (watch-folder / drop dir)
- [x] Pluggable LLM backends (Grok + local OpenAI-compatible; `--llm-backend local`)
- [x] Weekly brief, digest cache row, confidence scoring (v0)
- [ ] Configurable rule thresholds
- [ ] Lightweight web UI later (CLI-first)

## Disclaimer

This project is for **personal education and self-reflection only**. It is **not** a medical device and does **not** provide diagnosis, treatment, or clinical advice. Always consult a qualified healthcare professional for medical concerns.

## License

MIT (see `Cargo.toml`)
