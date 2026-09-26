//! Planned items of a dated week against the Google Health workouts recorded in it.

use chrono::{NaiveDate, NaiveTime, TimeDelta};

use crate::{
    calc,
    google::health::Workout,
    model::{Category, Item, Kind, Library, Plan, SessionType},
    services::{is_commute, workout_kind},
};

/// `Item.workout` value of an item marked missed by hand.
pub const MISSED_ON_PURPOSE: &str = "none";

/// Longest gap between a brick's ride ending and its run starting.
pub const BRICK_TRANSITION: TimeDelta = TimeDelta::minutes(30);

/// One block of a day on the board.
#[derive(Debug, Clone, PartialEq)]
pub enum Slot<'a> {
    /// Done as one workout, or as a ride and a run for a brick.
    Done {
        item: &'a Item,
        workouts: Vec<&'a Workout>,
    },
    Missed {
        item: &'a Item,
    },
    /// A workout no item was matched to.
    Extra {
        workout: &'a Workout,
    },
    Commute {
        workout: &'a Workout,
    },
    /// Not done yet, or not training.
    Planned {
        item: &'a Item,
    },
}

impl Slot<'_> {
    /// When the block starts: the workout's start once there is one, else the planned start.
    #[must_use]
    pub fn start(&self) -> NaiveTime {
        match self {
            Slot::Done { item, workouts } => {
                workouts.first().map_or(item.start, |w| w.start.time())
            }
            Slot::Extra { workout } | Slot::Commute { workout } => workout.start.time(),
            Slot::Missed { item } | Slot::Planned { item } => item.start,
        }
    }
}

/// A possible match of one item to its workouts, ranked by `gap` then `transition`.
struct Candidate {
    gap: TimeDelta,
    transition: TimeDelta,
    item: usize,
    workouts: Vec<usize>,
}

fn start_gap(item: &Item, w: &Workout) -> TimeDelta {
    (w.start.time() - item.start).abs()
}

/// Every way `item` could be matched to the day's `free` workouts.
fn candidates(
    kind: Kind,
    item_index: usize,
    item: &Item,
    free: &[usize],
    workouts: &[Workout],
) -> Vec<Candidate> {
    let of_kind = |k: Kind| {
        free.iter()
            .copied()
            .filter(move |&i| workout_kind(&workouts[i]) == Some(k))
    };
    if kind != Kind::Brick {
        return of_kind(kind)
            .map(|i| Candidate {
                gap: start_gap(item, &workouts[i]),
                transition: TimeDelta::zero(),
                item: item_index,
                workouts: vec![i],
            })
            .collect();
    }
    of_kind(Kind::Bike)
        .flat_map(|bike| {
            of_kind(Kind::Run).filter_map(move |run| {
                let transition = workouts[run].start - workouts[bike].end;
                (TimeDelta::zero()..=BRICK_TRANSITION)
                    .contains(&transition)
                    .then(|| Candidate {
                        gap: start_gap(item, &workouts[bike]),
                        transition,
                        item: item_index,
                        workouts: vec![bike, run],
                    })
            })
        })
        .collect()
}

