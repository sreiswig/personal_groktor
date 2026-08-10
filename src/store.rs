//! Local SQLite persistence for metrics and findings.

use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::error::Result;
use crate::schema::{Finding, MetricCategory, MetricKind, MetricPoint, Severity};

pub struct Store {
    conn: Connection,
    path: PathBuf,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        let store = Self { conn, path };
        store.migrate()?;
        Ok(store)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS metrics (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                category TEXT NOT NULL,
                recorded_at TEXT NOT NULL,
                day TEXT NOT NULL,
                value REAL NOT NULL,
                unit TEXT NOT NULL,
                source TEXT NOT NULL,
                raw_name TEXT,
                notes TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_metrics_day ON metrics(day);
            CREATE INDEX IF NOT EXISTS idx_metrics_kind_day ON metrics(kind, day);

            CREATE TABLE IF NOT EXISTS findings (
                id TEXT PRIMARY KEY,
                day TEXT NOT NULL,
                kind TEXT NOT NULL,
                severity TEXT NOT NULL,
                title TEXT NOT NULL,
                detail TEXT NOT NULL,
                value REAL,
                baseline REAL,
                rule_id TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_findings_day ON findings(day);
            "#,
        )?;
        Ok(())
    }

    /// Insert metrics; skip duplicates by id. Returns number newly inserted.
    pub fn upsert_metrics(&self, points: &[MetricPoint]) -> Result<usize> {
        let mut inserted = 0usize;
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                r#"
                INSERT OR IGNORE INTO metrics
                    (id, kind, category, recorded_at, day, value, unit, source, raw_name, notes)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                "#,
            )?;
            for p in points {
                let n = stmt.execute(params![
                    p.id.to_string(),
                    p.kind.as_str(),
                    p.category.as_str(),
                    p.recorded_at.to_rfc3339(),
                    p.day.to_string(),
                    p.value,
                    p.unit,
                    p.source,
                    p.raw_name,
                    p.notes,
                ])?;
                inserted += n;
            }
        }
        tx.commit()?;
        Ok(inserted)
    }

    pub fn replace_findings_for_days(&self, days: &[NaiveDate], findings: &[Finding]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut del = tx.prepare("DELETE FROM findings WHERE day = ?1")?;
            for d in days {
                del.execute(params![d.to_string()])?;
            }
            let mut ins = tx.prepare(
                r#"
                INSERT INTO findings
                    (id, day, kind, severity, title, detail, value, baseline, rule_id)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                "#,
            )?;
            for f in findings {
                ins.execute(params![
                    f.id.to_string(),
                    f.day.to_string(),
                    f.kind.as_str(),
                    f.severity.as_str(),
                    f.title,
                    f.detail,
                    f.value,
                    f.baseline,
                    f.rule_id,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn metrics_in_range(
        &self,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Vec<MetricPoint>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, kind, category, recorded_at, day, value, unit, source, raw_name, notes
            FROM metrics
            WHERE day >= ?1 AND day <= ?2
            ORDER BY day, kind
            "#,
        )?;
        let rows = stmt.query_map(params![start.to_string(), end.to_string()], |row| {
            Ok(row_to_metric(row)?)
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn all_metrics(&self) -> Result<Vec<MetricPoint>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, kind, category, recorded_at, day, value, unit, source, raw_name, notes
            FROM metrics
            ORDER BY day, kind
            "#,
        )?;
        let rows = stmt.query_map([], |row| Ok(row_to_metric(row)?))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn findings_for_day(&self, day: NaiveDate) -> Result<Vec<Finding>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, day, kind, severity, title, detail, value, baseline, rule_id
            FROM findings
            WHERE day = ?1
            ORDER BY severity DESC, title
            "#,
        )?;
        let rows = stmt.query_map(params![day.to_string()], |row| Ok(row_to_finding(row)?))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn metric_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM metrics", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    pub fn day_range(&self) -> Result<Option<(NaiveDate, NaiveDate)>> {
        let min: Option<String> = self
            .conn
            .query_row("SELECT MIN(day) FROM metrics", [], |r| r.get(0))
            .optional()?
            .flatten();
        let max: Option<String> = self
            .conn
            .query_row("SELECT MAX(day) FROM metrics", [], |r| r.get(0))
            .optional()?
            .flatten();
        match (min, max) {
            (Some(a), Some(b)) => Ok(Some((
                NaiveDate::parse_from_str(&a, "%Y-%m-%d").map_err(|e| {
                    crate::error::GroktorError::Parse(format!("bad day in db: {e}"))
                })?,
                NaiveDate::parse_from_str(&b, "%Y-%m-%d").map_err(|e| {
                    crate::error::GroktorError::Parse(format!("bad day in db: {e}"))
                })?,
            ))),
            _ => Ok(None),
        }
    }
}

fn row_to_metric(row: &rusqlite::Row<'_>) -> rusqlite::Result<MetricPoint> {
    let id: String = row.get(0)?;
    let kind_s: String = row.get(1)?;
    let cat_s: String = row.get(2)?;
    let recorded_at: String = row.get(3)?;
    let day_s: String = row.get(4)?;
    let value: f64 = row.get(5)?;
    let unit: String = row.get(6)?;
    let source: String = row.get(7)?;
    let raw_name: Option<String> = row.get(8)?;
    let notes: Option<String> = row.get(9)?;

    let kind = MetricKind::from_raw_name(&kind_s);
    let category = MetricCategory::parse(&cat_s);
    let recorded_at = DateTime::parse_from_rfc3339(&recorded_at)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let day = NaiveDate::parse_from_str(&day_s, "%Y-%m-%d")
        .unwrap_or_else(|_| Utc::now().date_naive());

    Ok(MetricPoint {
        id: Uuid::parse_str(&id).unwrap_or_else(|_| Uuid::new_v4()),
        kind,
        category,
        recorded_at,
        day,
        value,
        unit,
        source,
        raw_name,
        notes,
    })
}

fn row_to_finding(row: &rusqlite::Row<'_>) -> rusqlite::Result<Finding> {
    let id: String = row.get(0)?;
    let day_s: String = row.get(1)?;
    let kind_s: String = row.get(2)?;
    let sev_s: String = row.get(3)?;
    let title: String = row.get(4)?;
    let detail: String = row.get(5)?;
    let value: Option<f64> = row.get(6)?;
    let baseline: Option<f64> = row.get(7)?;
    let rule_id: String = row.get(8)?;

    let severity = match sev_s.as_str() {
        "high" => Severity::High,
        "medium" => Severity::Medium,
        "low" => Severity::Low,
        _ => Severity::Info,
    };
    let day = NaiveDate::parse_from_str(&day_s, "%Y-%m-%d")
        .unwrap_or_else(|_| Utc::now().date_naive());

    Ok(Finding {
        id: Uuid::parse_str(&id).unwrap_or_else(|_| Uuid::new_v4()),
        day,
        kind: MetricKind::from_raw_name(&kind_s),
        severity,
        title,
        detail,
        value,
        baseline,
        rule_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::MetricKind;
    use tempfile::TempDir;

    #[test]
    fn round_trip_metrics() {
        let dir = TempDir::new().unwrap();
        let store = Store::open(dir.path().join("test.db")).unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();
        let p = MetricPoint::new(
            MetricKind::Steps,
            Utc::now(),
            day,
            9000.0,
            "count",
            "test",
        );
        assert_eq!(store.upsert_metrics(&[p.clone()]).unwrap(), 1);
        assert_eq!(store.upsert_metrics(&[p]).unwrap(), 0); // duplicate id
        assert_eq!(store.metric_count().unwrap(), 1);
        let got = store.metrics_in_range(day, day).unwrap();
        assert_eq!(got.len(), 1);
        assert!((got[0].value - 9000.0).abs() < f64::EPSILON);
    }
}
