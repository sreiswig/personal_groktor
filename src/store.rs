//! Local SQLite persistence for metrics, findings, annotations, experiments, digests.

use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use crate::error::{GroktorError, Result};
use crate::schema::{
    Annotation, Arm, Digest, DigestConfidence, DigestHorizon, Experiment, ExperimentDay,
    ExperimentStatus, Finding, MetricCategory, MetricKind, MetricPoint, OutcomeSpec, Severity,
};

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

            CREATE TABLE IF NOT EXISTS annotations (
                id TEXT PRIMARY KEY,
                day TEXT NOT NULL,
                recorded_at TEXT NOT NULL,
                tags TEXT NOT NULL,
                body TEXT,
                mood INTEGER,
                energy INTEGER,
                experiment_id TEXT,
                source TEXT NOT NULL DEFAULT 'manual'
            );
            CREATE INDEX IF NOT EXISTS idx_annotations_day ON annotations(day);
            CREATE INDEX IF NOT EXISTS idx_annotations_experiment ON annotations(experiment_id);

            CREATE TABLE IF NOT EXISTS experiments (
                id TEXT PRIMARY KEY,
                slug TEXT NOT NULL UNIQUE,
                title TEXT NOT NULL,
                hypothesis TEXT NOT NULL,
                status TEXT NOT NULL,
                started_on TEXT,
                ended_on TEXT,
                outcomes_json TEXT NOT NULL,
                min_days INTEGER NOT NULL DEFAULT 14,
                notes TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS experiment_days (
                experiment_id TEXT NOT NULL,
                day TEXT NOT NULL,
                arm TEXT NOT NULL,
                note TEXT,
                PRIMARY KEY (experiment_id, day),
                FOREIGN KEY (experiment_id) REFERENCES experiments(id)
            );

            CREATE TABLE IF NOT EXISTS digests (
                day TEXT NOT NULL,
                horizon TEXT NOT NULL,
                week_start TEXT,
                generated_at TEXT NOT NULL,
                metric_count INTEGER NOT NULL,
                summary TEXT NOT NULL,
                llm_backend TEXT,
                llm_narrative TEXT,
                findings_json TEXT NOT NULL,
                annotations_json TEXT,
                experiments_json TEXT,
                research_json TEXT,
                confidence_json TEXT,
                PRIMARY KEY (day, horizon)
            );
            "#,
        )?;
        Ok(())
    }

    // ── Metrics ──────────────────────────────────────────────────────────────

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

    pub fn replace_findings_for_days(
        &self,
        days: &[NaiveDate],
        findings: &[Finding],
    ) -> Result<()> {
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

    /// Daily values for one metric kind in range (last value wins per day).
    pub fn metric_series(
        &self,
        kind: &MetricKind,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Vec<(NaiveDate, f64)>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT day, value FROM metrics
            WHERE kind = ?1 AND day >= ?2 AND day <= ?3
            ORDER BY day, recorded_at
            "#,
        )?;
        let rows = stmt.query_map(
            params![kind.as_str(), start.to_string(), end.to_string()],
            |row| {
                let day_s: String = row.get(0)?;
                let value: f64 = row.get(1)?;
                Ok((day_s, value))
            },
        )?;
        let mut map = std::collections::BTreeMap::new();
        for r in rows {
            let (day_s, value) = r?;
            let day = NaiveDate::parse_from_str(&day_s, "%Y-%m-%d").map_err(|e| {
                GroktorError::Parse(format!("bad day in db: {e}"))
            })?;
            map.insert(day, value);
        }
        Ok(map.into_iter().collect())
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
        self.findings_in_range(day, day)
    }

    pub fn findings_in_range(&self, start: NaiveDate, end: NaiveDate) -> Result<Vec<Finding>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, day, kind, severity, title, detail, value, baseline, rule_id
            FROM findings
            WHERE day >= ?1 AND day <= ?2
            ORDER BY day, severity DESC, title
            "#,
        )?;
        let rows = stmt.query_map(params![start.to_string(), end.to_string()], |row| {
            Ok(row_to_finding(row)?)
        })?;
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

    pub fn finding_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM findings", [], |r| r.get(0))?;
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
            (Some(a), Some(b)) => Ok(Some((parse_day(&a)?, parse_day(&b)?))),
            _ => Ok(None),
        }
    }

    // ── Annotations ──────────────────────────────────────────────────────────

    pub fn insert_annotation(&self, a: &Annotation) -> Result<()> {
        let tags = serde_json::to_string(&a.tags)?;
        self.conn.execute(
            r#"
            INSERT INTO annotations
                (id, day, recorded_at, tags, body, mood, energy, experiment_id, source)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
            params![
                a.id.to_string(),
                a.day.to_string(),
                a.recorded_at.to_rfc3339(),
                tags,
                a.body,
                a.mood.map(|m| m as i64),
                a.energy.map(|e| e as i64),
                a.experiment_id.map(|id| id.to_string()),
                a.source,
            ],
        )?;
        Ok(())
    }

    pub fn annotations_in_range(
        &self,
        start: NaiveDate,
        end: NaiveDate,
    ) -> Result<Vec<Annotation>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, day, recorded_at, tags, body, mood, energy, experiment_id, source
            FROM annotations
            WHERE day >= ?1 AND day <= ?2
            ORDER BY day, recorded_at
            "#,
        )?;
        let rows = stmt.query_map(params![start.to_string(), end.to_string()], |row| {
            Ok(row_to_annotation(row)?)
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn annotations_for_day(&self, day: NaiveDate) -> Result<Vec<Annotation>> {
        self.annotations_in_range(day, day)
    }

    pub fn annotations_with_tag(
        &self,
        tag: &str,
        start: Option<NaiveDate>,
        end: Option<NaiveDate>,
    ) -> Result<Vec<Annotation>> {
        let start = start.unwrap_or_else(|| NaiveDate::from_ymd_opt(1970, 1, 1).unwrap());
        let end = end.unwrap_or_else(|| NaiveDate::from_ymd_opt(2100, 1, 1).unwrap());
        let tag_l = tag.trim().to_ascii_lowercase();
        let all = self.annotations_in_range(start, end)?;
        Ok(all
            .into_iter()
            .filter(|a| a.tags.iter().any(|t| t == &tag_l))
            .collect())
    }

    pub fn annotation_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM annotations", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    // ── Experiments ──────────────────────────────────────────────────────────

    pub fn insert_experiment(&self, e: &Experiment) -> Result<()> {
        let outcomes = serde_json::to_string(&e.outcomes)?;
        self.conn.execute(
            r#"
            INSERT INTO experiments
                (id, slug, title, hypothesis, status, started_on, ended_on,
                 outcomes_json, min_days, notes, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            "#,
            params![
                e.id.to_string(),
                e.slug,
                e.title,
                e.hypothesis,
                e.status.as_str(),
                e.started_on.map(|d| d.to_string()),
                e.ended_on.map(|d| d.to_string()),
                outcomes,
                e.min_days as i64,
                e.notes,
                e.created_at.to_rfc3339(),
                e.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn update_experiment(&self, e: &Experiment) -> Result<()> {
        let outcomes = serde_json::to_string(&e.outcomes)?;
        let n = self.conn.execute(
            r#"
            UPDATE experiments SET
                title = ?2, hypothesis = ?3, status = ?4, started_on = ?5, ended_on = ?6,
                outcomes_json = ?7, min_days = ?8, notes = ?9, updated_at = ?10
            WHERE id = ?1
            "#,
            params![
                e.id.to_string(),
                e.title,
                e.hypothesis,
                e.status.as_str(),
                e.started_on.map(|d| d.to_string()),
                e.ended_on.map(|d| d.to_string()),
                outcomes,
                e.min_days as i64,
                e.notes,
                e.updated_at.to_rfc3339(),
            ],
        )?;
        if n == 0 {
            return Err(GroktorError::Message(format!(
                "experiment not found: {}",
                e.slug
            )));
        }
        Ok(())
    }

    pub fn experiment_by_slug(&self, slug: &str) -> Result<Option<Experiment>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, slug, title, hypothesis, status, started_on, ended_on,
                   outcomes_json, min_days, notes, created_at, updated_at
            FROM experiments WHERE slug = ?1
            "#,
        )?;
        let mut rows = stmt.query_map(params![slug], |row| Ok(row_to_experiment(row)?))?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    pub fn require_experiment(&self, slug: &str) -> Result<Experiment> {
        self.experiment_by_slug(slug)?.ok_or_else(|| {
            GroktorError::Message(format!("unknown experiment slug: {slug}"))
        })
    }

    pub fn list_experiments(&self) -> Result<Vec<Experiment>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT id, slug, title, hypothesis, status, started_on, ended_on,
                   outcomes_json, min_days, notes, created_at, updated_at
            FROM experiments
            ORDER BY created_at DESC
            "#,
        )?;
        let rows = stmt.query_map([], |row| Ok(row_to_experiment(row)?))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn active_experiments(&self) -> Result<Vec<Experiment>> {
        Ok(self
            .list_experiments()?
            .into_iter()
            .filter(|e| e.status == ExperimentStatus::Active)
            .collect())
    }

    pub fn experiment_counts(&self) -> Result<(usize, usize)> {
        let active: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM experiments WHERE status = 'active'",
            [],
            |r| r.get(0),
        )?;
        let completed: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM experiments WHERE status = 'completed'",
            [],
            |r| r.get(0),
        )?;
        Ok((active as usize, completed as usize))
    }

    pub fn upsert_experiment_day(&self, ed: &ExperimentDay) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO experiment_days (experiment_id, day, arm, note)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(experiment_id, day) DO UPDATE SET
                arm = excluded.arm,
                note = excluded.note
            "#,
            params![
                ed.experiment_id.to_string(),
                ed.day.to_string(),
                ed.arm.as_str(),
                ed.note,
            ],
        )?;
        Ok(())
    }

    pub fn experiment_days(&self, experiment_id: Uuid) -> Result<Vec<ExperimentDay>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT experiment_id, day, arm, note
            FROM experiment_days
            WHERE experiment_id = ?1
            ORDER BY day
            "#,
        )?;
        let rows = stmt.query_map(params![experiment_id.to_string()], |row| {
            Ok(row_to_experiment_day(row)?)
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn arm_for_day(
        &self,
        experiment_id: Uuid,
        day: NaiveDate,
    ) -> Result<Option<Arm>> {
        let arm_s: Option<String> = self
            .conn
            .query_row(
                "SELECT arm FROM experiment_days WHERE experiment_id = ?1 AND day = ?2",
                params![experiment_id.to_string(), day.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(arm_s.and_then(|s| Arm::parse(&s)))
    }

    // ── Digests cache ────────────────────────────────────────────────────────

    pub fn upsert_digest(&self, d: &Digest) -> Result<()> {
        let horizon = d.horizon.as_str();
        let week_start = match d.horizon {
            DigestHorizon::Week { start } => Some(start.to_string()),
            DigestHorizon::Day => None,
        };
        let findings_json = serde_json::to_string(&d.findings)?;
        let annotations_json = serde_json::to_string(&d.annotations)?;
        let experiments_json = serde_json::to_string(&d.active_experiments)?;
        let research_json = serde_json::to_string(&d.research_bits)?;
        let confidence_json = serde_json::to_string(&d.confidence)?;
        self.conn.execute(
            r#"
            INSERT INTO digests
                (day, horizon, week_start, generated_at, metric_count, summary,
                 llm_backend, llm_narrative, findings_json, annotations_json,
                 experiments_json, research_json, confidence_json)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            ON CONFLICT(day, horizon) DO UPDATE SET
                week_start = excluded.week_start,
                generated_at = excluded.generated_at,
                metric_count = excluded.metric_count,
                summary = excluded.summary,
                llm_backend = excluded.llm_backend,
                llm_narrative = excluded.llm_narrative,
                findings_json = excluded.findings_json,
                annotations_json = excluded.annotations_json,
                experiments_json = excluded.experiments_json,
                research_json = excluded.research_json,
                confidence_json = excluded.confidence_json
            "#,
            params![
                d.day.to_string(),
                horizon,
                week_start,
                d.generated_at.to_rfc3339(),
                d.metric_count as i64,
                d.summary,
                d.llm_backend,
                d.llm_narrative,
                findings_json,
                annotations_json,
                experiments_json,
                research_json,
                confidence_json,
            ],
        )?;
        Ok(())
    }

    pub fn get_digest(
        &self,
        day: NaiveDate,
        horizon: &str,
    ) -> Result<Option<Digest>> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT day, horizon, week_start, generated_at, metric_count, summary,
                   llm_backend, llm_narrative, findings_json, annotations_json,
                   experiments_json, research_json, confidence_json
            FROM digests WHERE day = ?1 AND horizon = ?2
            "#,
        )?;
        let mut rows = stmt.query_map(params![day.to_string(), horizon], |row| {
            Ok(row_to_digest(row)?)
        })?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }
}

// ── Row mappers ──────────────────────────────────────────────────────────────

fn parse_day(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| GroktorError::Parse(format!("bad day in db: {e}")))
}

