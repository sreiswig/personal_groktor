# Device integration guide (planned)

How an open-source device should hook into Personal Groktor without forking the pipeline.

## Target contracts

### Preferred: emit canonical rows

Same shape as CSV/JSON ingest today:

| Field | Required | Example |
|-------|----------|---------|
| `date` | yes | `2026-08-01` |
| `timestamp` | no | `2026-08-01T07:12:00Z` |
| `metric` | yes | `resting_heart_rate_bpm` or alias `rhr` |
| `value` | yes | `56` |
| `unit` | no | `bpm` |
| `source` | recommended | `pinetime` |
| `notes` | no | free text |

Adapters may write this as:

1. **File drop** — device/bridge writes CSV or JSON; user runs `personal_groktor ingest <path>`
2. **Direct adapter** — Rust module talks BLE/HTTP and calls `normalize` + `Store::upsert_metrics`
3. **Daemon / bridge** — small companion process (or Gadgetbridge export job) that leaves files in a watched directory

### Metric mapping

Use names from `MetricKind` when possible:

| Canonical | Common device labels |
|-----------|----------------------|
| `sleep_duration_hours` | total sleep, sleep_hours |
| `resting_heart_rate_bpm` | RHR, resting HR |
| `heart_rate_variability_ms` | HRV, RMSSD |
| `steps` | step_count |
| `active_calories_kcal` | active energy |
| `exercise_minutes` | workout minutes |
| `spo2_pct` | SpO₂ |
| `readiness_score` | recovery / readiness |

Unknown names become `MetricKind::Other` and still store; rules may ignore them until taught.

## Transport options (priority)

1. **Offline file** — simplest, matches current CLI; best default for v1 adapters  
2. **Local HTTP/MQTT** — DIY nodes, Nightscout  
3. **BLE GATT** — HR service, vendor-specific characteristics (harder; platform BLE crates)  
4. **Vendor cloud API** — only when no local path exists (see `apis/`)

## Source tagging

Always set a stable `source` so digests and debugging stay clear:

```text
pinetime
banglejs
gadgetbridge
gadgetbridge:miband7
ble-hrs
esp32:bedroom-hr
nightscout
oura-api
fitbit-export
```

## Privacy notes

- Prefer processing on-device or on LAN.
- If a bridge must use a phone, prefer open bridges (e.g. Gadgetbridge) that keep data off vendor clouds.
- Optional future: `--local-only` to refuse any remote LLM/API calls during digest generation.
