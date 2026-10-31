//! The control loops: from the goal tree and its weekly series to each node's status and the
//! master's verdict. Errors are days behind schedule, so they add up across unlike metrics.

mod control;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use chrono::{NaiveDate, TimeDelta};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::{Goal, GoalKind, Observation, Series};

/// Days either side of the reference path that still count as on course.
const ON_COURSE_DAYS: f64 = 3.0;
/// A controlled goal whose series has no reading this many days is stale.
const STALE_AFTER_DAYS: i64 = 28;
/// Weekly readings the integral, the derivative and the recent rate look back over, now included.
const WINDOW_WEEKS: i64 = 4;
/// A milestone due within this many days is due soon.
const DUE_SOON_DAYS: i64 = 14;
/// A goal whose error grows faster than this, in days per week, is drifting.
const DRIFT_DAYS_PER_WEEK: f64 = 1.0;
/// The tree's error, in days behind, above which the week runs [`Mode::B`].
const MODE_B_DAYS_BEHIND: f64 = 7.0;
/// Weight of a parent's own loop error against its children's weights in its roll-up.
const OWN_LOOP_WEIGHT: f64 = 1.0;

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
    let mut walk = Walk::new(goals, readings, today);
    let master = goals.iter().position(|goal| goal.kind == GoalKind::Master);
    if let Some(master) = master {
        let goal = &goals[master];
        walk.evaluate(
            master,
            Window {
                start: goal.start,
                due: goal.due,
                start_value: None,
                blocked: false,
            },
        );
    }
    let nodes = walk.nodes;

    let days_behind = master
        .and_then(|master| nodes[master].days_behind)
        .unwrap_or(0.0);
    let needed_hours = goals
        .iter()
        .zip(&nodes)
        .filter(|(_, node)| {
            !matches!(
                node.state,
                NodeState::Done | NodeState::Merged | NodeState::Pending
            )
        })
        .filter_map(|(goal, _)| goal.weekly_hours)
        .sum();
    let capacity_hours = master.and_then(|master| goals[master].capacity_hours);
    let overcommitted = capacity_hours.is_some_and(|capacity| needed_hours > capacity);
    let mode = if overcommitted || days_behind > MODE_B_DAYS_BEHIND {
        Mode::B
    } else {
        Mode::A
    };
    let drifting = nodes
        .iter()
        .filter(|node| {
            node.terms
                .is_some_and(|terms| terms.d > DRIFT_DAYS_PER_WEEK)
        })
        .map(|node| node.goal_id)
        .collect();

    Course {
        nodes,
        master: MasterVerdict {
            days_behind,
            needed_hours,
            capacity_hours,
            overcommitted,
            mode,
            drifting,
        },
    }
}

/// The dates and value a goal's path is laid between once its stack and ancestors are known.
#[derive(Debug, Clone, Copy)]
struct Window {
    /// The goal's own start shifted by earlier late merges, else where its stack or parent starts.
    start: Option<NaiveDate>,
    /// The goal's due date shifted by earlier late merges.
    due: Option<NaiveDate>,
    /// The previous checkpoint's target, where this one starts unless it says otherwise.
    start_value: Option<f64>,
    /// An earlier checkpoint in the stack, or an ancestor, is still open.
    blocked: bool,
}

/// One pass down the goal tree from the master, rolling errors back up.
struct Walk<'a> {
    goals: &'a [Goal],
    readings: &'a [Readings],
    today: NaiveDate,
    children: HashMap<i64, Vec<usize>>,
    visited: Vec<bool>,
    nodes: Vec<NodeStatus>,
}

impl<'a> Walk<'a> {
    fn new(goals: &'a [Goal], readings: &'a [Readings], today: NaiveDate) -> Self {
        let mut children: HashMap<i64, Vec<usize>> = HashMap::new();
        for (index, goal) in goals.iter().enumerate() {
            if let Some(parent) = goal.parent_id {
                children.entry(parent).or_default().push(index);
            }
        }
        Self {
            goals,
            readings,
            today,
            children,
            visited: vec![false; goals.len()],
            nodes: goals.iter().map(|goal| blank_status(goal.id)).collect(),
        }
    }

    /// Fills in the goal's status and its subtree's; returns the day it merged, if it has.
    fn evaluate(&mut self, index: usize, window: Window) -> Option<NaiveDate> {
        self.visited[index] = true;
        let goal = &self.goals[index];
        let (mut status, merged_on) = match goal.kind {
            GoalKind::Master => (blank_status(goal.id), None),
            GoalKind::Controlled => {
                let readings = goal
                    .series_id
                    .and_then(|id| self.readings.iter().find(|r| r.series.id == id));
                control::evaluate(goal, window, readings, self.today)
            }
            GoalKind::Milestone => milestone(goal, window, self.today),
        };

        let children = self.unvisited_children(goal.id);
        let subtree = Window {
            blocked: window.blocked || status.state == NodeState::Pending,
            ..window
        };
        if goal.kind == GoalKind::Master {
            for &child in &children {
                let child_goal = &self.goals[child];
                self.evaluate(
                    child,
                    Window {
                        start: child_goal.start.or(subtree.start),
                        due: child_goal.due,
                        start_value: None,
                        blocked: subtree.blocked,
                    },
                );
            }
        } else {
            self.stack(index, subtree, &children);
        }

        if !matches!(
            status.state,
            NodeState::Pending | NodeState::Merged | NodeState::Done
        ) {
            let contributions: Vec<_> = children
                .iter()
                .filter_map(|&child| self.contribution(child))
                .collect();
            roll_up(&mut status, &contributions);
        }
        self.nodes[index] = status;
        merged_on
    }

