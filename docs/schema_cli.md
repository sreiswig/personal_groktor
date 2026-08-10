# Schema + CLI design (brief, lab, research hooks)

Status: **design** — not fully implemented.  
Audience: personal install only.  
Companion: [research_agent_v0.md](research_agent_v0.md).

## Goals

Personal Groktor is a **local health lab + brief + literature copilot**:

| Job | Question it answers |
|-----|---------------------|
| **Brief** | What should I notice about me today / this week? |
| **Lab** | I’m testing X; what does my N=1 data show? |
| **Research** | What does open literature say, and how trustworthy is it? |

Rules and stats over **your** metrics stay local and explainable. Cloud or local LLMs are optional narrative layers. Research is an in-repo subsystem that attaches citations; it does not prescribe treatment.

## On-disk layout

```text
~/.local/share/personal_groktor/
  data.db                 # health metrics, findings, annotations, experiments, digests
  research/
    research.db           # papers, FTS, authenticity, query cache (see research_agent_v0.md)
    pdfs/                 # optional pinned OA PDFs only
    packs/                # installed topic pack definitions
~/.config/personal_groktor/
  config.toml             # thresholds, LLM default, brief/research prefs
```

**Why two databases:** research harvest and reindex can be wiped or rebuilt without risking personal health history or N=1 experiment records.

Environment overrides (planned):

| Variable | Meaning |
|----------|---------|
| `GROKTOR_DB` | Health SQLite path (exists today) |
| `GROKTOR_CONFIG` | Config file path |
| `GROKTOR_RESEARCH_DB` | Research SQLite path |

---

## Health database

### Existing tables (implemented)

#### `metrics`

Canonical daily (or timestamped) observations after normalize.

| Column | Type | Notes |
|--------|------|--------|
| `id` | TEXT PK | UUID |
| `kind` | TEXT | Canonical metric name |
| `category` | TEXT | sleep, heart, activity, body, recovery, other |
| `recorded_at` | TEXT | RFC3339 UTC |
| `day` | TEXT | `YYYY-MM-DD` |
| `value` | REAL | |
| `unit` | TEXT | |
| `source` | TEXT | e.g. `oura`, `gadgetbridge:amazfit`, `csv` |
| `raw_name` | TEXT | Optional original label |
| `notes` | TEXT | Optional |

Indexes: `day`; `(kind, day)`.

#### `findings`

Rule engine outputs with explainable `rule_id`.

| Column | Type | Notes |
|--------|------|--------|
| `id` | TEXT PK | UUID |
| `day` | TEXT | |
| `kind` | TEXT | Related metric kind |
| `severity` | TEXT | info, low, medium, high |
| `title` | TEXT | |
| `detail` | TEXT | Includes *why* |
| `value` | REAL | Optional observed |
| `baseline` | REAL | Optional baseline |
| `rule_id` | TEXT | Stable rule identifier |

Indexes: `day`.

### Planned health tables

#### `annotations`

Manual context for brief and lab (illness, alcohol, travel, mood, etc.).

```sql
CREATE TABLE annotations (
  id            TEXT PRIMARY KEY,
  day           TEXT NOT NULL,
  recorded_at   TEXT NOT NULL,
  tags          TEXT NOT NULL,   -- JSON array of strings
  body          TEXT,
  mood          INTEGER,         -- optional 1–5
  energy        INTEGER,         -- optional 1–5
  experiment_id TEXT,            -- soft link to experiments.id
  source        TEXT NOT NULL DEFAULT 'manual'
);
CREATE INDEX idx_annotations_day ON annotations(day);
CREATE INDEX idx_annotations_experiment ON annotations(experiment_id);
```

#### `experiments`

N=1 experiment definitions.

```sql
CREATE TABLE experiments (
  id              TEXT PRIMARY KEY,
  slug            TEXT NOT NULL UNIQUE,
  title           TEXT NOT NULL,
  hypothesis      TEXT NOT NULL,
  status          TEXT NOT NULL,  -- draft|active|paused|completed|abandoned
  started_on      TEXT,
  ended_on        TEXT,
  outcomes_json   TEXT NOT NULL,  -- see OutcomeSpec
  min_days        INTEGER NOT NULL DEFAULT 14,
  notes           TEXT,
  created_at      TEXT NOT NULL,
  updated_at      TEXT NOT NULL
);
```

`outcomes_json` example:

```json
[
  {"kind": "heart_rate_variability_ms", "direction": "up", "primary": true},
  {"kind": "sleep_duration_hours", "direction": "up", "primary": false}
]
```

#### `experiment_days`

Explicit arm assignment (v0: no automatic calendar rules).

