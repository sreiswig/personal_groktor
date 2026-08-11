# Data handling checklist (SEC-3)

Personal GrokTor touches health-adjacent local data and optional LLM calls. Treat this as the minimum bar before sharing the repo, demos, or exports.

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