    /// Children not reached yet, so a cycle or a duplicate id is walked once.
    fn unvisited_children(&self, id: i64) -> Vec<usize> {
        self.children
            .get(&id)
            .into_iter()
            .flatten()
            .copied()
            .filter(|&child| !self.visited[child])
            .collect()
    }

    /// Walks a parent's checkpoints in due order: merged ones rebase the rest, the lowest open
    /// one is active and the ones after it wait.
    fn stack(&mut self, parent: usize, window: Window, children: &[usize]) {
        let mut checkpoints = children.to_vec();
        checkpoints.sort_by_key(|&child| {
            let goal = &self.goals[child];
            (goal.due.is_none(), goal.due, goal.id)
        });

        let mut shift = TimeDelta::zero();
        let mut previous_end = None;
        let mut previous_target = None;
        let mut blocked = window.blocked;
        let mut active = None;
        let mut stack_end = None;
        for child in checkpoints {
            let goal = &self.goals[child];
            let due = goal.due.and_then(|due| due.checked_add_signed(shift));
            let child_window = Window {
                start: goal
                    .start
                    .and_then(|start| start.checked_add_signed(shift))
                    .or(previous_end)
                    .or(window.start),
                due,
                start_value: previous_target,
                blocked,
            };
            let target_value = goal.target_value;
            let merged_on = self.evaluate(child, child_window);

            let state = self.nodes[child].state;
            if !blocked && !matches!(state, NodeState::Merged | NodeState::Done) {
                blocked = true;
                if state != NodeState::Pending {
                    active = Some(child);
                }
            }
            let late_merge = merged_on.zip(due).filter(|(merged, due)| merged > due);
            if let Some((merged, due)) = late_merge {
                shift += merged - due;
            }
            previous_end = late_merge.map(|(merged, _)| merged).or(due);
            previous_target = target_value;
            stack_end = previous_end.or(stack_end);
        }

        let (Some(active), Some(parent_due), Some(stack_end)) = (active, window.due, stack_end)
        else {
            return;
        };
        let slack = (parent_due - stack_end).num_days();
        let parent_title = &self.goals[parent].title;
        let node = &mut self.nodes[active];
        node.slack_days = Some(slack);
        if slack < 0 {
            node.state = NodeState::Conflict;
            let overrun = format!("stack runs {} days past {parent_title}'s due date", -slack);
            node.conflict = Some(match node.conflict.take() {
                Some(reason) => format!("{reason}; {overrun}"),
                None => overrun,
            });
        }
    }

    /// The child's (weight, days behind) its parent averages, if it has a live error.
    fn contribution(&self, child: usize) -> Option<(f64, f64)> {
        let node = &self.nodes[child];
        let silent = matches!(
            node.state,
            NodeState::Pending | NodeState::Merged | NodeState::Done | NodeState::SensorStale
        );
        node.days_behind
            .filter(|_| !silent)
            .map(|days| (self.goals[child].weight, days))
    }
}

/// Folds the children's errors into the node's own; keeps a conflict or overdue state, else
/// judges the node on the combined error.
fn roll_up(status: &mut NodeStatus, children: &[(f64, f64)]) {
    if children.is_empty() {
        return;
    }
    let own = status.days_behind.map(|days| (OWN_LOOP_WEIGHT, days));
    let Some(days) = weighted_mean(own.iter().chain(children)) else {
        return;
    };
    status.days_behind = Some(days);
    if !matches!(status.state, NodeState::Conflict | NodeState::Overdue) {
        status.state = state_for(days);
    }
}

fn weighted_mean<'a>(errors: impl IntoIterator<Item = &'a (f64, f64)>) -> Option<f64> {
    let (weights, weighted) = errors
        .into_iter()
        .fold((0.0, 0.0), |(weights, weighted), (weight, days)| {
            (weights + weight, weighted + weight * days)
        });
    (weights > 0.0).then(|| weighted / weights)
}

fn state_for(days_behind: f64) -> NodeState {
    if days_behind > ON_COURSE_DAYS {
        NodeState::Behind
    } else if days_behind < -ON_COURSE_DAYS {
        NodeState::Ahead
    } else {
        NodeState::OnCourse
    }
}

fn milestone(goal: &Goal, window: Window, today: NaiveDate) -> (NodeStatus, Option<NaiveDate>) {
    let mut status = blank_status(goal.id);
    if window.blocked {
        status.state = NodeState::Pending;
        return (status, None);
    }
    status.state = match (goal.done_on, window.due) {
        (Some(_), _) => NodeState::Done,
        (None, Some(due)) if due < today => {
            status.days_behind = Some(control::days(today - due));
            NodeState::Overdue
        }
        (None, Some(due)) if (due - today).num_days() <= DUE_SOON_DAYS => NodeState::DueSoon,
        (None, _) => NodeState::Upcoming,
    };
    (status, goal.done_on)
}

fn blank_status(goal_id: i64) -> NodeStatus {
    NodeStatus {
        goal_id,
        state: NodeState::OnCourse,
        days_behind: None,
        terms: None,
        projected_arrival: None,
        needed_per_week: None,
        slack_days: None,
        conflict: None,
        path: Vec::new(),
    }
}
