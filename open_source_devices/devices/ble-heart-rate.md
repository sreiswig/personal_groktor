# BLE Heart Rate Service (generic)

| | |
|--|--|
| **Status** | Planned |
| **What** | Bluetooth GATT [Heart Rate Service](https://www.bluetooth.com/specifications/specs/heart-rate-service-1-0/) (UUID `0x180D`) |
| **Hardware** | Any compatible chest strap / arm band / watch exposing standard HRS |
| **Why** | Open standard; no vendor lock-in; great for workouts and HR trends |

## Metrics of interest

| Metric | Availability | Notes |
|--------|--------------|--------|
| Heart rate (bpm) | Yes | Real-time notifications |
| RR intervals | Sometimes | Useful for HRV derivation if exposed |
| Energy expended | Optional | Less useful for daily wellbeing |
| Sensor contact | Optional | Quality flag |

## Planned hook paths

1. **Session logger** (Rust BLE): subscribe during a workout → write CSV of samples + daily summary row  
2. **Night / rest sampling** daemon for RHR approximation (user-controlled)  
3. Derive **daily RHR** (e.g. lower quartile of overnight samples) → `resting_heart_rate_bpm`  
4. Optional **RMSSD HRV** from RR intervals → `heart_rate_variability_ms`

## Canonical mapping (draft)

| Derived field | `MetricKind` | Unit |
|---------------|--------------|------|
| daily resting estimate | `resting_heart_rate_bpm` | bpm |
| session mean HR | `Other("session_hr_bpm")` or notes | bpm |
| RMSSD / day | `heart_rate_variability_ms` | ms |

`source`: `ble-hrs` or `ble-hrs:<device-name>`

## Open questions

- Linux BLE stack choice (`btleplug`, etc.) and permissions model  
- How to avoid polluting baselines with workout-only days  
- Storage of high-frequency samples vs daily rollups only  

## References

- Bluetooth SIG Heart Rate Service  
- Many open HRM firmware projects expose this service