fn parse_rfc3339(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
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
    let recorded_at = parse_rfc3339(&recorded_at);
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

fn row_to_annotation(row: &rusqlite::Row<'_>) -> std::result::Result<Annotation, rusqlite::Error> {
    let id: String = row.get(0)?;
    let day_s: String = row.get(1)?;
    let recorded_at: String = row.get(2)?;
    let tags_s: String = row.get(3)?;
    let body: Option<String> = row.get(4)?;
    let mood: Option<i64> = row.get(5)?;
    let energy: Option<i64> = row.get(6)?;
    let experiment_id: Option<String> = row.get(7)?;
    let source: String = row.get(8)?;

    let tags: Vec<String> = serde_json::from_str(&tags_s).unwrap_or_default();
    let day = NaiveDate::parse_from_str(&day_s, "%Y-%m-%d")
        .unwrap_or_else(|_| Utc::now().date_naive());

    Ok(Annotation {
        id: Uuid::parse_str(&id).unwrap_or_else(|_| Uuid::new_v4()),
        day,
        recorded_at: parse_rfc3339(&recorded_at),
        tags,
        body,
        mood: mood.map(|m| m.clamp(1, 5) as u8),
        energy: energy.map(|e| e.clamp(1, 5) as u8),
        experiment_id: experiment_id.and_then(|s| Uuid::parse_str(&s).ok()),
        source,
    })
}

fn row_to_experiment(row: &rusqlite::Row<'_>) -> std::result::Result<Experiment, rusqlite::Error> {
    let id: String = row.get(0)?;
    let slug: String = row.get(1)?;
    let title: String = row.get(2)?;
    let hypothesis: String = row.get(3)?;
    let status_s: String = row.get(4)?;
    let started_on: Option<String> = row.get(5)?;
    let ended_on: Option<String> = row.get(6)?;
    let outcomes_json: String = row.get(7)?;
    let min_days: i64 = row.get(8)?;
    let notes: Option<String> = row.get(9)?;
    let created_at: String = row.get(10)?;
    let updated_at: String = row.get(11)?;

    let outcomes: Vec<OutcomeSpec> = serde_json::from_str(&outcomes_json).unwrap_or_default();
    let status = ExperimentStatus::parse(&status_s).unwrap_or(ExperimentStatus::Draft);

    Ok(Experiment {
        id: Uuid::parse_str(&id).unwrap_or_else(|_| Uuid::new_v4()),
        slug,
        title,
        hypothesis,
        status,
        started_on: started_on.and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()),
        ended_on: ended_on.and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()),
        outcomes,
        min_days: min_days.max(0) as u32,
        notes,
        created_at: parse_rfc3339(&created_at),
        updated_at: parse_rfc3339(&updated_at),
    })
}