```sql
CREATE TABLE experiment_days (
  experiment_id   TEXT NOT NULL,
  day             TEXT NOT NULL,
  arm             TEXT NOT NULL,  -- intervention|control|exclude
  note            TEXT,
  PRIMARY KEY (experiment_id, day),
  FOREIGN KEY (experiment_id) REFERENCES experiments(id)
);
```

#### `digests`

Cached brief output so re-open is free and research attach is stable.

```sql
CREATE TABLE digests (
  day              TEXT NOT NULL,
  horizon          TEXT NOT NULL,  -- day|week
  week_start       TEXT,           -- set when horizon=week
  generated_at     TEXT NOT NULL,
  metric_count     INTEGER NOT NULL,
  summary          TEXT NOT NULL,
  llm_backend      TEXT,           -- grok|local|null
  llm_narrative    TEXT,
  findings_json    TEXT NOT NULL,
  annotations_json TEXT,
  experiments_json TEXT,
  research_json    TEXT,
  confidence_json  TEXT,
  PRIMARY KEY (day, horizon)
);
```

### Optional migrate polish (non-blocking)

```sql
-- Stronger re-ingest dedupe (when id is random per import)
CREATE UNIQUE INDEX IF NOT EXISTS idx_metrics_natural
  ON metrics(day, kind, source, recorded_at, value);

-- Richer finding provenance
-- ALTER TABLE findings ADD COLUMN created_at TEXT;
-- ALTER TABLE findings ADD COLUMN inputs_json TEXT;
```

---

## Rust types (planned)

Align with `src/schema.rs`; split into modules if the file grows.

### Annotation

```rust
pub struct Annotation {
    pub id: Uuid,
    pub day: NaiveDate,
    pub recorded_at: DateTime<Utc>,
    pub tags: Vec<String>,       // lowercase snake_case
    pub body: Option<String>,
    pub mood: Option<u8>,        // 1–5
    pub energy: Option<u8>,      // 1–5
    pub experiment_id: Option<Uuid>,
    pub source: String,          // "manual" | "import:…"
}
```

### Experiment

```rust
pub enum ExperimentStatus {
    Draft,
    Active,
    Paused,
    Completed,
    Abandoned,
}

pub enum Arm {
    Intervention,
    Control,
    Exclude,
}

pub enum Direction {
    Up,
    Down,
    Change,
}

pub struct OutcomeSpec {
    pub kind: MetricKind,  // or MetricKind::Other
    pub direction: Direction,
    pub primary: bool,
}

pub struct Experiment {
    pub id: Uuid,
    pub slug: String,
    pub title: String,
    pub hypothesis: String,
    pub status: ExperimentStatus,
    pub started_on: Option<NaiveDate>,
    pub ended_on: Option<NaiveDate>,
    pub outcomes: Vec<OutcomeSpec>,
    pub min_days: u32,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct ExperimentDay {
    pub experiment_id: Uuid,
    pub day: NaiveDate,
    pub arm: Arm,
    pub note: Option<String>,
}
```

### Lab report (computed; optional cache later)

```rust
pub struct OutcomeResult {
    pub kind: MetricKind,
    pub direction: Direction,
    pub primary: bool,
    pub n_intervention: usize,
    pub n_control: usize,
    pub mean_intervention: Option<f64>,
    pub mean_control: Option<f64>,
    pub delta: Option<f64>,
    pub effect_size: Option<f64>,  // simple descriptive; only if n sufficient
}

pub struct LabReport {
    pub experiment: Experiment,
    pub window: (NaiveDate, NaiveDate),
    pub n_intervention: usize,
    pub n_control: usize,
    pub outcomes: Vec<OutcomeResult>,
    pub confounds: Vec<String>,
    pub findings_overlap: Vec<Finding>,
    pub summary: String,
    pub llm_narrative: Option<String>,
    pub research_refs: Vec<ResearchCite>,
}
```

v0 stats are **descriptive** (means, medians, Δ, optional effect size if n≥5). No p-value theater; label as personal N=1.

### Digest / brief (extended)

```rust
pub enum DigestHorizon {
    Day,
    Week { start: NaiveDate },
}

pub enum ConfidenceLevel {
    Thin,
    Ok,
    Rich,
}

pub struct DigestConfidence {
    pub level: ConfidenceLevel,
    pub reasons: Vec<String>,
}

pub struct ExperimentBrief {
    pub slug: String,
    pub title: String,
    pub day_index: Option<u32>,  // e.g. day 12 of active window
    pub arm_today: Option<Arm>,
}

pub struct Digest {
    pub day: NaiveDate,
    pub horizon: DigestHorizon,
    pub generated_at: DateTime<Utc>,
    pub findings: Vec<Finding>,
    pub annotations: Vec<Annotation>,
    pub active_experiments: Vec<ExperimentBrief>,
    pub summary: String,
    pub llm_narrative: Option<String>,
    pub llm_backend: Option<String>,
    pub metric_count: usize,
    pub research_bits: Vec<ResearchCite>,
    pub confidence: DigestConfidence,
}
```

