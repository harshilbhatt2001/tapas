//! The tapas domain. Pure: no I/O, so the engine can be unit-tested and later compiled elsewhere.

pub mod engine;

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
    /// Where the reference path starts. A checkpoint without one starts where the previous
    /// checkpoint in its stack ends, in date and value.
    pub start: Option<NaiveDate>,
    /// Controlled goals: the series steered on a straight path from `start_value` to
    /// `target_value`.
    pub series_id: Option<i64>,
    /// For an [`Aggregation::Sum`] series, a running total since `start`.
    pub start_value: Option<f64>,
    /// The goal merges once its series reaches this. For an [`Aggregation::Sum`] series, a
    /// running total since `start`.
    pub target_value: Option<f64>,
    /// Milestones: the day it was ticked done.
    pub done_on: Option<NaiveDate>,
    /// Share of the parent's error.
    pub weight: f64,
    /// Weekly time this goal needs.
    pub weekly_hours: Option<f64>,
    /// Master only: total weekly hours available.
    pub capacity_hours: Option<f64>,
    /// Largest allowed week-on-week rise of the weekly amount, as a fraction (0.1 = +10 %).
    pub max_ramp: Option<f64>,
}

/// A weekly series a controlled goal can steer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Series {
    pub id: i64,
    pub name: String,
    pub unit: String,
    pub aggregation: Aggregation,
    pub feed: Feed,
}

/// How a week's readings collapse into one value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Aggregation {
    /// The week's last reading.
    Level,
    /// The week's total.
    Sum,
}

/// Where a series' readings come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Feed {
    /// Entered in the app.
    Manual,
    /// Google Health weights.
    BodyWeight,
    /// Active time of all workouts, in hours.
    TrainingHours,
}

/// A series' value for one ISO week.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Observation {
    /// The ISO week's Monday.
    pub week: NaiveDate,
    pub value: f64,
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
