//! The built-in session library and starter week.

use chrono::{DateTime, NaiveTime};
use uuid::Uuid;

use crate::model::{Category, Effort, Item, Kind, Library, Plan, SessionType};

fn ef(key: &str, label: &str, rpe: f64, met: f64, dur: u32) -> Effort {
    Effort {
        key: key.into(),
        label: label.into(),
        rpe,
        met,
        dur,
        legs: false,
    }
}

#[allow(clippy::too_many_arguments)]
fn ty(
    key: &str,
    label: &str,
    color: &str,
    kind: Kind,
    category: Category,
    start: NaiveTime,
    counted: bool,
    efforts: Vec<Effort>,
) -> SessionType {
    SessionType {
        key: key.into(),
        label: label.into(),
        color: color.into(),
        kind,
        category,
        start,
        counted,
        efforts,
        updated_at: DateTime::UNIX_EPOCH,
    }
}

fn t(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).expect("valid time of day")
}

#[must_use]
#[expect(clippy::too_many_lines, reason = "one flat table of starter data")]
pub fn default_library() -> Library {
    use Category::{Life, Rest, Train};
    let heavy = Effort {
        legs: true,
        ..ef("heavy", "Heavy strength", 6.0, 5.0, 55)
    };
    Library {
        types: vec![
            ty(
                "swim",
                "Swim",
                "#5B9BE0",
                Kind::Swim,
                Train,
                t(7, 0),
                true,
                vec![
                    ef("technique", "Technique", 3.0, 5.0, 45),
                    ef("easy", "Easy aerobic", 4.0, 6.0, 45),
                    ef("threshold", "Threshold sets", 7.0, 9.5, 50),
                    ef("long", "Long swim", 5.0, 7.0, 60),
                    ef("openwater", "Open water", 5.0, 7.5, 45),
                ],
            ),
            ty(
                "bike",
                "Bike",
                "#E0A145",
                Kind::Bike,
                Train,
                t(18, 0),
                true,
                vec![
                    ef("recovery", "Recovery spin", 2.0, 4.0, 45),
                    ef("z2", "Endurance Z2", 3.0, 6.8, 75),
                    ef("tempo", "Tempo", 6.0, 8.5, 75),
                    ef("intervals", "Intervals", 8.0, 10.5, 60),
                    ef("long", "Long ride", 4.0, 7.5, 180),
                ],
            ),
            ty(
                "run",
                "Run",
                "#5DB585",
                Kind::Run,
                Train,
                t(7, 0),
                true,
                vec![
                    ef("runwalk", "Run/walk", 3.0, 6.0, 25),
                    ef("easy", "Easy", 4.0, 8.3, 35),
                    ef("tempo", "Tempo", 7.0, 10.0, 45),
                    ef("intervals", "Intervals", 8.0, 11.0, 45),
                    ef("long", "Long run", 5.0, 9.0, 75),
                ],
            ),
            ty(
                "gym",
                "Gym",
                "#A286DD",
                Kind::Gym,
                Train,
                t(7, 0),
                true,
                vec![
                    heavy,
                    ef("maint", "Maintenance", 5.0, 4.5, 40),
                    ef("tendon", "Tendon and core", 3.0, 3.5, 20),
                    ef("mobility", "Mobility", 2.0, 2.5, 25),
                ],
            ),
            ty(
                "brick",
                "Brick",
                "#DB7A63",
                Kind::Brick,
                Train,
                t(9, 0),
                true,
                vec![
                    ef("easy", "Easy brick", 4.0, 8.0, 90),
                    ef("race", "Race-pace brick", 7.0, 9.5, 120),
                ],
            ),
            ty(
                "padel",
                "Padel",
                "#E27BAE",
                Kind::Padel,
                Train,
                t(11, 30),
                false,
                vec![
                    ef("social", "Social game", 5.0, 6.0, 90),
                    ef("match", "Competitive match", 7.0, 7.5, 90),
                    ef("drills", "Coaching or drills", 4.0, 5.0, 60),
                ],
            ),
            ty(
                "work",
                "Work",
                "#8E9AA6",
                Kind::Work,
                Life,
                t(9, 0),
                false,
                vec![
                    ef("home", "From home", 0.0, 1.0, 480),
                    ef("office", "At the office", 0.0, 1.0, 480),
                ],
            ),
            ty(
                "commute",
                "Commute",
                "#8E9AA6",
                Kind::Commute,
                Life,
                t(7, 30),
                false,
                vec![
                    ef("transit", "Train or car", 0.0, 1.3, 60),
                    ef("walk", "Walk", 1.0, 3.5, 30),
                    ef("bike", "By bike", 2.0, 6.8, 30),
                ],
            ),
            ty(
                "rest",
                "Recovery",
                "#56BDB8",
                Kind::Rest,
                Rest,
                t(20, 0),
                false,
                vec![
                    ef("off", "Rest day", 0.0, 1.0, 0),
                    ef("mobility", "Stretch and foam roll", 1.0, 2.5, 20),
                    ef("sleep", "Early night", 0.0, 1.0, 0),
                ],
            ),
        ],
    }
}

