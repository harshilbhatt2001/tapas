use chrono::NaiveDate;

use super::{Course, Mode, NodeState, NodeStatus, Readings, course};
use crate::{Aggregation, Feed, Goal, GoalKind, Observation, Series};

const MASTER: i64 = 1;
const SERIES: i64 = 100;
const OTHER_SERIES: i64 = 101;

fn date(text: &str) -> NaiveDate {
    text.parse().unwrap()
}

/// A Wednesday.
fn today() -> NaiveDate {
    date("2026-10-28")
}

/// A Monday, 100 days before [`DUE`].
const START: &str = "2026-08-03";
const DUE: &str = "2026-11-11";

fn goal(id: i64, parent_id: Option<i64>, kind: GoalKind) -> Goal {
    Goal {
        id,
        parent_id,
        title: format!("goal {id}"),
        kind,
        due: None,
        start: None,
        series_id: None,
        start_value: None,
        target_value: None,
        done_on: None,
        weight: 1.0,
        weekly_hours: None,
        capacity_hours: None,
        max_ramp: None,
    }
}

fn master() -> Goal {
    goal(MASTER, None, GoalKind::Master)
}

/// A controlled goal under the master on [`SERIES`], from `start_value` on [`START`] to
/// `target` on [`DUE`].
fn controlled(id: i64, start_value: f64, target: f64) -> Goal {
    Goal {
        start: Some(date(START)),
        due: Some(date(DUE)),
        series_id: Some(SERIES),
        start_value: Some(start_value),
        target_value: Some(target),
        ..goal(id, Some(MASTER), GoalKind::Controlled)
    }
}

fn checkpoint(id: i64, parent: i64, due: &str, target: f64) -> Goal {
    Goal {
        due: Some(date(due)),
        series_id: Some(SERIES),
        target_value: Some(target),
        ..goal(id, Some(parent), GoalKind::Controlled)
    }
}

fn milestone(id: i64, due: &str) -> Goal {
    Goal {
        due: Some(date(due)),
        ..goal(id, Some(MASTER), GoalKind::Milestone)
    }
}

fn readings(id: i64, aggregation: Aggregation, weeks: &[(&str, f64)]) -> Readings {
    Readings {
        series: Series {
            id,
            name: format!("series {id}"),
            unit: "h".into(),
            aggregation,
            feed: Feed::Manual,
        },
        weeks: weeks
            .iter()
            .map(|&(week, value)| Observation {
                week: date(week),
                value,
            })
            .collect(),
    }
}

/// The same weekly value for consecutive weeks from `first`.
fn sum_weeks(first: &str, values: &[f64]) -> Readings {
    let mut series = readings(SERIES, Aggregation::Sum, &[]);
    series.weeks = date(first)
        .iter_weeks()
        .zip(values)
        .map(|(week, &value)| Observation { week, value })
        .collect();
    series
}

fn node(course: &Course, id: i64) -> &NodeStatus {
    course.nodes.iter().find(|node| node.goal_id == id).unwrap()
}

fn assert_close(actual: Option<f64>, expected: f64) {
    let actual = actual.expect("a value");
    assert!(
        (actual - expected).abs() < 1e-9,
        "{actual} is not {expected}"
    );
}

/// The status of a lone level goal whose latest reading, in the week of 2026-10-19, is
/// `latest`. That week ends 84 days after [`START`].
fn level_goal(start_value: f64, target: f64, latest: f64) -> NodeStatus {
    let goals = [master(), controlled(2, start_value, target)];
    let series = readings(SERIES, Aggregation::Level, &[("2026-10-19", latest)]);
    node(&course(&goals, &[series], today()), 2).clone()
}

#[test]
fn rising_target_counts_days_behind_and_ahead() {
    let behind = level_goal(0.0, 100.0, 70.0);
    assert_close(behind.days_behind, 14.0);
    assert_eq!(behind.state, NodeState::Behind);

    let ahead = level_goal(0.0, 100.0, 90.0);
    assert_close(ahead.days_behind, -6.0);
    assert_eq!(ahead.state, NodeState::Ahead);
}

#[test]
fn falling_target_counts_days_behind_and_ahead() {
    let behind = level_goal(90.0, 80.0, 83.0);
    assert_close(behind.days_behind, 14.0);
    assert_eq!(behind.state, NodeState::Behind);

    let ahead = level_goal(90.0, 80.0, 80.6);
    assert_close(ahead.days_behind, -10.0);
    assert_eq!(ahead.state, NodeState::Ahead);
}

