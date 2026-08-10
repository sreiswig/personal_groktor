# Bangle.js

| | |
|--|--|
| **Status** | Planned |
| **Hardware** | [Bangle.js](https://www.espruino.com/Bangle.js) (Pur3 Ltd / Espruino ecosystem) |
| **Firmware / apps** | Open; JavaScript apps on-device |
| **Why** | Fully hackable watch; easy custom exporters |

## Metrics of interest

| Metric | Availability | Notes |
|--------|--------------|--------|
| Heart rate | Yes | Built-in HR (model-dependent) |
| Steps | Yes | |
| GPS / outdoor activity | Model-dependent | Bangle.js 2 vs 1 |
| Sleep | App-dependent | Community apps |
| Custom sensors | High | User apps can log anything |

## Planned hook paths

1. **On-watch or App Loader app** that writes daily JSON/CSV over Bluetooth storage sync  
2. **Espruino Web IDE / CLI pull** → file drop into `ingest`  
3. **HTTP POST** from a watch app to a tiny local collector (optional)

## Canonical mapping (draft)

| Device field | `MetricKind` | Unit |
|--------------|--------------|------|
| steps | `steps` | count |
| bpm / hr | `resting_heart_rate_bpm` if daily rest; else note as sample | bpm |
| sleep hours | `sleep_duration_hours` | hours |

`source`: `banglejs`

## Open questions

- Standardize one “Groktor export” Bangle app format (JSON array of `RawMetricRow`)?
- Battery impact of continuous HR for wellbeing baselines?

## References

- <https://www.espruino.com/Bangle.js>
- Bangle.js App Loader community apps
