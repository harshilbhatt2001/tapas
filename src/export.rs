//! Calendar export of dated weeks, one event per item or workout: iCalendar and the artifact's
//! Google Calendar CSV. Google Calendar pushes the same events.

use std::fmt;

use anyhow::Result;
use chrono::{Days, NaiveDate, NaiveDateTime, TimeDelta, Weekday};
use csv::{QuoteStyle, Terminator, WriterBuilder};
use icalendar::{Calendar, Component, Event, EventLike};

use crate::{
    actuals::{self, Slot},
    calc::guide,
    google::health::Workout,
    health_cache::WeekActuals,
    model::{Category, Item, Kind, Store, Week, mondays},
    zones,
};

/// What an event stands for, stable across exports and pushes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum EventKey {
    /// A planned item, by its id.
    Item(String),
    /// A workout no item was matched to, by its Google id.
    Workout(String),
}

impl fmt::Display for EventKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EventKey::Item(id) => write!(f, "item-{id}"),
            EventKey::Workout(id) => write!(f, "workout-{id}"),
        }
    }
}

/// One event of the dated week from `week`. Timed events have `start` and `end` (floating local
/// time); all-day markers (duration 0) have neither.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportEvent {
    pub week: NaiveDate,
    pub key: EventKey,
    pub summary: String,
    pub description: String,
    pub start: Option<NaiveDateTime>,
    pub end: Option<NaiveDateTime>,
    pub date: NaiveDate,
}

/// The Monday after `today` (a week ahead when `today` is a Monday).
#[must_use]
pub fn next_monday(today: NaiveDate) -> NaiveDate {
    today.week(Weekday::Mon).first_day() + Days::new(7)
}

/// The item as planned on `date`. Life items are left out unless `include_life`, and so are
/// all-day recovery markers other than "Rest day".
fn planned_event(
    store: &Store,
    week: NaiveDate,
    date: NaiveDate,
    it: &Item,
    include_life: bool,
) -> Option<ExportEvent> {
    let lib = &store.library;
    let (t, e) = lib.resolve(it)?;
    if t.category == Category::Life && !include_life {
        return None;
    }
    if t.kind == Kind::Rest && it.dur == 0 && it.effort != "off" {
        return None;
    }
    let g = guide(lib, store.profile.weight, it);
    let description = [
        Some(format!("Effort: {}", e.label)),
        Some(it.notes.clone()),
        g.before.map(|x| format!("Before: {x}")),
        g.during.map(|x| format!("During: {x}")),
        g.after.map(|x| format!("After: {x}")),
    ]
    .into_iter()
    .flatten()
    .filter(|x| !x.is_empty())
    .collect::<Vec<_>>()
    .join("\n");
    let (start, end) = if it.dur == 0 {
        (None, None)
    } else {
        let s = date.and_time(it.start);
        (Some(s), Some(s + TimeDelta::minutes(it.dur.into())))
    };
    Some(ExportEvent {
        week,
        key: EventKey::Item(it.id.clone()),
        summary: format!("{}: {}", t.label, e.label),
        description,
        start,
        end,
        date,
    })
}

/// Events of the dated week from `monday` as of `today`. Without `actuals` every item is as
/// planned. With them, past items become what was done: a done item moves to its workouts'
/// times with their metrics, a missed one keeps its time under a ✗, and unplanned workouts get
/// their own events; commute rides get none.
#[must_use]
pub fn week_events(
    store: &Store,
    monday: NaiveDate,
    actuals: Option<&WeekActuals>,
    today: NaiveDate,
    include_life: bool,
) -> Vec<ExportEvent> {
    let unstored;
    let plan = if let Some(week) = store.week(monday) {
        &week.plan
    } else {
        unstored = Week::new(monday).plan;
        &unstored
    };
    let planned = |d: usize, it: &Item| {
        planned_event(
            store,
            monday,
            monday + Days::new(d as u64),
            it,
            include_life,
        )
    };
    let Some(actuals) = actuals else {
        return plan
            .days
            .iter()
            .enumerate()
            .flat_map(|(d, items)| items.iter().filter_map(move |it| planned(d, it)))
            .collect();
    };
    let (lib, profile) = (&store.library, &store.profile);
    let metrics = |workouts: &[&Workout]| -> Vec<String> {
        workouts
            .iter()
            .flat_map(|w| {
                let effort =
                    zones::workout_effort(w, &actuals.heart_rate, &profile.hr, profile.weight);
                actuals::metrics_lines(w, &effort)
            })
            .collect()
    };
    let days = actuals::reconcile(lib, plan, monday, &actuals.workouts, profile.weight, today);
    let mut out = Vec::new();
    for (d, slots) in days.iter().enumerate() {
        for slot in slots {
            out.extend(match slot {
                Slot::Planned { item } => planned(d, item),
                Slot::Missed { item } => planned(d, item).map(|ev| ExportEvent {
                    summary: actuals::missed_title(lib, item),
                    ..ev
                }),
                Slot::Done { item, workouts } => planned(d, item).map(|ev| {
                    let mut lines = metrics(workouts);
                    if !item.notes.is_empty() {
                        lines.push(item.notes.clone());
                    }
                    ExportEvent {
                        summary: actuals::done_title(lib, item, workouts),
                        description: lines.join("\n"),
                        start: workouts.first().map(|w| w.start.naive_local()),
                        end: workouts.last().map(|w| w.end.naive_local()),
                        ..ev
                    }
                }),
                Slot::Extra { workout } => Some(ExportEvent {
                    week: monday,
                    key: EventKey::Workout(workout.id.clone()),
                    summary: actuals::extra_title(workout),
                    description: metrics(&[workout]).join("\n"),
                    start: Some(workout.start.naive_local()),
                    end: Some(workout.end.naive_local()),
                    date: workout.day,
                }),
                Slot::Commute { .. } => None,
            });
        }
    }
    out
}

