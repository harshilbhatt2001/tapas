//! The control loops: from the goal tree and its weekly series to each node's status and the
//! master's verdict. Errors are days behind schedule, so they add up across unlike metrics.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{Goal, Observation, Series};

/// A series with its weekly values, oldest first.
#[derive(Debug, Clone, PartialEq)]
pub struct Readings {
    pub series: Series,
    pub weeks: Vec<Observation>,
}

/// The whole tree's status on one day.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Course {
    /// One per goal, master included.
    pub nodes: Vec<NodeStatus>,
    pub master: MasterVerdict,
}

/// One goal's status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct NodeStatus {
    pub goal_id: i64,
    pub state: NodeState,
    /// Negative when ahead; `None` while pending, done or stale.
    pub days_behind: Option<f64>,
    pub terms: Option<LoopTerms>,
    pub projected_arrival: Option<NaiveDate>,
    /// Weekly amount that gets back to the path, clamped to the goal's limits.
    pub needed_per_week: Option<f64>,
    /// How far the rest of the stack can slip before the parent's due date.
    pub slack_days: Option<i64>,
    /// Why catching up breaks a limit.
    pub conflict: Option<String>,
    /// The reference path, one point per week.
    pub path: Vec<Observation>,
}

/// A loop's error terms, all in days.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct LoopTerms {
    /// The gap now.
    pub p: f64,
    /// The mean gap over a rolling window.
    pub i: f64,
    /// Change of the gap per week; positive means falling behind.
    pub d: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    /// Within tolerance of the reference path.
    OnCourse,
    /// Ahead of the reference path.
    Ahead,
    /// Behind the reference path, and catching up fits the limits.
    Behind,
    /// Catching up would break a limit.
    Conflict,
    /// The series has no recent readings.
    SensorStale,
    /// Its path has not started yet.
    Pending,
    /// Reached its target; the next goal in the stack takes over.
    Merged,
    /// Milestone not yet near its due date.
    Upcoming,
    /// Milestone due within days.
    DueSoon,
    /// Milestone ticked done.
    Done,
    /// Milestone past its due date and not done.
    Overdue,
}

/// The master goal's judgement of the whole tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct MasterVerdict {
    /// The weighted error of the whole tree.
    pub days_behind: f64,
    /// Weekly hours the active goals need.
    pub needed_hours: f64,
    pub capacity_hours: Option<f64>,
    /// `needed_hours` exceeds `capacity_hours`.
    pub overcommitted: bool,
    pub mode: Mode,
    /// Goals whose error keeps growing.
    pub drifting: Vec<i64>,
}

/// Which plan the week runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The full plan.
    A,
    /// The minimum viable fallback.
    B,
}

/// Every node's status and the master's verdict on `today`.
#[must_use]
pub fn course(goals: &[Goal], readings: &[Readings], today: NaiveDate) -> Course {
    let _ = (goals, readings, today);
    todo!("slice 1 engine")
}
