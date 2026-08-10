# Research agent v0

Status: **design** — not implemented.  
Companion: [schema_cli.md](schema_cli.md) (brief/lab CLI and health schema).  
Source inventory: [`open_medical_papers/`](../open_medical_papers/).  
Storage estimates: [`data_estimates/medical_research_agent_storage.txt`](../data_estimates/medical_research_agent_storage.txt).

## Purpose

An **in-repo** subsystem that:

1. Harvests **open** biomedical/digital-health literature for personal topic packs  
2. Indexes it locally for retrieval  
3. Labels **authenticity / trust signals** (preprint vs journal, OA, retractions when known)  
4. Attaches short background cites to **brief** findings and **lab** experiments  
5. Optionally uses cloud or local LLM to synthesize **only over retrieved snippets**

It does **not** diagnose, treat, or replace clinical judgment. It does **not** scrape paywalls or shadow libraries.

## Goals and non-goals

| In v0 | Out of v0 |
|-------|-----------|
| Metadata + **abstracts** for topic packs | Mirroring all of PubMed |
| Local **FTS5** search | Perfect journal prestige rankings |
| Optional pin of legal OA PDF | Default full-PDF corpus |
| `query`, `for-finding`, `for-experiment` | Auto treatment advice |
| Retraction / preprint / OA badges when APIs provide them | Google Scholar scraping |
| LLM synthesis over retrieved cites only | Inventing papers not in the index |
| Separate `research.db` | Storing research blobs inside health `data.db` |

v0 is a **personal retrieval + labeling layer**, not a medical oracle.

## Architecture

```text
open APIs (OpenAlex, Europe PMC, …)
        │
        ▼
  research harvest ──► research.db (papers, FTS, authenticity, cache)
        │
        ▼
  ResearchAgent trait
        │
        ├── research query / show / pin (CLI)
        ├── brief --research  (attach 0–N cites to top findings)
        └── lab report --research  (background for hypothesis)
                 │
                 └── optional LlmBackend (grok | local)
```

Health pipeline (metrics → rules → brief/lab) stays independent: research is **opt-in attach**, never required for digests.

## On-disk layout

```text
~/.local/share/personal_groktor/research/
  research.db
  pdfs/                 # only papers explicitly pinned
  packs/                # installed pack TOML copies
```

Repo may ship default pack definitions under e.g. `research_packs/` (to be added at implementation time).

Config (see also schema_cli.md):

```toml
[research]
enabled = true
# db = ""           # default path under data_local_dir
embed = false       # v0: FTS only; embeddings = v0.1+
```

Env: `GROKTOR_RESEARCH_DB`.

---

## Topic packs

Packs are **data**, not hard-coded queries in Rust.

Example `wearables.toml`:

```toml
id = "wearables"
title = "Wearables, sleep, HRV, recovery"
years = 5
max_papers = 8000

[[queries]]
q = "heart rate variability sleep"

[[queries]]
q = "wearable resting heart rate recovery"

[[queries]]
q = "consumer sleep tracker validation"

[[queries]]
q = "sleep restriction resting heart rate"
```

Suggested default packs to ship:

| Pack id | Focus |
|---------|--------|
| `wearables` | Sleep, HRV, RHR, recovery, consumer wearable validation |
| `self_experiment_methods` | N-of-1 design, self-tracking methods (background for lab) |

Personal scale target (from storage estimates, scenario S1): roughly **2k–10k** papers in the working set after harvest — tens of MB for metadata + abstracts.

---

## Research database schema

```sql
CREATE TABLE papers (
  id              TEXT PRIMARY KEY,  -- e.g. openalex:W… or doi:…
  doi             TEXT,
  pmid            TEXT,
  pmcid           TEXT,
  title           TEXT NOT NULL,
  abstract        TEXT,
  year            INTEGER,
  venue           TEXT,
  authors_json    TEXT,
  oa_url          TEXT,
  license         TEXT,
  paper_type      TEXT,              -- journal_article|preprint|review|other
  is_oa           INTEGER NOT NULL DEFAULT 0,
  is_preprint     INTEGER NOT NULL DEFAULT 0,
  retracted       INTEGER NOT NULL DEFAULT 0,
  expression_of_concern INTEGER NOT NULL DEFAULT 0,
  peer_reviewed   INTEGER,           -- 1 VoR journal; 0 preprint; NULL unknown
  cited_by_count  INTEGER,
  source_api      TEXT NOT NULL,     -- openalex|europe_pmc|…
  raw_json        TEXT,              -- trimmed API payload
  harvested_at    TEXT NOT NULL,
  pinned          INTEGER NOT NULL DEFAULT 0,
  fulltext_path   TEXT
);

-- FTS: implementation may use FTS5 content table or external-content pattern
CREATE VIRTUAL TABLE papers_fts USING fts5(
  paper_id UNINDEXED,
  title,
  abstract,
  venue
);

CREATE TABLE paper_packs (
  paper_id  TEXT NOT NULL,
  pack_id   TEXT NOT NULL,
  PRIMARY KEY (paper_id, pack_id)
);

CREATE TABLE authenticity_events (
  id          TEXT PRIMARY KEY,
  paper_id    TEXT NOT NULL,
  kind        TEXT NOT NULL,  -- retraction|eoc|correction|preprint_version
  detail      TEXT,
  source      TEXT,
  recorded_at TEXT NOT NULL
);

CREATE TABLE chunks (
  id          TEXT PRIMARY KEY,
  paper_id    TEXT NOT NULL,
  ordinal     INTEGER NOT NULL,
  text        TEXT NOT NULL,
  embedding   BLOB            -- NULL in v0; used if embed=true later
);

CREATE TABLE query_cache (
  id          TEXT PRIMARY KEY,
  query       TEXT NOT NULL,
  created_at  TEXT NOT NULL,
  result_json TEXT NOT NULL
);

CREATE TABLE agent_runs (
  id          TEXT PRIMARY KEY,
  kind        TEXT NOT NULL,  -- harvest|query|for_finding|for_experiment|reindex
  started_at  TEXT NOT NULL,
  finished_at TEXT,
  detail_json TEXT
);
```

