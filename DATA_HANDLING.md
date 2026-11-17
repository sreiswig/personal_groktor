# Data handling checklist (SEC-3)

Personal GrokTor touches health-adjacent local data and optional LLM calls. Treat this as the minimum bar before sharing the repo, demos, or exports.

## What the code enforces (verified against `src/`)

These are guarantees the code makes today. If you change the code, keep this list true.

- **File permissions (Unix).** `Store::open` creates missing parent directories as `0700` and the DB file as `0600` before SQLite opens it (`src/private_fs.rs`, `src/store.rs`). SQLite gives `-wal` / `-shm` / `-journal` sidecars the main DB's mode, so they are `0600` too, and older sidecars are narrowed on open. `brief --out`, `digest --out` and `lab report --out` write `0600` files and create missing parents as `0700`. Directories that already exist are not changed. On Windows these modes do not apply.
- **`--llm` is off by default.** No LLM call happens without `--llm`. With `--llm` and no `--llm-backend`, the backend is **`grok`**, which sends the prompt to `https://api.x.ai/v1` (or `XAI_BASE_URL`, which must be https unless it is loopback).
- **`--llm-backend local` stays on your network.** It has no default URL. `GROKTOR_LLM_BASE` / `LLM_API_BASE` must point to loopback, RFC 1918, Tailscale `100.64.0.0/10`, link-local, IPv6 ULA `fc00::/7`, `localhost`, a single-label host, or a `.local` / `.lan` / `.home.arpa` / `.internal` / `.ts.net` name. Anything else is refused. Proxy environment variables (`HTTP_PROXY`, `ALL_PROXY`, …) are ignored for this backend, redirects are not followed, it never falls back to Grok, and there is no telemetry. Limit: a LAN-style *name* is trusted as named. If your own DNS sends `foo.lan` to a public address, the prompt goes there.
- **What the prompt contains.** With `--llm`, the prompt includes the day's raw metric values, units and sources, the rule findings, your annotations for the window (tags, note text, mood, energy), and active experiment slugs. See `build_prompt` in `src/llm/mod.rs` and `build_brief_prompt` in `src/brief/mod.rs`.
- **Logs and errors.** LLM failures are logged (`tracing::warn!`) with the HTTP status and body length only, never the response body, which can echo the prompt. Ingest errors log the row number, an escaped metric name or date, and only non-finite sentinels (`NaN` / `inf`), never a stored reading. Health structs derive `Debug` but are never formatted with `{:?}` outside tests.

## Local health DB

- [ ] Keep the health DB on a **local path only** (not cloud-synced folders you do not control, not a public volume).
- [ ] Confirm `*.db*` / SQLite sidecars are gitignored and never staged.
- [ ] Prefer encrypting the disk/volume that holds the DB (OS full-disk encryption).
- [ ] Do not commit DB dumps, CSV exports of real health rows, or "sample" files derived from real data without explicit scrubbing.

## Secrets / API keys

- [ ] `XAI_API_KEY` (and any other keys) live in `.env` or the process environment — **never** in source, README, or commit history.
- [ ] Ship only `.env.example` with empty/placeholder values.
- [ ] If a key was ever committed or pasted into a ticket/chat: **rotate it** immediately.

## `--llm` / LLM behavior

- [ ] Assume anything sent to an LLM provider may leave the machine; do not send raw health records, identifiers, or free-text that re-identifies a person unless you explicitly accept that risk.
- [ ] Prefer minimization: aggregate, redact, or summarize locally before any remote call.
- [ ] Document when `--llm` is off by default; require an explicit flag to enable remote inference.
- [ ] Log only that an LLM call occurred (and maybe token counts), not prompt/response bodies containing personal data.

## Exports & sharing

- [ ] Never commit real exports (JSON/CSV/SQLite copies) "for debugging."
- [ ] If you need fixtures: synthesize fake rows; mark them `FIXTURE` / `SYNTHETIC`.
- [ ] Before attaching files to issues/PRs: scan for names, dates of birth, device IDs, API keys.

## Operational hygiene

- [ ] Review `git status` before every commit when working near data dirs.
- [ ] Periodically `git log --all --full-history -- '*.db*' '.env'` (and similar) to confirm nothing slipped in.
- [ ] If secrets/data did slip into git: stop, rotate, and plan a history purge **with Sam approval** (destructive).