#[test]
fn sum_series_runs_a_total_from_the_start_week() {
    let goals = [master(), controlled(2, 0.0, 100.0)];
    let status = |weekly: f64| {
        let mut values = vec![weekly; 12];
        values.push(2.0);
        let mut series = sum_weeks(START, &values);
        series.weeks.insert(
            0,
            Observation {
                week: date("2026-07-27"),
                value: 50.0,
            },
        );
        node(&course(&goals, &[series], today()), 2).clone()
    };

    let on_course = status(7.0);
    assert_close(on_course.days_behind, 0.0);
    assert_eq!(on_course.state, NodeState::OnCourse);
    assert_close(on_course.needed_per_week, 7.0);

    let behind = status(5.0);
    assert_close(behind.days_behind, 24.0);
    assert_eq!(behind.state, NodeState::Behind);
}

#[test]
fn stale_sensor_has_no_error_and_rolls_none_up() {
    let goals = [master(), controlled(2, 0.0, 100.0)];
    let series = readings(SERIES, Aggregation::Level, &[("2026-09-21", 50.0)]);
    let course = course(&goals, &[series], today());

    let stale = node(&course, 2);
    assert_eq!(stale.state, NodeState::SensorStale);
    assert_eq!(stale.days_behind, None);
    assert_eq!(stale.terms, None);
    assert_eq!(node(&course, MASTER).days_behind, None);
    assert!(course.master.days_behind.abs() < f64::EPSILON);
}

#[test]
fn ramp_limit_conflict_clamps_the_catch_up_and_stops_the_integral() {
    let goals = [
        master(),
        Goal {
            max_ramp: Some(0.1),
            ..controlled(2, 0.0, 200.0)
        },
    ];
    let course = course(&goals, &[sum_weeks(START, &[7.0; 12])], today());

    let status = node(&course, 2);
    assert_eq!(status.state, NodeState::Conflict);
    assert_close(status.needed_per_week, 7.7);
    let reason = status.conflict.as_deref().unwrap();
    assert!(reason.contains("needs 58.0 h/wk"), "{reason}");
    assert!(reason.contains("allows 7.7 h/wk"), "{reason}");
    let terms = status.terms.unwrap();
    assert_close(Some(terms.p), 42.0);
    assert_close(Some(terms.i), 0.0);
}

/// A parent due 2027-01-31 whose three checkpoints, the middle one due 2026-11-30, are listed
/// out of due order.
fn stack(first_due: &str, last_due: &str) -> Vec<Goal> {
    let parent = Goal {
        start: Some(date(START)),
        due: Some(date("2027-01-31")),
        ..goal(10, Some(MASTER), GoalKind::Controlled)
    };
    let first = Goal {
        start_value: Some(0.0),
        ..checkpoint(11, 10, first_due, 50.0)
    };
    vec![
        master(),
        parent,
        checkpoint(13, 10, last_due, 100.0),
        first,
        checkpoint(12, 10, "2026-11-30", 80.0),
    ]
}

#[test]
fn merge_hands_over_to_the_next_checkpoint() {
    let series = readings(
        SERIES,
        Aggregation::Level,
        &[
            ("2026-08-03", 10.0),
            ("2026-08-24", 52.0),
            ("2026-10-19", 65.0),
        ],
    );
    let course = course(&stack("2026-09-01", "2026-12-31"), &[series], today());

    assert_eq!(node(&course, 11).state, NodeState::Merged);
    let active = node(&course, 12);
    // From 50 on 2026-09-01 to 80 on 2026-11-30: 65 is where the path was on day 45 of 55.
    assert_close(active.days_behind, 10.0);
    assert_eq!(active.state, NodeState::Behind);
    assert_eq!(active.slack_days, Some(31));
    let pending = node(&course, 13);
    assert_eq!(pending.state, NodeState::Pending);
    assert_eq!(pending.days_behind, None);
    assert!(!pending.path.is_empty());
    assert_close(node(&course, 10).days_behind, 10.0);
}

#[test]
fn late_merge_rebases_the_stack_into_negative_slack() {
    let series = readings(
        SERIES,
        Aggregation::Level,
        &[
            ("2026-08-03", 10.0),
            ("2026-08-31", 52.0),
            ("2026-10-19", 70.0),
        ],
    );
    let course = course(&stack("2026-08-17", "2027-01-20"), &[series], today());

    let active = node(&course, 12);
    assert_eq!(active.slack_days, Some(-3));
    assert_eq!(active.state, NodeState::Conflict);
    let reason = active.conflict.as_deref().unwrap();
    assert!(
        reason.contains("3 days past goal 10's due date"),
        "{reason}"
    );
    assert_eq!(active.path.first().unwrap().week, date("2026-08-31"));
    assert_eq!(active.path.last().unwrap().week, date("2026-12-14"));
    assert_eq!(
        node(&course, 13).path.last().unwrap().week,
        date("2027-02-01")
    );
}

