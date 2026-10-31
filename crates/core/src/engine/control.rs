//! One controlled goal's loop: its reference path, its error in days and its catch-up output.

use chrono::{NaiveDate, TimeDelta, Weekday};

use super::{
    LoopTerms, NodeState, NodeStatus, Readings, STALE_AFTER_DAYS, WINDOW_WEEKS, Window,
    blank_status, state_for,
};
use crate::{Aggregation, Goal, Observation};

/// The goal's status and, once its series reached the target, the week it merged.
pub(super) fn evaluate(
    goal: &Goal,
    window: Window,
    readings: Option<&Readings>,
    today: NaiveDate,
) -> (NodeStatus, Option<NaiveDate>) {
    let mut status = blank_status(goal.id);
    let plan = Plan::new(goal, window, readings);
    if window.blocked || plan.as_ref().is_ok_and(|plan| plan.start > today) {
        status.state = NodeState::Pending;
        status.path = plan.map(|plan| plan.path()).unwrap_or_default();
        return (status, None);
    }
    let plan = match plan {
        Ok(plan) => plan,
        Err(Unplanned::NoLoop) => return (status, None),
        Err(Unplanned::NoAnchor) => {
            status.state = NodeState::SensorStale;
            return (status, None);
        }
    };
    status.path = plan.path();

    let track = plan.track(today);
    if let Some(merge) = track.iter().find(|reading| plan.reached(reading.actual)) {
        status.state = NodeState::Merged;
        return (status, Some(merge.week));
    }
    let Some(now) = track
        .last()
        .filter(|now| (today - now.week).num_days() <= STALE_AFTER_DAYS)
    else {
        status.state = NodeState::SensorStale;
        return (status, None);
    };

    let terms = plan.terms(&track);
    status.days_behind = Some(terms.p);
    status.terms = Some(terms);
    status.projected_arrival = plan.projected_arrival(&track, now);
    match plan.catch_up(&track, today) {
        CatchUp::Fits { needed } => {
            status.state = state_for(terms.p);
            status.needed_per_week = Some(needed);
        }
        CatchUp::RampLimited { needed, allowed } => {
            let unit = &plan.readings.series.unit;
            status.state = NodeState::Conflict;
            status.needed_per_week = Some(allowed.copysign(needed));
            status.conflict = Some(format!(
                "needs {:.1} {unit}/wk, ramp limit allows {allowed:.1} {unit}/wk: \
                 move the deadline, lower the target or re-plan",
                needed.abs()
            ));
        }
        CatchUp::PastDue => {
            status.state = NodeState::Conflict;
            status.conflict = Some(format!(
                "due {} passed short of {} {}: move the deadline or lower the target",
                plan.due, plan.target, plan.readings.series.unit
            ));
        }
    }
    (status, None)
}

/// Why a controlled goal has no reference path.
enum Unplanned {
    /// Not set up as a loop: no series, target or due date, or due before it starts.
    NoLoop,
    /// Set up, but no reading anchors where the path starts.
    NoAnchor,
}

/// A straight reference path from (`start`, `start_value`) to (`due`, `target`).
struct Plan<'a> {
    readings: &'a Readings,
    start: NaiveDate,
    start_value: f64,
    due: NaiveDate,
    target: f64,
    max_ramp: Option<f64>,
}

/// One week of the series as the goal sees it.
#[derive(Debug, Clone, Copy)]
struct Reading {
    week: NaiveDate,
    /// When the week's value holds: the week's end, or today for the running week.
    as_of: NaiveDate,
    /// The week's own value.
    weekly: f64,
    /// y(t): the value itself for a level, the running total since the start for a sum.
    actual: f64,
}

/// What getting back to the path takes now.
enum CatchUp {
    Fits { needed: f64 },
    RampLimited { needed: f64, allowed: f64 },
    PastDue,
}

impl CatchUp {
    fn is_conflict(&self) -> bool {
        !matches!(self, Self::Fits { .. })
    }
}

impl<'a> Plan<'a> {
    fn new(goal: &Goal, window: Window, readings: Option<&'a Readings>) -> Result<Self, Unplanned> {
        let (Some(readings), Some(target), Some(due)) = (readings, goal.target_value, window.due)
        else {
            return Err(Unplanned::NoLoop);
        };
        let start = window
            .start
            .or_else(|| readings.weeks.first().map(|first| first.week))
            .ok_or(Unplanned::NoAnchor)?;
        if start >= due {
            return Err(Unplanned::NoLoop);
        }
        let start_week = monday(start);
        let start_value = goal
            .start_value
            .or(window.start_value)
            .or_else(|| match readings.series.aggregation {
                Aggregation::Sum => Some(0.0),
                Aggregation::Level => readings
                    .weeks
                    .iter()
                    .find(|observation| observation.week >= start_week)
                    .map(|observation| observation.value),
            })
            .ok_or(Unplanned::NoAnchor)?;
        Ok(Self {
            readings,
            start,
            start_value,
            due,
            target,
            max_ramp: goal.max_ramp,
        })
    }

    fn slope_per_day(&self) -> f64 {
        (self.target - self.start_value) / days(self.due - self.start)
    }

    fn value_on(&self, date: NaiveDate) -> f64 {
        self.start_value + self.slope_per_day() * days(date - self.start)
    }

    /// Days from the date the line passes `value` to `on`, extrapolating past either end.
    fn days_behind(&self, on: NaiveDate, value: f64) -> f64 {
        days(on - self.start) - (value - self.start_value) / self.slope_per_day()
    }

