# Gadgetbridge (bridge)

| | |
|--|--|
| **Status** | Planned |
| **What** | [Gadgetbridge](https://gadgetbridge.org/) — free Android app that talks to many wearables **without** vendor clouds |
| **Why** | One local bridge unlocks Mi Band / Amazfit / PineTime / and others for privacy-first ingest |

Gadgetbridge is not a sensor itself; it is the preferred **companion path** for devices whose stock apps force cloud accounts.

## Metrics of interest

Depends on the paired device. Commonly:

- Steps, activity intensity  
- Heart rate (periodic or continuous)  
- Sleep stages / duration  
- Sometimes SpO₂, stress proxies, workout sessions  

## Planned hook paths

1. **Database / export export** from Gadgetbridge (SQLite or CSV) → dedicated parser → normalize  
2. **Intent / auto-export** workflows on Android → syncthing/folder → desktop `ingest`  
3. Document per-device `source` tags: `gadgetbridge:miband7`, `gadgetbridge:pinetime`, …

## Canonical mapping (draft)

Reuse the same metric aliases as file ingest. Prefer daily aggregates for rule engine compatibility.

| GB-style activity | `MetricKind` |
|-------------------|--------------|
| total steps | `steps` |
| sleep duration | `sleep_duration_hours` |
| resting / average HR | `resting_heart_rate_bpm` (document derivation) |
| HRV if present | `heart_rate_variability_ms` |

## Open questions

- Which Gadgetbridge export format is most stable across releases?
- Linux-native alternatives (e.g. `siglo`, `amazfish`) — support the same file schema?

## References

- <https://gadgetbridge.org/>
- <https://codeberg.org/Freeyourgadget/Gadgetbridge>
