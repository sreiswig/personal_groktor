# PineTime (InfiniTime)

| | |
|--|--|
| **Status** | Planned |
| **Hardware** | [Pine64 PineTime](https://www.pine64.org/pinetime/) |
| **Firmware** | [InfiniTime](https://github.com/InfiniTimeOrg/InfiniTime) (open source) |
| **Why** | Affordable open smartwatch; community BLE + Gadgetbridge support |

## Metrics of interest

| Metric | Availability | Notes |
|--------|--------------|--------|
| Heart rate | Common | On-demand / continuous depending on firmware/settings |
| Steps | Common | Daily activity |
| Motion / activity | Partial | Firmware-dependent |
| Sleep | Partial | Depends on firmware / companion |
| HRV | Uncommon | Not a primary InfiniTime feature historically |

## Planned hook paths

1. **Gadgetbridge export** → normalize → ingest (likely first)  
2. **Direct BLE** from a Rust companion (later)  
3. **File sync** if a companion app dumps CSV/JSON

## Canonical mapping (draft)

| Device field | `MetricKind` | Unit |
|--------------|--------------|------|
| daily steps | `steps` | count |
| heart rate samples | session avg → document; RHR if derived | bpm |
| sleep duration | `sleep_duration_hours` | hours |

`source`: `pinetime` or `gadgetbridge:pinetime`

## Open questions

- Best daily aggregate for HR (mean, resting estimate, last night)?
- Sleep detection quality good enough to feed sleep-debt rules?
- Preferred companion on Linux vs Android-only Gadgetbridge?

## References

- InfiniTime docs / BLE interface (upstream repo)
- Gadgetbridge PineTime device support