/// Match a dated week's items to its workouts, day by day, as of `today`. `weight_kg` tells
/// commute rides apart (see [`is_commute`]).
#[must_use]
pub fn reconcile<'a>(
    lib: &Library,
    week: &'a Plan,
    monday: NaiveDate,
    workouts: &'a [Workout],
    weight_kg: f64,
    today: NaiveDate,
) -> [Vec<Slot<'a>>; 7] {
    let day_of: Vec<Option<usize>> = workouts
        .iter()
        .map(|w| {
            usize::try_from((w.day - monday).num_days())
                .ok()
                .filter(|&d| d < 7)
        })
        .collect();
    let commute: Vec<bool> = workouts.iter().map(|w| is_commute(w, weight_kg)).collect();
    let mut consumed = vec![false; workouts.len()];
    let mut days: [Vec<Slot>; 7] = Default::default();
    let mut unlinked: [Vec<&Item>; 7] = Default::default();

    for ((d, items), date) in week.days.iter().enumerate().zip(monday.iter_days()) {
        for item in items {
            if date > today {
                days[d].push(Slot::Planned { item });
                continue;
            }
            let linked = match item.workout.as_deref() {
                Some(MISSED_ON_PURPOSE) => {
                    days[d].push(Slot::Missed { item });
                    continue;
                }
                Some(id) => (0..workouts.len())
                    .find(|&i| workouts[i].id == id && day_of[i].is_some() && !consumed[i]),
                None => None,
            };
            if let Some(i) = linked {
                consumed[i] = true;
                days[d].push(Slot::Done {
                    item,
                    workouts: vec![&workouts[i]],
                });
            } else {
                unlinked[d].push(item);
            }
        }
    }

    for ((d, items), date) in unlinked.iter().enumerate().zip(monday.iter_days()) {
        if date > today {
            continue;
        }
        let free: Vec<usize> = (0..workouts.len())
            .filter(|&i| day_of[i] == Some(d) && !consumed[i] && !commute[i])
            .collect();
        let mut pairs: Vec<Candidate> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| calc::is_train(lib, item))
            .filter_map(|(k, item)| Some((k, item, lib.get(&item.type_key)?.kind)))
            .flat_map(|(k, item, kind)| candidates(kind, k, item, &free, workouts))
            .collect();
        pairs.sort_by_key(|c| (c.gap, c.transition));
        let mut matched: Vec<Option<Vec<usize>>> = vec![None; items.len()];
        for pair in pairs {
            if matched[pair.item].is_none() && pair.workouts.iter().all(|&i| !consumed[i]) {
                for &i in &pair.workouts {
                    consumed[i] = true;
                }
                matched[pair.item] = Some(pair.workouts);
            }
        }
        for (&item, found) in items.iter().zip(matched) {
            days[d].push(match found {
                Some(found) => Slot::Done {
                    item,
                    workouts: found.into_iter().map(|i| &workouts[i]).collect(),
                },
                None if date < today && calc::is_train(lib, item) => Slot::Missed { item },
                None => Slot::Planned { item },
            });
        }
    }

    for (i, workout) in workouts.iter().enumerate() {
        if let (Some(d), false) = (day_of[i], consumed[i]) {
            days[d].push(if commute[i] {
                Slot::Commute { workout }
            } else {
                Slot::Extra { workout }
            });
        }
    }
    for day in &mut days {
        day.sort_by_key(Slot::start);
    }
    days
}

/// Planned and done minutes of one counted training type over a week.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeTotals {
    pub type_key: String,
    pub planned_min: u32,
    pub done_min: u32,
}

/// The first counted training type of the workout's kind: where an [`Slot::Extra`] counts.
fn counted_type_of_workout<'a>(lib: &'a Library, w: &Workout) -> Option<&'a SessionType> {
    let kind = workout_kind(w)?;
    lib.types
        .iter()
        .find(|t| t.kind == kind && t.category == Category::Train && t.counted)
}

