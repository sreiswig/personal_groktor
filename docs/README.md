# Personal Groktor — design docs

Planning documents for features not fully implemented in the CLI yet.  
Runtime code of record is still `src/` and the root [README](../README.md).

| Doc | Contents |
|-----|----------|
| [schema_cli.md](schema_cli.md) | Health DB schema extensions, brief/lab types, CLI surface, config, LLM backends, implementation phases |
| [research_agent_v0.md](research_agent_v0.md) | In-repo research agent: storage, harvest APIs, FTS retrieval, authenticity, brief/lab integration |

## Product context (locked)

| Decision | Choice |
|----------|--------|
| Audience | Solo (personal use) |
| Modes | **Brief** and **Lab** are both required |
| Research | In-project agent (not a separate product) |
| LLM | Cloud (Grok) and local (OpenAI-compatible) both supported |
| Primary ecosystems | Amazfit + Google Health / Health Connect |
| Device policy | Universal local hook (CSV/JSON drop + adapters); any device via normalize |

Related folders:

- [`open_source_devices/`](../open_source_devices/) — device and vendor API inventory  
- [`open_medical_papers/`](../open_medical_papers/) — free/OA literature source list  
- [`data_estimates/`](../data_estimates/) — storage sizing (health + research agent)  

## Disclaimer

These designs are for a personal education tool. Not a medical device; not diagnosis or treatment advice.
