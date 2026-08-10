# Open source devices

Planned **local / open** hardware and firmware that can feed Personal Groktor — as a complement to cloud vendor APIs (Oura, Fitbit, Apple Health, etc.).

This folder is a planning surface, not shipping adapters yet. When an adapter lands in Rust, link it from the device page and from `src/ingest/`.

## Why this exists

| Path | Who owns the data path | Typical fit |
|------|------------------------|-------------|
| **Open devices** | You (BLE, USB, SD, local sync apps) | Privacy-first, offline, hackable |
| **Vendor APIs / exports** | Platform + optional official API | Already have years of history |

Personal Groktor’s ingest layer should accept both. Open devices map cleanly onto the same canonical metrics in `src/schema.rs` (sleep, heart, activity, body, recovery).

## Design principles for device hooks

1. **Normalize early** — device-specific loaders emit `RawMetricRow` / `MetricPoint`, never parallel schemas.
2. **Prefer local** — BLE, file drop, or self-hosted bridge over vendor cloud when possible.
3. **Tag `source`** — e.g. `pinetime`, `banglejs`, `gadgetbridge:huami`, so digests stay explainable.
4. **Degrade gracefully** — missing HRV/sleep on a simple HR belt is fine; rules only use what exists.
5. **Not medical devices** — same project disclaimer; DIY sensors are for personal insight only.

## Planned devices & bridges

| Device / bridge | Status | Metrics (planned) | Notes |
|-----------------|--------|-------------------|--------|
| [PineTime / InfiniTime](devices/pinetime-infinitime.md) | Planned | HR, steps, activity | Open firmware; Gadgetbridge or direct BLE |
| [Bangle.js](devices/banglejs.md) | Planned | HR, steps, sleep (app-dep.) | Fully open; JSON/CSV via apps or BLE |
| [Gadgetbridge](devices/gadgetbridge.md) | Planned | Varies by band | Local Android bridge for many wearables |
| [BLE Heart Rate Service](devices/ble-heart-rate.md) | Planned | HR (live / session) | Standard GATT; belts & many watches |
| [DIY ESP32 / MCU nodes](devices/diy-esp32.md) | Planned | Custom (HR, SpO₂, env, …) | MQTT / HTTP / CSV → ingest |
| [Nightscout / open CGM](devices/nightscout-cgm.md) | Planned | Glucose (extension) | Self-hosted; schema extension may be needed |

## How data flows (target)

```text
  Open hardware / firmware          Cloud / phone ecosystems
  ─────────────────────────         ────────────────────────
  PineTime, Bangle.js,              Oura, Fitbit, Apple Health,
  ESP32 node, HR belt,              Garmin, Withings, Google Fit, …
  Gadgetbridge export
           │                                  │
           ▼                                  ▼
    device adapters (planned)          API / export adapters (planned)
           │                                  │
           └──────────────┬───────────────────┘
                          ▼
                 normalize → SQLite → analyze → digest
                          (existing pipeline)
```

Commercial / export-oriented sources are sketched under [apis/](apis/README.md).

## Integration checklist (for future PRs)

When adding a device adapter:

- [ ] Device page updated: status → In progress / Supported
- [ ] Loader under `src/ingest/` (or `src/ingest/devices/`)
- [ ] Maps raw fields → `MetricKind` / units documented on the device page
- [ ] Example fixture under `examples/devices/<name>/`
- [ ] `source` string stable and documented
- [ ] README roadmap checkbox if user-facing

## Related code today

- Schema: `src/schema.rs`
- Ingest: `src/ingest/` (CSV / JSON only for now)
- Sample generic export: `examples/sample_metrics.csv`