/// Whether the actuals going from `before` to `after` change the events of the week of
/// `monday` dated before `today`.
#[must_use]
pub fn past_events_changed(
    store: &Store,
    monday: NaiveDate,
    before: Option<&WeekActuals>,
    after: &WeekActuals,
    today: NaiveDate,
    include_life: bool,
) -> bool {
    let past = |actuals| {
        let mut events = week_events(store, monday, actuals, today, include_life);
        events.retain(|e| e.date < today);
        events
    };
    past(before) != past(Some(after))
}

/// [`week_events`] of `weeks` consecutive weeks from the week containing `from`, each with the
/// actuals `actuals_of` gives for its Monday.
#[must_use]
pub fn range_events(
    store: &Store,
    from: NaiveDate,
    weeks: u32,
    actuals_of: impl Fn(NaiveDate) -> Option<WeekActuals>,
    today: NaiveDate,
    include_life: bool,
) -> Vec<ExportEvent> {
    mondays(from, weeks)
        .flat_map(|monday| {
            week_events(
                store,
                monday,
                actuals_of(monday).as_ref(),
                today,
                include_life,
            )
        })
        .collect()
}

/// An iCalendar named `name` with one event each.
#[must_use]
pub fn to_ics(name: &str, events: &[ExportEvent]) -> String {
    let mut cal = Calendar::new();
    cal.name(name);
    for ev in events {
        let mut e = Event::new();
        e.uid(&format!("{}-{}@tapas", ev.week, ev.key))
            .summary(&ev.summary)
            .description(&ev.description);
        match (ev.start, ev.end) {
            (Some(s), Some(end)) => e.starts(s).ends(end),
            // DTEND of an all-day event is exclusive.
            _ => e.starts(ev.date).ends(ev.date + Days::new(1)),
        };
        cal.push(e.done());
    }
    cal.done().to_string()
}

const CSV_HEADER: [&str; 9] = [
    "Subject",
    "Start Date",
    "Start Time",
    "End Date",
    "End Time",
    "All Day Event",
    "Description",
    "Location",
    "Private",
];