    fn reached(&self, value: f64) -> bool {
        (value - self.target) * (self.target - self.start_value) >= 0.0
    }

    fn path(&self) -> Vec<Observation> {
        monday(self.start)
            .iter_weeks()
            .take_while(|week| *week <= self.due)
            .map(|week| Observation {
                week,
                value: self.value_on(week),
            })
            .collect()
    }

    /// The readings from the start's week up to today.
    fn track(&self, today: NaiveDate) -> Vec<Reading> {
        let start_week = monday(self.start);
        let mut total = self.start_value;
        self.readings
            .weeks
            .iter()
            .filter(|observation| (start_week..=today).contains(&observation.week))
            .map(|observation| {
                let actual = match self.readings.series.aggregation {
                    Aggregation::Level => observation.value,
                    Aggregation::Sum => {
                        total += observation.value;
                        total
                    }
                };
                Reading {
                    week: observation.week,
                    as_of: observation
                        .week
                        .checked_add_signed(TimeDelta::weeks(1))
                        .map_or(today, |week_end| week_end.min(today)),
                    weekly: observation.value,
                    actual,
                }
            })
            .collect()
    }

    /// P, I and D over the weekly errors; weeks in conflict stay out of I.
    fn terms(&self, track: &[Reading]) -> LoopTerms {
        let errors: Vec<(NaiveDate, f64, bool)> = (0..track.len())
            .map(|end| {
                let reading = track[end];
                let conflicted = self.catch_up(&track[..=end], reading.as_of).is_conflict();
                (
                    reading.week,
                    self.days_behind(reading.as_of, reading.actual),
                    conflicted,
                )
            })
            .collect();
        let recent = &errors[recent_start(track)..];
        let (now_week, p, _) = recent[recent.len() - 1];

        let counted: Vec<f64> = recent
            .iter()
            .filter(|(_, _, conflicted)| !conflicted)
            .map(|&(_, error, _)| error)
            .collect();
        let i = mean(&counted).unwrap_or(0.0);

        let (first_week, first_error, _) = recent[0];
        let weeks = days(now_week - first_week) / 7.0;
        let d = if weeks > 0.0 {
            (p - first_error) / weeks
        } else {
            0.0
        };
        LoopTerms { p, i, d }
    }

    fn projected_arrival(&self, track: &[Reading], now: &Reading) -> Option<NaiveDate> {
        let rate = rate_per_week(&track[recent_start(track)..])?;
        let remaining = self.target - now.actual;
        (rate * remaining > 0.0)
            .then(|| add_days(now.as_of, remaining / rate * 7.0))
            .flatten()
    }

    /// The weekly amount that reaches the target by the due date from the last reading, judged
    /// against the ramp limit as it stood on `on`.
    fn catch_up(&self, track: &[Reading], on: NaiveDate) -> CatchUp {
        let Some(now) = track.last() else {
            return CatchUp::Fits { needed: 0.0 };
        };
        if self.due < on {
            return CatchUp::PastDue;
        }
        let weeks_left = days((self.due - on).max(TimeDelta::days(1))) / 7.0;
        let needed = (self.target - now.actual) / weeks_left;
        match self.ramp_allowance(track, on) {
            Some(allowed) if needed.abs() > allowed => CatchUp::RampLimited { needed, allowed },
            _ => CatchUp::Fits { needed },
        }
    }

    /// The largest weekly amount `max_ramp` allows on top of the recent one. A sum ramps from
    /// its last complete week, a level from its recent rate of change.
    fn ramp_allowance(&self, track: &[Reading], on: NaiveDate) -> Option<f64> {
        let max_ramp = self.max_ramp?;
        let recent = match self.readings.series.aggregation {
            Aggregation::Sum => {
                track
                    .iter()
                    .rev()
                    .find(|reading| {
                        reading
                            .week
                            .checked_add_signed(TimeDelta::weeks(1))
                            .is_some_and(|week_end| week_end <= on)
                    })?
                    .weekly
            }
            Aggregation::Level => rate_per_week(&track[recent_start(track)..])?.abs(),
        };
        (recent > 0.0).then_some(recent * (1.0 + max_ramp))
    }
}

/// Where the last [`WINDOW_WEEKS`] weeks of the track begin.
fn recent_start(track: &[Reading]) -> usize {
    let Some(from) = track.last().and_then(|last| {
        last.week
            .checked_sub_signed(TimeDelta::weeks(WINDOW_WEEKS - 1))
    }) else {
        return 0;
    };
    track.partition_point(|reading| reading.week < from)
}

fn rate_per_week(readings: &[Reading]) -> Option<f64> {
    let (first, last) = (readings.first()?, readings.last()?);
    let span = days(last.as_of - first.as_of);
    (span > 0.0).then(|| (last.actual - first.actual) / span * 7.0)
}

fn monday(date: NaiveDate) -> NaiveDate {
    date.week(Weekday::Mon).first_day()
}

#[expect(
    clippy::cast_precision_loss,
    reason = "day counts between calendar dates are far below 2^52"
)]
pub(super) fn days(delta: TimeDelta) -> f64 {
    delta.num_days() as f64
}

#[expect(clippy::cast_precision_loss, reason = "a handful of weekly errors")]
fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "saturates, and try_days rejects what is out of range"
)]
fn add_days(date: NaiveDate, days: f64) -> Option<NaiveDate> {
    if !days.is_finite() {
        return None;
    }
    date.checked_add_signed(TimeDelta::try_days(days.round() as i64)?)
}
