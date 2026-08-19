# Agent notes — Personal Groktor

Rust CLI that turns personal wearable/health exports into a local brief. Insights are for **self-reflection only — not medical advice**. Do not invent product features beyond the repo roadmap.

## Everyday commands

| Command | Purpose |
|---------|---------|
| `ingest <path>` | Load `.csv` or `.json` into SQLite |
| `analyze [--from DATE] [--to DATE]` | Rule engine; store findings |
| `brief [--day DATE] [--week] [--llm] [--llm-backend grok\|local] [--out PATH]` | Day/week wellbeing brief |
| `digest …` | Alias of `brief` |

Global `--db PATH` or `GROKTOR_DB` overrides the default database (`~/.local/share/personal_groktor/data.db` on Linux).

Typical loop: `ingest` → `analyze` (optional; `brief` re-runs rules) → `brief`.

## LLM

`brief --llm` / `digest --llm` is the on-switch. `--llm-backend` defaults to `grok`.

### Grok (default)

| Variable | Meaning |
|----------|---------|
| `XAI_API_KEY` | Required |
| `XAI_BASE_URL` | Default `https://api.x.ai/v1` |
| `XAI_MODEL` | Default `grok-3` |

### Local OpenAI-compatible (Spark vLLM, Ollama, …)

No phone-home default. Base URL and model are required.

| Variable | Meaning |
|----------|---------|
| `GROKTOR_LLM_BASE` or `LLM_API_BASE` | Required, e.g. `http://127.0.0.1:8000/v1` |
| `GROKTOR_LLM_MODEL` or `LLM_MODEL` | Required served model id |
| `GROKTOR_LLM_API_KEY` or `LLM_API_KEY` | Optional; if unset the bearer token is `local` |

Example against a Spark box running vLLM:

```bash
export GROKTOR_LLM_BASE=http://127.0.0.1:8000/v1
export GROKTOR_LLM_MODEL=your-served-model
cargo run -- brief --llm --llm-backend local
```

Groktor POSTs `{base}/chat/completions`. `digest.llm_backend` is stored as `"grok"` or `"local"`.

## Guardrails

- Do not diagnose, prescribe, or present output as clinical advice.
- Keep analysis local; only `--llm` sends a prompt to the selected backend.
- Prefer unit tests with no network for backend selection and missing config.