#[test]
fn milestone_states_follow_the_due_date() {
    let goals = [
        master(),
        milestone(2, "2026-12-31"),
        milestone(3, "2026-11-11"),
        milestone(4, "2026-10-20"),
        Goal {
            done_on: Some(date("2026-10-01")),
            ..milestone(5, "2026-10-01")
        },
    ];
    let course = course(&goals, &[], today());

    assert_eq!(node(&course, 2).state, NodeState::Upcoming);
    assert_eq!(node(&course, 2).days_behind, None);
    assert_eq!(node(&course, 3).state, NodeState::DueSoon);
    assert_eq!(node(&course, 4).state, NodeState::Overdue);
    assert_close(node(&course, 4).days_behind, 8.0);
    assert_eq!(node(&course, 5).state, NodeState::Done);
    assert_close(Some(course.master.days_behind), 8.0);
}

#[test]
fn roll_up_skips_pending_and_stale_children() {
    let goals = [
        master(),
        Goal {
            weight: 2.0,
            ..controlled(2, 0.0, 100.0)
        },
        Goal {
            weight: 5.0,
            series_id: Some(OTHER_SERIES),
            ..controlled(3, 0.0, 100.0)
        },
        Goal {
            weight: 1.0,
            ..milestone(4, "2026-10-20")
        },
        Goal {
            weight: 3.0,
            start: Some(date("2026-11-02")),
            ..controlled(5, 0.0, 100.0)
        },
    ];
    let series = [
        readings(SERIES, Aggregation::Level, &[("2026-10-19", 70.0)]),
        readings(OTHER_SERIES, Aggregation::Level, &[("2026-09-14", 0.0)]),
    ];
    let course = course(&goals, &series, today());

    assert_eq!(node(&course, 3).state, NodeState::SensorStale);
    assert_eq!(node(&course, 5).state, NodeState::Pending);
    assert_close(node(&course, MASTER).days_behind, 12.0);
    assert_eq!(node(&course, MASTER).state, NodeState::Behind);
    assert_close(Some(course.master.days_behind), 12.0);
    assert_eq!(course.master.mode, Mode::B);
}

#[test]
fn master_flags_overcommitment_and_drift() {
    let goals = |capacity: f64| {
        [
            Goal {
                capacity_hours: Some(capacity),
                ..master()
            },
            Goal {
                weekly_hours: Some(6.0),
                ..controlled(2, 0.0, 100.0)
            },
            Goal {
                weekly_hours: Some(6.0),
                series_id: Some(OTHER_SERIES),
                ..controlled(3, 0.0, 100.0)
            },
            Goal {
                weekly_hours: Some(5.0),
                done_on: Some(date("2026-10-01")),
                ..milestone(4, "2026-10-01")
            },
        ]
    };
    // Goal 2 adds 3 a week against a path of 7 a week: 4 more days behind every week.
    let series = [
        readings(
            SERIES,
            Aggregation::Level,
            &[
                ("2026-09-28", 61.0),
                ("2026-10-05", 64.0),
                ("2026-10-12", 67.0),
                ("2026-10-19", 70.0),
            ],
        ),
        readings(
            OTHER_SERIES,
            Aggregation::Level,
            &[("2026-10-12", 77.0), ("2026-10-19", 84.0)],
        ),
    ];

    let overcommitted = course(&goals(10.0), &series, today());
    let terms = node(&overcommitted, 2).terms.unwrap();
    assert_close(Some(terms.d), 4.0);
    assert_close(Some(terms.i), 8.0);
    assert_close(Some(overcommitted.master.needed_hours), 12.0);
    assert!(overcommitted.master.overcommitted);
    assert_eq!(overcommitted.master.mode, Mode::B);
    assert_eq!(overcommitted.master.drifting, vec![2]);

    let within = course(&goals(15.0), &series, today());
    assert!(!within.master.overcommitted);
    assert_close(Some(within.master.days_behind), 7.0);
    assert_eq!(within.master.mode, Mode::A);
}

#[test]
fn invalid_trees_yield_a_status_per_goal_without_error() {
    let goals = [
        goal(2, Some(3), GoalKind::Milestone),
        goal(3, Some(2), GoalKind::Milestone),
        Goal {
            series_id: Some(999),
            ..controlled(4, 0.0, 100.0)
        },
    ];
    let course = course(&goals, &[], today());

    assert_eq!(course.nodes.len(), goals.len());
    assert!(course.nodes.iter().all(|node| node.days_behind.is_none()));
    assert_eq!(course.master.mode, Mode::A);
}
