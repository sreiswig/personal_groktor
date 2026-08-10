# Nightscout / open CGM stack

| | |
|--|--|
| **Status** | Planned (schema extension) |
| **What** | [Nightscout](https://nightscout.github.io/) and related open diabetes tooling (e.g. xDrip+, open APS ecosystem) |
| **Why** | Self-hosted glucose history; strong privacy-minded community |

## Metrics of interest

| Metric | Notes |
|--------|--------|
| Glucose (mg/dL or mmol/L) | Primary time series |
| Direction / trend arrows | Optional context |
| Treatments / carbs / insulin | Later; richer model |

Glucose is **not** in the core `MetricKind` list yet. Plan:

1. Short term: `MetricKind::Other("glucose_mgdl")` / `glucose_mmol`  
2. Later: first-class kind + rules (with very careful non-clinical wording)

## Planned hook paths

1. Nightscout **API** (`/api/v1/entries.json`) on LAN or VPN only  
2. **Export file** from xDrip+ / similar → ingest  
3. Never require public Nightscout URLs in docs/examples

## Canonical mapping (draft)

| Field | Mapping |
|-------|---------|
| `sgv` (mg/dL) | `Other("glucose_mgdl")` or future `glucose_mgdl` |
| direction | `notes` or separate event stream |

`source`: `nightscout` or `xdrip`

## Open questions

- First-class glucose in schema vs keep as extension forever?  
- Separate “metabolic” digest section so wellbeing rules do not over-claim?  
- Unit conversion (mg/dL ↔ mmol/L) ownership  

## Disclaimer (extra)

Glucose-related tooling is especially easy to misuse. Personal Groktor must never present CGM data as treatment advice. Therapy decisions stay with the user and their clinicians.

## References

- <https://nightscout.github.io/>