### Shared cite type (health + research)

```rust
pub struct ResearchCite {
    pub paper_id: String,
    pub title: String,
    pub year: Option<i32>,
    pub venue: Option<String>,
    pub oa_url: Option<String>,
    pub is_preprint: bool,
    pub retracted: bool,
    pub snippet: Option<String>,
    pub score: f32,
}
```

---

## Harvest APIs (priority)

Prefer no key or free tier, stable JSON, legal OA only.

| Priority | API | Role |
|----------|-----|------|
| **1** | [OpenAlex](https://openalex.org/) | Primary harvest: works search, OA URLs, concepts, cited_by, retraction flags when present |
| **2** | [Europe PMC](https://europepmc.org/) | Enrich abstracts, PMCID, free full-text links, preprints |
| **3** | Crossref | DOI metadata polish |
| Later | Unpaywall | OA resolution (email key) |
| Later | Semantic Scholar | Extra ranking / TLDR |
| Avoid | Google Scholar scrape | Fragile / ToS |

### Pack harvest algorithm

```text
for each query in pack:
  OpenAlex works search
    filter: publication year window, language optional
  page until pack max_papers / per-query budget
  upsert papers (stable id = openalex work id preferred)
  link paper_packs
  set is_oa, oa_url, license, is_preprint, paper_type, cited_by_count, retracted
optional:
  Europe PMC by DOI → pmid/pmcid / free full text hint
rebuild FTS
log agent_runs
```

- Polite rate limits; identify User-Agent with project name + contact if required  
- Incremental harvest (updated-date cursors) can wait until after first full pack install  
- Do **not** download PDFs during bulk harvest; only on explicit `research pin`

---

## Authenticity signals (v0)

| Signal | Storage / display |
|--------|-------------------|
| Preprint vs journal | `is_preprint`, `paper_type`, venue |
| OA + license | `is_oa`, `oa_url`, `license` |
| Citation count | `cited_by_count` (popularity ≠ quality; show raw) |
| Retracted | `retracted`; `authenticity_events` when detailed |
| Peer-reviewed | `peer_reviewed`: journal VoR ≈ true; preprint ≈ false; else null |

### Display badges

```text
[2019] [journal] [OA] Title… (Sleep Medicine)
[2024] [preprint] [OA] Title… (medRxiv)  — not peer-reviewed
[2011] [RETRACTED] Title…  — excluded from auto-attach
```

**Policy:**

- Retracted papers: **excluded** from brief/lab auto-attach; still visible via `research show` / explicit query with warning  
- Prefer peer-reviewed OA over preprint when ranking for “background reading”  
- Always show preprint / retraction status next to any cite

---

## Retrieval (v0)

Embeddings optional (`research.embed = false` by default).

1. Normalize user query string  
2. FTS5 over title + abstract (+ venue)  
3. Rank: FTS score × mild recency boost × OA boost × slight preprint penalty for background mode  
4. Drop retracted for auto-attach paths  
5. Return top *k* `ResearchCite` with abstract snippet  

**v0.1+:** fill `chunks.embedding` via local embed API; same `ResearchCite` interface.

### Agent trait

```rust
pub trait ResearchAgent {
    fn search(&self, q: &str, limit: usize) -> Result<Vec<ResearchCite>>;
    fn for_finding(&self, f: &Finding, limit: usize) -> Result<Vec<ResearchCite>>;
    fn for_experiment(&self, e: &Experiment, limit: usize) -> Result<Vec<ResearchCite>>;
    fn background_prompt_block(&self, cites: &[ResearchCite]) -> String;
}
```

**`for_finding`:** build query from `rule_id` + metric kind + direction  
Example: `hrv_drop` → `heart rate variability decrease sleep recovery wearable`

**`for_experiment`:** hypothesis text + primary outcome metric names → 3–5 background/methods papers.

**Brief attach:** if `brief.attach_research` and findings non-empty; cap with `brief.max_research_cites` (default 3).

### LLM synthesis rules

When `--llm` is used with research:

```text
Optional literature (open abstracts / metadata only; may be incomplete):
- …

Rules for the model:
- Use only the provided items; do not invent citations
- General background only; these papers did not study this user
- Do not diagnose or prescribe
- Prefer peer-reviewed over preprints; state uncertainty if mixed/weak
```

---

## CLI

```bash
personal_groktor research status
personal_groktor research pack list
personal_groktor research pack install wearables
personal_groktor research harvest --pack wearables
personal_groktor research harvest --query "HRV sleep restriction" --years 5
personal_groktor research reindex

personal_groktor research query "does short sleep raise resting heart rate"
personal_groktor research query "HRV recovery" --limit 10 --llm

personal_groktor research for-finding --day 2026-08-09 --rule hrv_drop
personal_groktor research for-finding <finding-uuid>
personal_groktor research for-experiment early_bed

personal_groktor research show <paper_id>
personal_groktor research pin <paper_id>
personal_groktor research check <paper_id>
```

| Command | Behavior |
|---------|----------|
| `status` | Paper count, packs, last harvest, disk use, embed on/off |
| `pack list` / `install` | Show / copy pack def and optionally harvest |
| `harvest` | Network → upsert papers + FTS; logs `agent_runs` |
| `reindex` | Rebuild FTS (and embeddings if enabled) |
| `query` | Local FTS table; `--llm` summarizes retrieved only |
| `for-finding` | Load finding from health DB; search; print cites |
| `for-experiment` | Load experiment by slug; search on hypothesis/outcomes |
| `show` | Full stored metadata + authenticity events |
| `pin` | If legal `oa_url` PDF, download under `pdfs/`, set `fulltext_path` |
| `check` | Refresh retraction/type signals from APIs when available |

Integration flags on other commands (see schema_cli.md):

```bash
personal_groktor brief --research
personal_groktor lab report early_bed --research --llm
```

---

## Module layout (target)

```text
src/research/
  mod.rs           # ResearchAgent, public types
  store.rs         # research.db migrate + CRUD
  harvest/
    mod.rs
    openalex.rs
    europe_pmc.rs  # optional enrich
  retrieve.rs      # FTS (+ future embeddings)
  packs.rs         # load pack TOML
  authenticity.rs  # badge helpers, filter retracted
  cite.rs          # ResearchCite, prompt block formatting
```

Health `brief` / `lab` call only the trait + `ResearchCite`; they do not import harvest details.

---

## Storage expectations (order of magnitude)

From `data_estimates/medical_research_agent_storage.txt`, personal topic pack:

| Layer | Rough size |
|-------|------------|
| Metadata + abstract for ~5k–10k papers | tens of MB |
| FTS indexes | small multiple of text |
| Optional embeddings later | can dominate (plan before enabling) |
| Pinned PDFs | ~1 MB each median; keep rare |

Health `data.db` remains separate and small under daily-summary usage (~10–100 MB/year).

---

## Implementation phases (research track)

Aligns with phases **E–F** in [schema_cli.md](schema_cli.md).

| Step | Work |
|------|------|
| E1 | `research.db` migrate + `ResearchCite` + empty `search` |
| E2 | OpenAlex harvest + pack install + FTS |
| E3 | CLI: `status`, `harvest`, `query`, `show` |
| E4 | Authenticity badges + retract filter |
| F1 | `for-finding` / `for-experiment` |
| F2 | `brief --research` / `lab report --research` |
| F3 | Optional `pin`, Europe PMC enrich, query cache |
| Later | Embeddings, Unpaywall, incremental harvest |

---

## Ethics, license, safety

- Prefer **peer-reviewed OA** or clearly labeled preprints  
- Check **license** before redistributing PDFs or bulk full text  
- Cache API JSON for personal use; do not republish full corpus  
- Pair literature with **local** analysis: papers inform curiosity; **your** wearable data stays on your machine  
- Every research-facing report ends with the project disclaimer  

### Disclaimer (copy for outputs)

> Open literature context for personal education only. Not medical advice. Retrieval may be incomplete; check primary sources. Preprints are not peer-reviewed. Retracted work is flagged when known and excluded from automatic brief/lab attach.

## Related

- Free source catalog: [`open_medical_papers/sources.txt`](../open_medical_papers/sources.txt)  
- Health brief/lab schema: [schema_cli.md](schema_cli.md)  
- Device/API ingest (separate from literature): [`open_source_devices/`](../open_source_devices/)  