fn row_to_experiment_day(
    row: &rusqlite::Row<'_>,
) -> std::result::Result<ExperimentDay, rusqlite::Error> {
    let eid: String = row.get(0)?;
    let day_s: String = row.get(1)?;
    let arm_s: String = row.get(2)?;
    let note: Option<String> = row.get(3)?;
    Ok(ExperimentDay {
        experiment_id: Uuid::parse_str(&eid).unwrap_or_else(|_| Uuid::new_v4()),
        day: NaiveDate::parse_from_str(&day_s, "%Y-%m-%d")
            .unwrap_or_else(|_| Utc::now().date_naive()),
        arm: Arm::parse(&arm_s).unwrap_or(Arm::Exclude),
        note,
    })
}

fn row_to_digest(row: &rusqlite::Row<'_>) -> std::result::Result<Digest, rusqlite::Error> {
    use crate::schema::{ExperimentBrief, ResearchCite};

    let day_s: String = row.get(0)?;
    let horizon_s: String = row.get(1)?;
    let week_start: Option<String> = row.get(2)?;
    let generated_at: String = row.get(3)?;
    let metric_count: i64 = row.get(4)?;
    let summary: String = row.get(5)?;
    let llm_backend: Option<String> = row.get(6)?;
    let llm_narrative: Option<String> = row.get(7)?;
    let findings_json: String = row.get(8)?;
    let annotations_json: Option<String> = row.get(9)?;
    let experiments_json: Option<String> = row.get(10)?;
    let research_json: Option<String> = row.get(11)?;
    let confidence_json: Option<String> = row.get(12)?;

    let day = NaiveDate::parse_from_str(&day_s, "%Y-%m-%d")
        .unwrap_or_else(|_| Utc::now().date_naive());
    let horizon = if horizon_s == "week" {
        let start = week_start
            .and_then(|s| NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok())
            .unwrap_or(day - chrono::Duration::days(6));
        DigestHorizon::Week { start }
    } else {
        DigestHorizon::Day
    };

    let findings: Vec<Finding> = serde_json::from_str(&findings_json).unwrap_or_default();
    let annotations: Vec<Annotation> = annotations_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let active_experiments: Vec<ExperimentBrief> = experiments_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let research_bits: Vec<ResearchCite> = research_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let confidence: DigestConfidence = confidence_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(DigestConfidence {
            level: crate::schema::ConfidenceLevel::Ok,
            reasons: vec![],
        });

    Ok(Digest {
        day,
        horizon,
        generated_at: parse_rfc3339(&generated_at),
        findings,
        annotations,
        active_experiments,
        summary,
        llm_narrative,
        llm_backend,
        metric_count: metric_count.max(0) as usize,
        research_bits,
        confidence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Direction, MetricKind};
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
        assert_eq!(store.upsert_metrics(&[p]).unwrap(), 0);
        assert_eq!(store.metric_count().unwrap(), 1);
        let got = store.metrics_in_range(day, day).unwrap();
        assert_eq!(got.len(), 1);
        assert!((got[0].value - 9000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn annotations_and_experiments() {
        let dir = TempDir::new().unwrap();
        let store = Store::open(dir.path().join("test.db")).unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();

        let mut a = Annotation::new(day, vec!["alcohol".into(), "late".into()], Some("wine".into()));
        a.mood = Some(3);
        store.insert_annotation(&a).unwrap();
        assert_eq!(store.annotation_count().unwrap(), 1);
        let got = store.annotations_for_day(day).unwrap();
        assert_eq!(got[0].tags, vec!["alcohol", "late"]);

        let now = Utc::now();
        let exp = Experiment {
            id: Uuid::new_v4(),
            slug: "early_bed".into(),
            title: "Earlier bedtime".into(),
            hypothesis: "Bed early improves HRV".into(),
            status: ExperimentStatus::Active,
            started_on: Some(day),
            ended_on: None,
            outcomes: vec![OutcomeSpec {
                kind: MetricKind::HeartRateVariabilityMs,
                direction: Direction::Up,
                primary: true,
            }],
            min_days: 14,
            notes: None,
            created_at: now,
            updated_at: now,
        };
        store.insert_experiment(&exp).unwrap();
        store
            .upsert_experiment_day(&ExperimentDay {
                experiment_id: exp.id,
                day,
                arm: Arm::Intervention,
                note: None,
            })
            .unwrap();

        let loaded = store.require_experiment("early_bed").unwrap();
        assert_eq!(loaded.outcomes.len(), 1);
        assert_eq!(
            store.arm_for_day(exp.id, day).unwrap(),
            Some(Arm::Intervention)
        );
        let (active, completed) = store.experiment_counts().unwrap();
        assert_eq!((active, completed), (1, 0));
    }
}
