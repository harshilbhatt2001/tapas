//! The tapas domain. Pure: no I/O, so the engine can be unit-tested and later compiled elsewhere.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// A node in the goal tree. The root is the single [`GoalKind::Master`]; checkpoints are child
/// goals stacked under their parent by due date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Goal {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub title: String,
    pub kind: GoalKind,
    pub due: Option<NaiveDate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalKind {
    /// The root: watches total capacity and the whole tree's error.
    Master,
    /// Tracks a metric against a reference path with its own loop.
    Controlled,
    /// Done or not by its due date; adds error only once overdue.
    Milestone,
}

/// Where a measurement or workout was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    GoogleHealth,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    BodyWeightKg,
}

/// One reading of a metric at an instant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Measurement {
    pub metric: Metric,
    pub at: DateTime<Utc>,
    pub value: f64,
    pub source: Source,
}

/// A completed activity as its source reported it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Workout {
    pub source: Source,
    /// The source's own id, unique per source.
    pub source_id: String,
    /// The source's activity type, e.g. Google's `RUNNING`.
    pub activity: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub active_secs: Option<u32>,
    pub distance_m: Option<f64>,
    pub avg_hr: Option<u16>,
    pub kcal: Option<f64>,
}
