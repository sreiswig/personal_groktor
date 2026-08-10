//! Common health metric schema used across ingest, storage, and analysis.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// High-level metric families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricCategory {
    Sleep,
    Heart,
    Activity,
    Body,
    Recovery,
    Other,
}

impl MetricCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sleep => "sleep",
            Self::Heart => "heart",
            Self::Activity => "activity",
            Self::Body => "body",
            Self::Recovery => "recovery",
            Self::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "sleep" => Self::Sleep,
            "heart" | "hr" | "hrv" => Self::Heart,
            "activity" | "steps" | "exercise" => Self::Activity,
            "body" | "weight" => Self::Body,
            "recovery" | "readiness" => Self::Recovery,
            _ => Self::Other,
        }
    }
}

impl std::fmt::Display for MetricCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Canonical metric names used after normalization.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    SleepDurationHours,
    SleepEfficiencyPct,
    RestingHeartRateBpm,
    HeartRateVariabilityMs,
    Steps,
    ActiveCaloriesKcal,
    ExerciseMinutes,
    WeightKg,
    BodyFatPct,
    ReadinessScore,
    Spo2Pct,
    Other(String),
}

impl MetricKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::SleepDurationHours => "sleep_duration_hours",
            Self::SleepEfficiencyPct => "sleep_efficiency_pct",
            Self::RestingHeartRateBpm => "resting_heart_rate_bpm",
            Self::HeartRateVariabilityMs => "heart_rate_variability_ms",
            Self::Steps => "steps",
            Self::ActiveCaloriesKcal => "active_calories_kcal",
            Self::ExerciseMinutes => "exercise_minutes",
            Self::WeightKg => "weight_kg",
            Self::BodyFatPct => "body_fat_pct",
            Self::ReadinessScore => "readiness_score",
            Self::Spo2Pct => "spo2_pct",
            Self::Other(s) => s.as_str(),
        }
    }

    pub fn category(&self) -> MetricCategory {
        match self {
            Self::SleepDurationHours | Self::SleepEfficiencyPct => MetricCategory::Sleep,
            Self::RestingHeartRateBpm | Self::HeartRateVariabilityMs | Self::Spo2Pct => {
                MetricCategory::Heart
            }
            Self::Steps | Self::ActiveCaloriesKcal | Self::ExerciseMinutes => {
                MetricCategory::Activity
            }
            Self::WeightKg | Self::BodyFatPct => MetricCategory::Body,
            Self::ReadinessScore => MetricCategory::Recovery,
            Self::Other(_) => MetricCategory::Other,
        }
    }

    /// Map common export column / type names onto canonical kinds.
    pub fn from_raw_name(raw: &str) -> Self {
        let key = raw
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-'], "_");
        match key.as_str() {
            "sleep_duration" | "sleep_duration_hours" | "total_sleep_hours" | "sleep_hours" => {
                Self::SleepDurationHours
            }
            "sleep_efficiency" | "sleep_efficiency_pct" | "efficiency" => Self::SleepEfficiencyPct,
            "resting_heart_rate" | "resting_hr" | "rhr" | "resting_heart_rate_bpm" => {
                Self::RestingHeartRateBpm
            }
            "hrv" | "heart_rate_variability" | "heart_rate_variability_ms" | "rmssd" => {
                Self::HeartRateVariabilityMs
            }
            "steps" | "step_count" | "daily_steps" => Self::Steps,
            "active_calories" | "active_calories_kcal" | "calories_burned" | "active_energy" => {
                Self::ActiveCaloriesKcal
            }
            "exercise_minutes" | "workout_minutes" | "active_minutes" => Self::ExerciseMinutes,
            "weight" | "weight_kg" | "body_weight" => Self::WeightKg,
            "body_fat" | "body_fat_pct" | "body_fat_percent" => Self::BodyFatPct,
            "readiness" | "readiness_score" | "recovery_score" => Self::ReadinessScore,
            "spo2" | "spo2_pct" | "blood_oxygen" | "oxygen_saturation" => Self::Spo2Pct,
            other => Self::Other(other.to_string()),
        }
    }
}

impl std::fmt::Display for MetricKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single normalized observation at a point in time (or for a calendar day).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricPoint {
    pub id: Uuid,
    pub kind: MetricKind,
    pub category: MetricCategory,
    /// Instant the measurement was recorded (UTC).
    pub recorded_at: DateTime<Utc>,
    /// Calendar day the metric applies to (local day when known, else UTC date).
    pub day: NaiveDate,
    pub value: f64,
    pub unit: String,
    pub source: String,
    pub raw_name: Option<String>,
    pub notes: Option<String>,
}

impl MetricPoint {
    pub fn new(
        kind: MetricKind,
        recorded_at: DateTime<Utc>,
        day: NaiveDate,
        value: f64,
        unit: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        let category = kind.category();
        Self {
            id: Uuid::new_v4(),
            kind,
            category,
            recorded_at,
            day,
            value,
            unit: unit.into(),
            source: source.into(),
            raw_name: None,
            notes: None,
        }
    }
}

/// A flagged anomaly or notable pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub id: Uuid,
    pub day: NaiveDate,
    pub kind: MetricKind,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    pub value: Option<f64>,
    pub baseline: Option<f64>,
    pub rule_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Daily wellbeing digest (rules + optional LLM narrative).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Digest {
    pub day: NaiveDate,
    pub generated_at: DateTime<Utc>,
    pub findings: Vec<Finding>,
    pub summary: String,
    pub llm_narrative: Option<String>,
    pub metric_count: usize,
}

/// Raw row as commonly found in wearable CSV/JSON exports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMetricRow {
    pub date: String,
    #[serde(default)]
    pub timestamp: Option<String>,
    pub metric: String,
    pub value: f64,
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}
