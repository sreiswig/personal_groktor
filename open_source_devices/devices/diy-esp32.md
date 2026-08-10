# DIY ESP32 / MCU sensor nodes

| | |
|--|--|
| **Status** | Planned |
| **Hardware** | ESP32 / ESP32-C3 / nRF52 / similar + breakout sensors |
| **Firmware** | ESPHome, ESP-IDF, Zephyr, Arduino, or custom Rust (`esp-rs`) |
| **Why** | Fully owned pipeline; bedroom HR, scales, environment + health proxies |

## Example node ideas

| Node | Sensors | Metrics |
|------|---------|---------|
| Nightstand HR | MAX30102 / similar | SpO₂, pulse (experimental) |
| Desk presence / stress proxy | HR belt receiver, or temp/humidity | activity context |
| DIY scale bridge | HX711 load cell | `weight_kg` |
| Sleep environment | BME280, lux, mic level | context metrics (`Other`) |

**Caution:** fingertip SpO₂/HR DIY modules are noisy and **not** clinical grade. Label clearly in `notes` / `source`.

## Planned hook paths

1. **HTTP POST** JSON array of metric rows to a tiny collector that writes files for `ingest`  
2. **MQTT** → bridge script → CSV/JSON  
3. **Periodic CSV** on SD / flashed via USB  
4. ESPHome + Home Assistant history export → normalize (heavier)

## Canonical mapping (draft)

| DIY field | `MetricKind` |
|-----------|--------------|
| weight | `weight_kg` |
| spo2 | `spo2_pct` |
| pulse resting | `resting_heart_rate_bpm` |
| room context | `Other("room_temp_c")` etc. |

`source`: `esp32:<node-id>` (stable id per device)

## Open questions

- Official “Groktor node” JSON schema versioning?  
- Whether context metrics get first-class categories later  

## References

- ESPHome, `esp-rs`, Zephyr samples  
- Common breakouts: MAX30102, MPU6050, HX711, BME280