`ResearchCite` is defined in [research_agent_v0.md](research_agent_v0.md); health code depends on a small shared type (or re-export from `research`).

### Tag conventions (personal)

Not enforced in schema; documented defaults:

| Family | Examples |
|--------|----------|
| Substance | `alcohol`, `caffeine_late` |
| Context | `travel`, `sick`, `stress_high`, `night_shift` |
| Sleep intent | `bed_early`, `bed_late`, `nap` |
| Training | `hard_session`, `rest_day`, `race` |
| Lab | Prefer `experiment_days.arm`; tags optional extras |

---

## CLI surface

### Global

```text
personal_groktor [GLOBAL] <command>

  --db PATH           Health DB (env: GROKTOR_DB)
  --config PATH       Config file
  --research-db PATH  Research DB override
```

### Command map

| Command | Status | Purpose |
|---------|--------|---------|
| `ingest <path>` | **exists** | Import CSV/JSON metrics |
| `analyze` | **exists** | Run rule engine; store findings |
| `digest` | **exists** → alias of `brief` | Day brief (compat) |
| `brief` | planned | Day/week wellbeing brief |
| `note` | planned | Log / list annotations |
| `lab` | planned | Experiments (start, arms, report) |
| `research` | planned | Literature agent (see research doc) |
| `status` | **exists** → extend | Health + lab + research summary |
| `sync` | later | Watch-folder / drop-dir ingest |

### `brief`

```bash
personal_groktor brief
personal_groktor brief --day 2026-08-09
personal_groktor brief --day 2026-08-09 --llm
personal_groktor brief --day 2026-08-09 --llm local
personal_groktor brief --day 2026-08-09 --llm grok --out ~/notes/health/2026-08-09.md
personal_groktor brief --week
personal_groktor brief --week --day 2026-08-09 --llm
personal_groktor brief --no-research
personal_groktor brief --research
personal_groktor brief --refresh
```

| Flag | Meaning |
|------|---------|
| `--day` | Calendar day; default latest day with metrics |
| `--week` | Rolling 7 days ending on `--day` (or latest) — **v0 week definition** |
| `--llm [backend]` | `grok` \| `local` \| `off`; bare `--llm` uses config default |
| `--out` | Write markdown |
| `--research` / `--no-research` | Attach open-lit cites (default from config) |
| `--refresh` | Ignore digest cache |

**Pipeline:**

```text
resolve day/week
→ metrics window (≥21 days before end, for baselines)
→ analyze / replace findings for window days
→ load annotations in range
→ load active experiments + arm for day
→ confidence score
→ local summary (metrics + findings + tags + lab status)
→ optional research.attach_for_findings (top findings, max cites from config)
→ optional llm.narrate(BriefContext)
→ upsert digests row
→ print / write markdown
```

`digest` remains a hidden or documented alias of `brief` for muscle memory.

### `note`

```bash
personal_groktor note --tag alcohol --tag late
personal_groktor note --day 2026-08-08 --tag travel -m "flight landed 23:40"
personal_groktor note --tag sick --mood 2 --energy 2
personal_groktor note --experiment early_bed --arm intervention
personal_groktor note list --from 2026-08-01 --to 2026-08-10
personal_groktor note list --tag alcohol
```

Default subcommand is **add**. `--arm` with `--experiment` upserts `experiment_days` and may add tags.

### `lab`

```bash
personal_groktor lab start early_bed \
  --title "Earlier bedtime" \
  --hypothesis "Bed by 22:30 improves next-day HRV and sleep duration" \
  --outcome hrv:up --outcome sleep_duration_hours:up \
  --primary hrv \
  --min-days 21

personal_groktor lab pause early_bed
personal_groktor lab resume early_bed
personal_groktor lab complete early_bed
personal_groktor lab abandon early_bed --reason "travel month"

personal_groktor lab day early_bed --arm intervention
personal_groktor lab day early_bed --day 2026-08-07 --arm control
personal_groktor lab day early_bed --from 2026-08-01 --to 2026-08-07 --arm intervention

personal_groktor lab list
personal_groktor lab show early_bed
personal_groktor lab report early_bed
personal_groktor lab report early_bed --llm --research --out ~/lab/early_bed.md
```

**`lab report` pipeline:**

```text
load experiment + experiment_days
→ metrics for outcome kinds in window
→ split intervention vs control (exclude arm dropped)
→ descriptive stats + confounds (e.g. intervention days also tagged sick)
→ local LabReport.summary
→ optional research.for_experiment
→ optional llm
→ markdown
```

