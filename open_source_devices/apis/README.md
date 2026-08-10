# Vendor APIs & official exports

Cloud and phone-ecosystem sources that often **already hold** years of user history. These sit **alongside** [open devices](../README.md), not instead of them.

Nothing here is implemented yet; this is the planned inventory for future adapters under `src/ingest/`.

## Comparison

| Source type | Example | Auth | Offline? | Typical history |
|-------------|---------|------|----------|-----------------|
| Open device / local bridge | PineTime, Gadgetbridge | None / local | Yes | From when you started local capture |
| Official export | Apple Health zip, Fitbit export | User download | After export | Often multi-year |
| Official API | Oura, Fitbit Web API | OAuth / token | No | Deep if account is old |
| Unofficial reverse-eng | Some watch clouds | Fragile | No | Avoid if local path exists |

**Preference order for Personal Groktor:** local device → official export file → official API → unofficial cloud scrapers (last resort).

## Planned API / export adapters

| Source | Access style | Metrics (typical) | Priority notes |
|--------|--------------|-------------------|----------------|
| **Apple Health** | Export XML / ZIP | Broad (sleep, HR, HRV, steps, workouts) | High — huge install base |
| **Google Fit / Health Connect** | Export or Android APIs | Activity, some vitals | High on Android |
| **Fitbit** | Bulk export + Web API | Sleep, HR, steps, exercise | Export-first for privacy |
| **Oura** | Cloud API + app export | Sleep, readiness, HR, HRV | Strong recovery metrics |
| **Garmin** | Export / Connect IQ / API | Broad athletic + sleep | File export preferred |
| **Withings** | API | Weight, BP, sleep (scales/mats) | Good for `body` category |
| **Whoop** | API (access policy varies) | Strain, recovery, sleep | Confirm ToS / API access |
| **Polar** | Flow export / API | Training, HR | BLE HRS also overlaps open path |
| **Samsung Health** | Export | Broad | Format churn risk |
| **Strava** | API | Workouts (activity context) | Supplement, not RHR/HRV core |

## How APIs relate to open devices

```text
User may have:
  • years of Oura/Fitbit in the cloud   → backfill via export/API once
  • daily PineTime / Bangle going forward → open device path ongoing

Both land in the same SQLite store with different `source` tags.
Rules and digests merge by day; sources remain visible for explainability.
```

## Implementation sketch (future)

```text
src/ingest/
  csv_source.rs      # exists
  json_source.rs     # exists
  apple_health.rs    # planned
  fitbit.rs          # planned
  oura_api.rs        # planned
  devices/
    pinetime.rs      # planned
    gadgetbridge.rs  # planned
    ble_hrs.rs       # planned
```

## Privacy defaults (draft policy)

- Store API tokens only in env / user keyring, never in the DB.  
- Document exactly which endpoints are called.  
- Offer export-file paths that never need long-lived cloud tokens.  
- Digest `--llm` already optional; keep metric analysis local regardless of source.