/// A new item of `type_key` with its first effort and default start and duration.
#[must_use]
pub fn new_item(lib: &Library, type_key: &str) -> Option<Item> {
    let t = lib.get(type_key)?;
    let e = t.efforts.first()?;
    Some(Item {
        id: Uuid::new_v4().to_string(),
        type_key: t.key.clone(),
        effort: e.key.clone(),
        start: t.start,
        dur: e.dur,
        notes: String::new(),
        updated_at: DateTime::UNIX_EPOCH,
    })
}

/// The artifact's starter week.
#[must_use]
pub fn starter_days() -> [Vec<Item>; 7] {
    let mk = |ty: &str, effort: &str, start: NaiveTime, dur: u32, notes: &str| Item {
        id: Uuid::new_v4().to_string(),
        type_key: ty.into(),
        effort: effort.into(),
        start,
        dur,
        notes: notes.into(),
        updated_at: DateTime::UNIX_EPOCH,
    };
    let wfh = || mk("work", "home", t(9, 0), 480, "");
    let off = |back: NaiveTime, wd: u32| {
        vec![
            mk("commute", "transit", t(7, 30), 60, "DH → AMS"),
            mk("work", "office", t(8, 30), wd, ""),
            mk("commute", "transit", back, 90, "AMS → home"),
        ]
    };
    let with = |mut v: Vec<Item>, more: Vec<Item>| {
        v.extend(more);
        v
    };
    [
        with(
            off(t(17, 6), 510),
            vec![mk(
                "run",
                "runwalk",
                t(19, 0),
                25,
                "10 × 1 min run / 1 min walk",
            )],
        ),
        vec![
            mk("gym", "heavy", t(7, 0), 55, ""),
            wfh(),
            mk("swim", "technique", t(18, 30), 45, ""),
        ],
        vec![
            mk("run", "runwalk", t(7, 0), 25, ""),
            wfh(),
            mk("bike", "z2", t(17, 30), 75, ""),
        ],
        with(
            off(t(18, 6), 570),
            vec![mk("rest", "mobility", t(20, 0), 20, "")],
        ),
        vec![
            mk(
                "gym",
                "maint",
                t(7, 0),
                40,
                "Upper body and core, no heavy legs before Saturday",
            ),
            wfh(),
            mk("swim", "technique", t(18, 0), 45, ""),
        ],
        vec![mk("bike", "long", t(9, 0), 240, "Ride block toward Nov 8")],
        vec![
            mk("run", "runwalk", t(10, 0), 25, ""),
            mk("padel", "social", t(11, 30), 90, ""),
            mk("swim", "technique", t(16, 0), 45, ""),
            mk("gym", "tendon", t(17, 0), 20, ""),
        ],
    ]
}

#[must_use]
pub fn starter_plan(name: &str) -> Plan {
    Plan {
        days: starter_days(),
        ..Plan::new(name)
    }
}