### `research`

Full command list and behavior: [research_agent_v0.md](research_agent_v0.md).

### `status` (extended)

```text
Database:  …/data.db
Metrics:   N   range DATE → DATE
Findings:  N
Notes:     N
Labs:      A active, C completed
Research:  …/research.db  papers=N  last_harvest=DATE
LLM:       default=…  local=…
```

### LLM flag convention

Used by `brief`, `lab report`, and `research query`:

```text
--llm           → config default backend
--llm grok
--llm local
--llm off       → force no remote/local generation
```

Omit `--llm` → local deterministic summary only (today’s `digest` without `--llm`).

---

## Config (`config.toml` sketch)

```toml
[llm]
default = "grok"          # grok | local | off
grok_model = "grok-3"
# XAI_API_KEY / XAI_BASE_URL still from env
local_base_url = "http://127.0.0.1:11434/v1"
local_model = "llama3.1"

[brief]
attach_research = true
max_research_cites = 3
# rolling week length if ever configurable
# week_days = 7

[lab]
min_days_default = 14

[research]
enabled = true
# db = ""  # default under data_local_dir/personal_groktor/research/research.db
embed = false             # v0 FTS-only; embeddings later

[privacy]
# cloud payload richness when using remote LLM
cloud_payload = "findings_and_daily"   # findings_only | findings_and_daily | full
```

---

## LLM backend trait

```rust
#[async_trait]
pub trait LlmBackend {
    fn name(&self) -> &str; // "grok" | "local"
    async fn complete(&self, system: &str, user: &str) -> Result<String>;
}
```

Narrative helpers (same backends):

- `narrate_brief(BriefContext) -> String`
- `narrate_lab(LabContext) -> String`
- `synthesize_research(question, &[ResearchCite]) -> String` — must not invent papers not in the cite list

Analysis (rules, lab stats, FTS) never requires an LLM.

---

## Report shapes (markdown)

### Day brief

```markdown
# Brief — YYYY-MM-DD

_Confidence: Ok — …_

## Summary
(local)

## Narrative
(optional LLM)

## Findings
- **[MED] …** — … _(rule: `…`)_

## Context
- tags: …
- lab: early_bed — intervention

## Background reading
- [2019][journal][OA] Title (Venue)

---
For personal education only. Not a medical device. Not diagnosis or treatment advice.
```

### Lab report

```markdown
# Lab — slug

## Hypothesis
## Design
## Outcomes
## Confounds
## Narrative
## Related open literature
```

---

## Source layout (target)

```text
src/
  schema.rs / schema/     # metrics, findings, annotations, experiments, digest
  store.rs                # health migrate + CRUD
  lab/
    mod.rs
    analyze.rs            # N=1 stats → LabReport
  brief/
    mod.rs                # build day/week digest
    confidence.rs
  research/               # see research_agent_v0.md
  llm/
    mod.rs
    grok.rs               # exists
    local.rs              # OpenAI-compatible
    backend.rs            # LlmBackend trait
  report.rs               # format brief, lab, findings
  main.rs                 # CLI
```

---

## Implementation phases

| Phase | Deliverable | Depends |
|-------|-------------|---------|
| **A** | Migrate: `annotations`, `experiments`, `experiment_days`, `digests` + types | `store` |
| **B** | CLI: `note`, `lab start/day/list/show`, `brief` (+ `digest` alias), annotations in prompt | A |
| **C** | `lab report` local stats (no research) | B |
| **D** | `LlmBackend` + `--llm grok\|local` | independent-ish |
| **E** | Research DB + harvest + FTS + `research` commands | [research_agent_v0.md](research_agent_v0.md) |
| **F** | Wire `for-finding` / `for-experiment` + `brief --research` | C + E |
| **G** | Digest cache + week brief + confidence | B |

**A→B→C** and **E** can proceed in parallel; **F** is the integration point.

---

## Open decisions (when implementing)

1. **Week definition** — locked for v0 as **rolling 7 days** ending on `day` (not ISO week). Revisit if weekly habits prefer calendar weeks.
2. **Experiment arms** — **explicit only** in v0 (no “weekdays = intervention” rules).
3. **Finding addressing** — support `research for-finding --day D --rule rule_id` as well as UUID.
4. **Metric natural key** — add unique index when re-ingest duplication becomes painful.

## Non-goals (this design)

- Multi-user / SaaS
- Clinical diagnosis or treatment recommendations
- Replacing vendor chart UIs
- Web UI (CLI-first; later optional)

## Disclaimer

Personal education and self-reflection only. Not a medical device. Not diagnosis or treatment advice.