/// Google Calendar import CSV, one row per event, every field quoted, CRLF lines.
pub fn to_google_csv(events: &[ExportEvent]) -> Result<String> {
    const DATE: &str = "%m/%d/%Y";
    const TIME: &str = "%-I:%M %p";
    let mut w = WriterBuilder::new()
        .quote_style(QuoteStyle::Always)
        .terminator(Terminator::CRLF)
        .from_writer(Vec::new());
    w.write_record(CSV_HEADER)?;
    for ev in events {
        let row = if let (Some(s), Some(e)) = (ev.start, ev.end) {
            [
                ev.summary.clone(),
                s.format(DATE).to_string(),
                s.format(TIME).to_string(),
                e.format(DATE).to_string(),
                e.format(TIME).to_string(),
                "False".into(),
                ev.description.clone(),
                String::new(),
                "True".into(),
            ]
        } else {
            let d = ev.date.format(DATE).to_string();
            [
                ev.summary.clone(),
                d.clone(),
                String::new(),
                d,
                String::new(),
                "True".into(),
                ev.description.clone(),
                String::new(),
                "True".into(),
            ]
        };
        w.write_record(&row)?;
    }
    Ok(String::from_utf8(w.into_inner()?)?)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use chrono::{DateTime, Local, NaiveTime, TimeZone};

    use super::*;
    use crate::library::{new_item, starter_plan};

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, d).unwrap()
    }

    const MONDAY: u32 = 21;

    fn at(date: NaiveDate, h: u32, m: u32) -> NaiveDateTime {
        date.and_hms_opt(h, m, 0).unwrap()
    }

    /// The starter plan applied to the weeks of 21 and 28 Sep, with a late run on the first
    /// Sunday that runs past midnight.
    fn store() -> Store {
        let mut store = Store {
            plans: vec![starter_plan("Base")],
            ..Store::default()
        };
        let id = store.plans[0].id.clone();
        assert!(store.apply_template(&id, day(MONDAY), 2));
        let late = Item {
            start: NaiveTime::from_hms_opt(23, 30, 0).unwrap(),
            dur: 60,
            ..new_item(&store.library, "run").unwrap()
        };
        store.weeks[0].plan.days[6].push(late);
        store
    }

    fn workout(exercise_type: &str, start: NaiveDateTime, minutes: u32) -> Workout {
        let start: DateTime<Local> = Local.from_local_datetime(&start).unwrap();
        Workout {
            id: format!("{exercise_type}-{start}"),
            exercise_type: exercise_type.into(),
            day: start.date_naive(),
            start,
            end: start + TimeDelta::minutes(minutes.into()),
            minutes,
            active_min: None,
            kcal: Some(300.0),
            avg_hr: Some(142),
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

    fn actuals(workouts: Vec<Workout>) -> WeekActuals {
        WeekActuals {
            fetched_at: DateTime::UNIX_EPOCH,
            workouts,
            heart_rate: Vec::new(),
        }
    }

    #[test]
    fn next_monday_is_strictly_after() {
        assert_eq!(next_monday(day(25)), day(28));
        assert_eq!(next_monday(day(27)), day(28));
        assert_eq!(next_monday(day(28)), day(28) + Days::new(7));
    }

    #[test]
    fn planned_week_skip_rules() {
        let store = store();
        let ev = week_events(&store, day(MONDAY), None, day(1), false);
        // 22 starter items minus 9 life items, plus the late run.
        assert_eq!(ev.len(), 14);
        assert!(ev.iter().all(|e| !e.summary.starts_with("Work")));
        assert_eq!(
            week_events(&store, day(MONDAY), None, day(1), true).len(),
            23
        );
        let first = &ev[0];
        assert_eq!(first.summary, "Run: Run/walk");
        assert_eq!(
            first.key,
            EventKey::Item(store.weeks[0].plan.days[0][3].id.clone())
        );
        assert_eq!(first.start, Some(at(day(MONDAY), 19, 0)));
        assert_eq!(
            first.description,
            "Effort: Run/walk\n10 × 1 min run / 1 min walk\nBefore: Fasted is fine, or a small snack.\nDuring: Water only.\nAfter: Normal next meal."
        );
    }

    #[test]
    fn past_days_show_what_was_done() {
        let store = store();
        let week = &store.weeks[0].plan;
        let monday = day(MONDAY);
        let run = workout("RUNNING", at(monday, 19, 10), 30);
        let walk = workout("WALKING", at(monday, 12, 0), 30);
        let a = actuals(vec![run.clone(), walk.clone()]);
        let today = day(MONDAY + 2);
        let ev = week_events(&store, monday, Some(&a), today, false);
        let of = |key: &EventKey| ev.iter().find(|e| &e.key == key).unwrap();

        let run_item = &week.days[0][3];
        let done = of(&EventKey::Item(run_item.id.clone()));
        assert_eq!(done.summary, "✓ Run 30m (plan Run/walk 25m)");
        assert_eq!(done.start, Some(at(monday, 19, 10)));
        assert_eq!(done.end, Some(at(monday, 19, 40)));
        assert!(done.description.starts_with("Running\n19:10–19:40 · 30m\n"));
        assert!(done.description.ends_with("\n10 × 1 min run / 1 min walk"));

        let extra = of(&EventKey::Workout(walk.id.clone()));
        assert_eq!(extra.summary, "✓ Walking 30m");
        assert_eq!(extra.start, Some(at(monday, 12, 0)));
        assert_eq!(extra.date, monday);

        let gym = &week.days[1][0];
        let missed = of(&EventKey::Item(gym.id.clone()));
        assert_eq!(missed.summary, actuals::missed_title(&store.library, gym));
        assert!(missed.summary.starts_with("✗ "));
        assert_eq!(missed.start, Some(at(day(MONDAY + 1), 7, 0)));

        // Today's and later items are as planned.
        let later: Vec<_> = ev.iter().filter(|e| e.date >= today).collect();
        assert!(!later.is_empty());
        assert!(later.iter().all(|e| !e.summary.starts_with(['✓', '✗'])));
        assert_eq!(ev.len(), 15);
    }

    #[test]
    fn only_past_day_changes_count() {
        let store = store();
        let monday = day(MONDAY);
        let today = day(MONDAY + 2);
        let changed = |before: &WeekActuals, after: &WeekActuals| {
            past_events_changed(&store, monday, Some(before), after, today, false)
        };
        let run = workout("RUNNING", at(monday, 19, 10), 30);
        let old = actuals(vec![]);
        let new = actuals(vec![run.clone()]);
        assert!(changed(&old, &new));
        assert!(!changed(&new, &new));
        let refetched = WeekActuals {
            fetched_at: DateTime::UNIX_EPOCH + TimeDelta::hours(1),
            ..new.clone()
        };
        assert!(!changed(&new, &refetched));
        let today_ride = workout("BIKING", at(today, 7, 0), 45);
        let later = workout("RUNNING", at(day(MONDAY + 4), 7, 0), 45);
        assert!(!changed(&new, &actuals(vec![run, today_ride, later])));
        assert!(past_events_changed(
            &store, monday, None, &old, today, false
        ));
    }

    #[test]
    fn unstored_week_shows_only_its_workouts() {
        let store = Store::default();
        let monday = day(MONDAY);
        let walk = workout("WALKING", at(monday, 12, 0), 30);
        let a = actuals(vec![walk.clone()]);
        let ev = week_events(&store, monday, Some(&a), day(MONDAY + 3), false);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].key, EventKey::Workout(walk.id));
        assert!(week_events(&store, monday, None, day(MONDAY + 3), true).is_empty());
    }

    #[test]
    fn ics_has_one_event_each_without_rrule() {
        let store = store();
        let ev = range_events(&store, day(MONDAY), 2, |_| None, day(1), false);
        assert_eq!(ev.len(), 14 + 13);
        let ics = to_ics("Training", &ev);
        assert!(!ics.contains("RRULE"));
        let cal: Calendar = ics.parse().unwrap();
        assert_eq!(cal.get_name(), Some("Training"));
        let parsed: Vec<&Event> = cal.events().collect();
        assert_eq!(parsed.len(), ev.len());
        let uids: HashSet<_> = parsed.iter().filter_map(|p| p.get_uid()).collect();
        assert_eq!(uids.len(), ev.len());
        for (p, e) in parsed.iter().zip(&ev) {
            assert_eq!(
                p.get_uid(),
                Some(format!("{}-{}@tapas", e.week, e.key).as_str())
            );
            assert_eq!(p.get_summary(), Some(e.summary.as_str()));
            assert_eq!(p.get_description(), Some(e.description.as_str()));
        }
        let late = parsed[13];
        assert_eq!(late.property_value("DTSTART"), Some("20260927T233000"));
        assert_eq!(late.property_value("DTEND"), Some("20260928T003000"));
    }

    #[test]
    fn all_day_ics_and_csv() {
        let mut store = Store::default();
        let rest = new_item(&store.library, "rest").unwrap();
        store.week_mut_or_insert(day(28)).plan.days[2].push(rest);
        let ev = week_events(&store, day(28), None, day(1), false);
        let ics = to_ics("R", &ev);
        assert!(ics.contains("DTSTART;VALUE=DATE:20260930"));
        assert!(ics.contains("DTEND;VALUE=DATE:20261001"));
        let csv = to_google_csv(&ev).unwrap();
        assert!(csv.ends_with(
            "\"Recovery: Rest day\",\"09/30/2026\",\"\",\"09/30/2026\",\"\",\"True\",\"Effort: Rest day\nAfter: Aim for 8+ hours of sleep; this is where the adaptation happens.\",\"\",\"True\"\r\n"
        ));
    }

    #[test]
    fn csv_has_one_row_per_event() {
        let store = store();
        let ev = range_events(&store, day(MONDAY), 2, |_| None, day(1), false);
        let csv = to_google_csv(&ev).unwrap();
        assert!(csv.starts_with("\"Subject\",\"Start Date\",\"Start Time\",\"End Date\",\"End Time\",\"All Day Event\",\"Description\",\"Location\",\"Private\"\r\n"));
        let mut r = csv::Reader::from_reader(csv.as_bytes());
        let rows: Vec<csv::StringRecord> = r.records().map(|x| x.unwrap()).collect();
        assert_eq!(rows.len(), ev.len());
        assert_eq!(
            &rows[0].iter().take(6).collect::<Vec<_>>(),
            &[
                "Run: Run/walk",
                "09/21/2026",
                "7:00 PM",
                "09/21/2026",
                "7:25 PM",
                "False"
            ]
        );
        assert_eq!(&rows[14][1], "09/28/2026");
        assert_eq!(
            rows[13].iter().skip(1).take(4).collect::<Vec<_>>(),
            ["09/27/2026", "11:30 PM", "09/28/2026", "12:30 AM"]
        );
        assert_eq!(&rows[0][6], ev[0].description);
    }
}