/// Planned against done minutes per counted training type, in library order. Done minutes are
/// the matched workouts' minutes plus the extra workouts of the type's kind.
#[must_use]
pub fn totals(lib: &Library, week: &Plan, days: &[Vec<Slot>; 7]) -> Vec<TypeTotals> {
    lib.types
        .iter()
        .filter(|t| t.category == Category::Train && t.counted)
        .map(|t| TypeTotals {
            type_key: t.key.clone(),
            planned_min: week
                .days
                .iter()
                .flatten()
                .filter(|x| x.type_key == t.key)
                .map(|x| x.dur)
                .sum(),
            done_min: days
                .iter()
                .flatten()
                .map(|slot| match slot {
                    Slot::Done { item, workouts } if item.type_key == t.key => {
                        workouts.iter().map(|w| w.minutes).sum()
                    }
                    Slot::Extra { workout }
                        if counted_type_of_workout(lib, workout)
                            .is_some_and(|c| c.key == t.key) =>
                    {
                        workout.minutes
                    }
                    _ => 0,
                })
                .sum(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::{Days, Local, TimeZone};

    use super::*;
    use crate::library::{default_library, new_item};

    fn monday() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 21).unwrap()
    }

    fn after_week() -> NaiveDate {
        monday() + Days::new(7)
    }

    fn hhmm(s: &str) -> NaiveTime {
        NaiveTime::parse_from_str(s, "%H:%M").unwrap()
    }

    fn item(lib: &Library, ty: &str, start: &str, dur: u32) -> Item {
        Item {
            start: hhmm(start),
            dur,
            ..new_item(lib, ty).unwrap()
        }
    }

    fn workout(exercise_type: &str, day: u64, start: &str, minutes: u32) -> Workout {
        let date = monday() + Days::new(day);
        let start = Local
            .from_local_datetime(&date.and_time(hhmm(start)))
            .unwrap();
        Workout {
            id: format!("{exercise_type}-{date}-{}", start.time()),
            exercise_type: exercise_type.into(),
            day: date,
            start,
            end: start + TimeDelta::minutes(minutes.into()),
            minutes,
            active_min: None,
            kcal: None,
            avg_hr: None,
            distance_m: None,
            elevation_m: None,
            pace_s_per_km: None,
            swim_lengths: None,
            azm: None,
            google_zones: None,
            notes: None,
            display_name: None,
        }
    }

    fn week(days: [Vec<Item>; 7]) -> Plan {
        Plan {
            days,
            ..Plan::new("Week")
        }
    }

    const KG: f64 = 71.0;

    #[test]
    fn matches_the_same_kind_only() {
        let lib = default_library();
        let week = week([
            vec![
                item(&lib, "run", "07:00", 40),
                item(&lib, "swim", "18:00", 45),
            ],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        ]);
        let ws = [
            workout("RUNNING", 0, "07:10", 42),
            workout("BIKING", 0, "18:30", 60),
            workout("WALKING", 0, "20:00", 20),
            workout("RUNNING", 7, "07:00", 40),
        ];
        let days = reconcile(&lib, &week, monday(), &ws, KG, after_week());
        let [run, swim] = [&week.days[0][0], &week.days[0][1]];
        assert_eq!(
            days[0],
            [
                Slot::Done {
                    item: run,
                    workouts: vec![&ws[0]]
                },
                Slot::Missed { item: swim },
                Slot::Extra { workout: &ws[1] },
                Slot::Extra { workout: &ws[2] },
            ]
        );
        assert!(days[1..].iter().all(Vec::is_empty));
    }

    #[test]
    fn nearest_start_wins_across_the_day() {
        let lib = default_library();
        let week = week([
            vec![
                item(&lib, "run", "07:00", 40),
                item(&lib, "run", "12:00", 40),
            ],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        ]);
        let ws = [
            workout("RUNNING", 0, "11:00", 40),
            workout("RUNNING", 0, "13:30", 40),
        ];
        let days = reconcile(&lib, &week, monday(), &ws, KG, after_week());
        assert_eq!(
            days[0],
            [
                Slot::Done {
                    item: &week.days[0][1],
                    workouts: vec![&ws[0]]
                },
                Slot::Done {
                    item: &week.days[0][0],
                    workouts: vec![&ws[1]]
                },
            ]
        );
    }

    #[test]
    fn a_session_moved_within_the_day_still_matches() {
        let lib = default_library();
        let week = week([
            vec![item(&lib, "run", "07:00", 40)],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        ]);
        let ws = [workout("RUNNING", 0, "19:30", 40)];
        let days = reconcile(&lib, &week, monday(), &ws, KG, after_week());
        assert_eq!(
            days[0],
            [Slot::Done {
                item: &week.days[0][0],
                workouts: vec![&ws[0]]
            }]
        );
    }

    #[test]
    fn brick_is_a_ride_then_a_run_within_the_transition() {
        let lib = default_library();
        let week = week([
            vec![item(&lib, "brick", "09:00", 120)],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        ]);
        let brick = &week.days[0][0];

        let ws = [
            workout("BIKING", 0, "09:05", 90),
            workout("RUNNING", 0, "10:50", 30),
        ];
        let days = reconcile(&lib, &week, monday(), &ws, KG, after_week());
        assert_eq!(
            days[0],
            [Slot::Done {
                item: brick,
                workouts: vec![&ws[0], &ws[1]]
            }]
        );

        let ws = [
            workout("BIKING", 0, "09:05", 90),
            workout("RUNNING", 0, "11:20", 30),
        ];
        let days = reconcile(&lib, &week, monday(), &ws, KG, after_week());
        assert_eq!(
            days[0],
            [
                Slot::Missed { item: brick },
                Slot::Extra { workout: &ws[0] },
                Slot::Extra { workout: &ws[1] },
            ]
        );
    }

    #[test]
    fn manual_link_overrides_auto_matching() {
        let lib = default_library();
        let ws = [workout("RUNNING", 1, "07:00", 40)];
        let week = week([
            vec![Item {
                workout: Some(ws[0].id.clone()),
                ..item(&lib, "run", "07:00", 40)
            }],
            vec![item(&lib, "run", "07:00", 40)],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        ]);
        let days = reconcile(&lib, &week, monday(), &ws, KG, after_week());
        assert_eq!(
            days[0],
            [Slot::Done {
                item: &week.days[0][0],
                workouts: vec![&ws[0]]
            }]
        );
        assert_eq!(
            days[1],
            [Slot::Missed {
                item: &week.days[1][0]
            }]
        );
    }

    #[test]
    fn missed_on_purpose_ignores_a_matching_workout() {
        let lib = default_library();
        let week = week([
            vec![Item {
                workout: Some(MISSED_ON_PURPOSE.into()),
                ..item(&lib, "run", "07:00", 40)
            }],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        ]);
        let ws = [workout("RUNNING", 0, "07:00", 40)];
        let days = reconcile(&lib, &week, monday(), &ws, KG, monday());
        assert_eq!(
            days[0],
            [
                Slot::Missed {
                    item: &week.days[0][0]
                },
                Slot::Extra { workout: &ws[0] },
            ]
        );
    }

    #[test]
    fn commutes_are_never_matched() {
        let lib = default_library();
        let week = week([
            vec![item(&lib, "bike", "08:00", 60)],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        ]);
        let ws = [
            workout("BIKING", 0, "08:00", 15),
            workout("BIKING", 0, "18:00", 60),
        ];
        let days = reconcile(&lib, &week, monday(), &ws, KG, after_week());
        assert_eq!(
            days[0],
            [
                Slot::Commute { workout: &ws[0] },
                Slot::Done {
                    item: &week.days[0][0],
                    workouts: vec![&ws[1]]
                },
            ]
        );
    }

    #[test]
    fn today_and_later_stay_planned_until_the_day_ends() {
        let lib = default_library();
        let week = week([
            vec![
                item(&lib, "run", "07:00", 40),
                item(&lib, "work", "09:00", 480),
            ],
            vec![item(&lib, "run", "07:00", 40)],
            vec![item(&lib, "run", "07:00", 40)],
            vec![],
            vec![],
            vec![],
            vec![],
        ]);
        let tuesday = monday() + Days::new(1);
        let days = reconcile(&lib, &week, monday(), &[], KG, tuesday);
        assert_eq!(
            days[0],
            [
                Slot::Missed {
                    item: &week.days[0][0]
                },
                Slot::Planned {
                    item: &week.days[0][1]
                },
            ]
        );
        assert_eq!(
            days[1],
            [Slot::Planned {
                item: &week.days[1][0]
            }]
        );
        assert_eq!(
            days[2],
            [Slot::Planned {
                item: &week.days[2][0]
            }]
        );
    }

    #[test]
    fn totals_count_matched_and_extra_minutes() {
        let lib = default_library();
        let week = week([
            vec![
                item(&lib, "run", "07:00", 40),
                item(&lib, "bike", "18:00", 60),
            ],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        ]);
        let ws = [
            workout("RUNNING", 0, "07:05", 45),
            workout("RUNNING", 1, "07:00", 30),
            workout("BIKING", 1, "08:00", 15),
            workout("PADEL", 2, "11:30", 90),
        ];
        let days = reconcile(&lib, &week, monday(), &ws, KG, after_week());
        let totals = totals(&lib, &week, &days);
        let of = |key: &str| {
            let t = totals.iter().find(|t| t.type_key == key).unwrap();
            (t.planned_min, t.done_min)
        };
        assert_eq!(of("run"), (40, 75));
        assert_eq!(of("bike"), (60, 0));
        assert_eq!(of("swim"), (0, 0));
        assert!(totals.iter().all(|t| t.type_key != "padel"));
    }
}
