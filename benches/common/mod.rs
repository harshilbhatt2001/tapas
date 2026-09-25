//! Shared bench fixtures. Each bench binary uses a subset, so unused ones are expected.
#![allow(dead_code)]

use chrono::{Days, Local, NaiveDate, NaiveTime, TimeZone};
use tapas::{
    google::health::Workout,
    library::{default_library, starter_plan},
    model::{Item, Library, Plan, SessionType, Store},
};

/// Monday used by every date-dependent bench, so results don't drift with the calendar.
#[must_use]
pub fn monday() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 9, 28).expect("valid date")
}

/// The real-world size: the default library and the artifact's starter week (~30 items).
#[must_use]
pub fn starter() -> (Library, Plan) {
    (default_library(), starter_plan("bench"))
}

/// `per_day` items on each day, cycling through every type and effort of `lib`. Starts step
/// by 45 minutes from 06:00 while durations are the effort defaults, so neighbours overlap.
#[must_use]
pub fn plan_with(lib: &Library, per_day: usize) -> Plan {
    let pairs: Vec<(&SessionType, usize)> = lib
        .types
        .iter()
        .flat_map(|t| (0..t.efforts.len()).map(move |e| (t, e)))
        .collect();
    let mut plan = Plan::new(format!("{per_day} per day"));
    for (d, day) in plan.days.iter_mut().enumerate() {
        for i in 0..per_day {
            let (t, e) = pairs[(d * per_day + i) % pairs.len()];
            let e = &t.efforts[e];
            let min = u32::try_from((6 * 60 + i * 45) % (24 * 60)).expect("small");
            day.push(Item {
                id: format!("d{d}-i{i}"),
                type_key: t.key.clone(),
                effort: e.key.clone(),
                start: NaiveTime::from_hms_opt(min / 60, min % 60, 0).expect("valid time"),
                dur: e.dur.max(30),
                notes: String::new(),
            });
        }
    }
    plan
}

/// 7 days × 12 items with deliberate overlaps.
#[must_use]
pub fn dense() -> (Library, Plan) {
    let lib = default_library();
    let plan = plan_with(&lib, 12);
    (lib, plan)
}

/// 7 days × 200 items: the pathological case for anything quadratic.
#[must_use]
pub fn huge() -> (Library, Plan) {
    let lib = default_library();
    let plan = plan_with(&lib, 200);
    (lib, plan)
}

/// The default library plus `extra` custom copies of its types (keys `custom-<n>`), so
/// linear lookups have something to scan. Lookups for built-in keys still hit early.
#[must_use]
pub fn library_with(extra: usize) -> Library {
    let mut lib = default_library();
    let base = lib.types.clone();
    for n in 0..extra {
        let mut t = base[n % base.len()].clone();
        t.key = format!("custom-{n}");
        t.label = format!("Custom {n}");
        lib.types.push(t);
    }
    lib
}

/// A heavy store: 50 `dense` plans and 40 custom types.
#[must_use]
pub fn store_many() -> Store {
    let library = library_with(40);
    let plans = (0..50)
        .map(|n| {
            let mut p = plan_with(&library, 12);
            p.name = format!("Plan {n}");
            p
        })
        .collect();
    Store {
        library,
        plans,
        ..Store::default()
    }
}

/// A store holding just the starter plan: what a typical user has.
#[must_use]
pub fn store_starter() -> Store {
    Store {
        plans: vec![starter_plan("bench")],
        ..Store::default()
    }
}

/// One synthetic workout a day for a year starting at `monday()`, cycling exercise types.
#[must_use]
pub fn workouts_year() -> Vec<Workout> {
    const TYPES: [&str; 6] = [
        "RUNNING",
        "CYCLING",
        "SWIMMING",
        "STRENGTH_TRAINING",
        "PADEL",
        "YOGA",
    ];
    (0..365u32)
        .map(|d| {
            let date = monday() + Days::new(d.into());
            let start = Local
                .from_local_datetime(&date.and_hms_opt(7, 0, 0).expect("valid time"))
                .earliest()
                .expect("7:00 exists every day");
            Workout {
                exercise_type: TYPES[d as usize % TYPES.len()].into(),
                start,
                minutes: 30 + d % 90,
                kcal: Some(f64::from(300 + d % 400)),
            }
        })
        .collect()
}
