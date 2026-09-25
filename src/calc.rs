//! Energy, load, fuelling guidance and weekly checks, ported from the artifact.
//!
//! Everything keys off the session type's `Kind`/`Category` and the effort's fields rather
//! than type keys, so custom library types get sensible treatment. Items whose type is no
//! longer in the library count for nothing.

use std::collections::HashSet;

use crate::model::{Category, DAYS, Effort, Item, Kind, Library, Plan, Profile, SessionType};

fn resolve<'a>(lib: &'a Library, it: &Item) -> Option<(&'a SessionType, &'a Effort)> {
    lib.resolve(it)
}

fn kind(lib: &Library, it: &Item) -> Option<Kind> {
    lib.get(&it.type_key).map(|t| t.kind)
}

fn rpe(lib: &Library, it: &Item) -> f64 {
    resolve(lib, it).map_or(0.0, |(_, e)| e.rpe)
}

pub fn is_train(lib: &Library, it: &Item) -> bool {
    lib.get(&it.type_key)
        .is_some_and(|t| t.category == Category::Train)
}

pub fn is_long(lib: &Library, it: &Item) -> bool {
    let Some(k) = kind(lib, it) else { return false };
    it.effort == "long"
        || match k {
            Kind::Bike => it.dur >= 150,
            Kind::Run => it.dur >= 75,
            Kind::Brick => it.dur >= 120,
            _ => false,
        }
}

pub fn is_hard(lib: &Library, it: &Item) -> bool {
    is_train(lib, it) && rpe(lib, it) >= 6.0 && kind(lib, it) != Some(Kind::Gym)
}

/// Energy on top of the day's base, kcal.
pub fn kcal(lib: &Library, weight: f64, it: &Item) -> f64 {
    resolve(lib, it).map_or(0.0, |(_, e)| {
        (e.met - 1.0).max(0.0) * weight * (f64::from(it.dur) / 60.0)
    })
}

/// Training load: hours times RPE.
pub fn load(lib: &Library, it: &Item) -> f64 {
    f64::from(it.dur) / 60.0 * rpe(lib, it)
}

/// Card tag for sessions long enough to need fuel on the go.
pub fn fuel_on_the_go(lib: &Library, it: &Item) -> bool {
    is_train(lib, it) && kind(lib, it) != Some(Kind::Gym) && it.dur >= 75
}

/// `Swim (technique)`
pub fn label(lib: &Library, it: &Item) -> String {
    match resolve(lib, it) {
        Some((t, e)) => format!("{} ({})", t.label, e.label.to_lowercase()),
        None => format!("{} ({})", it.type_key, it.effort),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Rest,
    Easy,
    Moderate,
    Hard,
    Big,
}

impl Level {
    pub fn name(self) -> &'static str {
        match self {
            Level::Rest => "Rest",
            Level::Easy => "Easy",
            Level::Moderate => "Moderate",
            Level::Hard => "Hard",
            Level::Big => "Big",
        }
    }
}

/// Energy and macros for one day.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DayCalc {
    pub kc: i64,
    pub p: i64,
    pub c: i64,
    pub f: i64,
    /// Training load of the day.
    pub ld: f64,
    pub lvl: Level,
    pub train_min: u32,
}

/// Extra energy goes 85% to carbs and 15% to fat on top of the base.
pub fn day_calc(lib: &Library, profile: &Profile, items: &[Item]) -> DayCalc {
    let b = profile.base;
    let extra: f64 = items.iter().map(|x| kcal(lib, profile.weight, x)).sum();
    let train = || items.iter().filter(|x| is_train(lib, x));
    let train_min = train().map(|x| x.dur).sum();
    let ld: f64 = train().map(|x| load(lib, x)).sum();
    let lvl = match ld {
        l if l < 1.0 => Level::Rest,
        l if l < 4.0 => Level::Easy,
        l if l < 8.0 => Level::Moderate,
        l if l < 13.0 => Level::Hard,
        _ => Level::Big,
    };
    DayCalc {
        kc: (b.kcal + extra).round() as i64,
        p: b.p.round() as i64,
        c: (b.c + extra * 0.85 / 4.0).round() as i64,
        f: (b.f + extra * 0.15 / 9.0).round() as i64,
        ld,
        lvl,
        train_min,
    }
}

/// Fuelling guidance for one session; unset parts are not shown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Guide {
    pub before: Option<String>,
    pub during: Option<String>,
    pub after: Option<String>,
    pub note: Option<String>,
}

pub fn guide(lib: &Library, weight: f64, it: &Item) -> Guide {
    let mut out = Guide::default();
    let Some((t, e)) = resolve(lib, it) else {
        return out;
    };
    let d = it.dur;
    let s = |x: &str| Some(x.to_string());
    if t.category != Category::Train {
        if t.kind == Kind::Commute && it.effort == "bike" {
            out.during = s("Water. Counts toward the day's energy.");
        }
        if t.kind == Kind::Rest {
            out.after = s("Aim for 8+ hours of sleep; this is where the adaptation happens.");
        }
        if t.kind == Kind::Work {
            out.during = s("Keep a protein snack and water at hand; stand or walk between calls.");
        }
        return out;
    }
    let hard = e.rpe >= 6.0;
    let long = is_long(lib, it);
    if t.kind == Kind::Padel {
        out.before = s("Normal meal 2–3 h before. Warm up calves and ankles properly.");
        out.during = s(if d > 75 {
            "Water plus electrolytes; 30–40 g carbs if it runs past 90 min."
        } else {
            "Water."
        });
        out.after = s("25–40 g protein and carbs at the next meal.");
        out.note = s(
            "Stops, lunges and jumps load calves and Achilles like running does. Count it as run-type load.",
        );
        return out;
    }
    if t.kind == Kind::Gym {
        out.before = s(if e.legs {
            "Keep it 48 h away from long runs, long rides and key bike sessions."
        } else {
            "Any normal meal 1–3 h before."
        });
        out.after = s(if e.rpe >= 5.0 {
            "25–40 g protein within 1–2 h."
        } else {
            "Nothing special."
        });
        return out;
    }
    let hours = f64::from(d) / 60.0;
    out.before = Some(if long || hard {
        format!(
            "Carb-rich meal 2–3 h before (about {} g carbs), low fibre and fat.",
            (weight * 1.5).round()
        )
    } else if d <= 60 {
        "Fasted is fine, or a small snack.".into()
    } else {
        "Light carb snack 30–60 min before.".into()
    });
    out.during = Some(if t.kind == Kind::Swim {
        if d > 75 {
            "Bottle on deck; 30 g carbs if over 75 min.".into()
        } else {
            "Water only.".into()
        }
    } else if d < 75 && !hard {
        "Water only.".into()
    } else if d < 75 {
        "Water; optional 20–30 g carbs.".into()
    } else if d <= 150 {
        format!(
            "40–60 g carbs per hour (about {} g total).",
            (hours * 50.0).round()
        )
    } else {
        format!(
            "60–90 g carbs per hour (about {} g total), glucose–fructose mix, 300–600 mg sodium per hour. Rehearse race fuel.",
            (hours * 75.0).round()
        )
    });
    out.after = Some(if long || hard {
        format!(
            "Within 1 h: 25–40 g protein plus about {} g carbs.",
            weight.round()
        )
    } else {
        "Normal next meal.".into()
    });
    if t.kind == Kind::Run && it.effort == "runwalk" {
        out.note = s(
            "Talk-test pace. Next-morning shins and Achilles decide progression, not how easy it felt.",
        );
    }
    out
}

/// Timed items of a day in start order, with each neighbour pair that overlaps.
fn overlaps(items: &[Item]) -> Vec<(&Item, &Item)> {
    let mut s: Vec<&Item> = items.iter().filter(|x| x.dur > 0).collect();
    s.sort_by_key(|x| x.start);
    s.windows(2)
        .filter(|w| w[1].start_min() < w[0].start_min() + w[0].dur)
        .map(|w| (w[0], w[1]))
        .collect()
}

/// Ids of items that overlap a neighbour on the same day.
pub fn clashes(plan: &Plan) -> HashSet<String> {
    plan.days
        .iter()
        .flat_map(|d| overlaps(d))
        .flat_map(|(a, b)| [a.id.clone(), b.id.clone()])
        .collect()
}

/// Conflict and load warnings for the week, deduplicated in order.
pub fn checks(lib: &Library, plan: &Plan) -> Vec<String> {
    let days = &plan.days;
    let k = |it: &Item| kind(lib, it);
    let key = |it: &Item| is_long(lib, it) || is_hard(lib, it);
    let mut w = Vec::new();

    for (i, items) in days.iter().enumerate() {
        for (a, b) in overlaps(items) {
            w.push(format!(
                "{}: {} overlaps {}.",
                DAYS[i],
                label(lib, a),
                label(lib, b)
            ));
        }
    }
    for i in 0..7 {
        let heavy = days[i]
            .iter()
            .any(|x| resolve(lib, x).is_some_and(|(_, e)| e.legs));
        if !heavy {
            continue;
        }
        for off in [1, 2] {
            let j = (i + off) % 7;
            let found = days[j]
                .iter()
                .find(|x| matches!(k(x), Some(Kind::Run | Kind::Bike | Kind::Brick)) && key(x));
            if let Some(x) = found {
                w.push(format!(
                    "Heavy legs on {} is within 48 h of {} on {}.",
                    DAYS[i],
                    label(lib, x),
                    DAYS[j]
                ));
                break;
            }
        }
    }
    for i in 0..7 {
        let j = (i + 1) % 7;
        let long_ride = days[j]
            .iter()
            .any(|x| k(x) == Some(Kind::Bike) && is_long(lib, x));
        if long_ride && days[i].iter().any(|x| k(x) == Some(Kind::Run)) {
            w.push(format!(
                "Run on {} the day before the long ride on {}.",
                DAYS[i], DAYS[j]
            ));
        }
    }
    for i in 0..7 {
        let j = (i + 1) % 7;
        let padel = days[i]
            .iter()
            .any(|x| k(x) == Some(Kind::Padel) && rpe(lib, x) >= 7.0);
        let run = days[j].iter().find(|x| k(x) == Some(Kind::Run) && key(x));
        if let (true, Some(r)) = (padel, run) {
            w.push(format!(
                "Competitive padel on {} the day before {} on {}.",
                DAYS[i],
                label(lib, r),
                DAYS[j]
            ));
        }
    }
    let hard_days: Vec<bool> = days.iter().map(|d| d.iter().any(key)).collect();
    for i in 0..7 {
        if hard_days[i] && hard_days[(i + 1) % 7] && hard_days[(i + 2) % 7] {
            w.push(format!(
                "Three hard or long days in a row from {}.",
                DAYS[i]
            ));
        }
    }
    let train_days = days
        .iter()
        .filter(|d| d.iter().any(|x| is_train(lib, x) && rpe(lib, x) >= 3.0))
        .count();
    if !plan.is_empty() && train_days >= 7 {
        w.push("No lighter day this week. Keep at least one real rest or recovery day.".into());
    }
    let runs: Vec<&Item> = days
        .iter()
        .flatten()
        .filter(|x| k(x) == Some(Kind::Run))
        .collect();
    let longest = runs.iter().map(|x| x.dur).max().unwrap_or(0);
    let tot: u32 = runs.iter().map(|x| x.dur).sum();
    if runs.len() > 1 && f64::from(longest) > f64::from(tot) * 0.5 {
        w.push("Long run is over half of weekly run time. Spread it out.".into());
    }
    if !plan.is_empty() && runs.is_empty() {
        w.push("No running this week. Tendons need frequent, small doses.".into());
    }

    let mut seen = HashSet::new();
    w.retain(|x| seen.insert(x.clone()));
    w
}

/// Shown when `checks` is empty.
pub fn no_checks_message(plan: &Plan) -> &'static str {
    if plan.is_empty() {
        "Add sessions to see checks."
    } else {
        "No conflicts or load problems found."
    }
}

/// Minutes of one training type over the week.
#[derive(Clone, Debug, PartialEq)]
pub struct Discipline {
    pub key: String,
    pub label: String,
    pub color: String,
    pub minutes: u32,
    /// Counts toward the weekly hours; the artifact adds "(not in weekly hours)" otherwise.
    pub counted: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    /// Weekly hours of counted types, to one decimal.
    pub hours: f64,
    pub sessions: usize,
    pub hard: usize,
    pub long: usize,
    /// Every training type in library order.
    pub disciplines: Vec<Discipline>,
}

pub fn summary(lib: &Library, plan: &Plan) -> Summary {
    let all: Vec<&Item> = plan.days.iter().flatten().collect();
    let counted: Vec<&Item> = all
        .iter()
        .copied()
        .filter(|x| lib.get(&x.type_key).is_some_and(|t| t.counted))
        .collect();
    let tot: u32 = counted.iter().map(|x| x.dur).sum();
    let disciplines = lib
        .types
        .iter()
        .filter(|t| t.category == Category::Train)
        .map(|t| Discipline {
            key: t.key.clone(),
            label: t.label.clone(),
            color: t.color.clone(),
            minutes: all
                .iter()
                .filter(|x| x.type_key == t.key)
                .map(|x| x.dur)
                .sum(),
            counted: t.counted,
        })
        .collect();
    Summary {
        hours: (f64::from(tot) / 6.0).round() / 10.0,
        sessions: counted.len(),
        hard: counted.iter().filter(|x| is_hard(lib, x)).count(),
        long: counted.iter().filter(|x| is_long(lib, x)).count(),
        disciplines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{default_library, new_item, starter_plan};
    use chrono::NaiveTime;

    fn it(lib: &Library, ty: &str, effort: &str, start: &str, dur: u32) -> Item {
        Item {
            effort: effort.into(),
            start: NaiveTime::parse_from_str(start, "%H:%M").unwrap(),
            dur,
            ..new_item(lib, ty).unwrap()
        }
    }

    fn plan(days: [Vec<Item>; 7]) -> Plan {
        Plan {
            days,
            ..Plan::new("t")
        }
    }

    #[test]
    fn starter_week_numbers() {
        let lib = default_library();
        let p = starter_plan("s");
        let prof = Profile::default();
        let mon = day_calc(&lib, &prof, &p.days[0]);
        assert_eq!((mon.kc, mon.p, mon.c, mon.f), (2807, 150, 359, 83));
        assert_eq!((mon.lvl, mon.train_min), (Level::Easy, 25));
        let sat = day_calc(&lib, &prof, &p.days[5]);
        assert_eq!((sat.kc, sat.c, sat.f), (4498, 718, 112));
        assert_eq!(sat.lvl, Level::Big);
        assert_eq!(sat.ld, 16.0);
        let thu = day_calc(&lib, &prof, &p.days[3]);
        assert_eq!(thu.lvl, Level::Rest);

        let s = summary(&lib, &p);
        assert_eq!((s.hours, s.sessions, s.hard, s.long), (10.7, 11, 0, 1));
        let padel = s.disciplines.iter().find(|d| d.key == "padel").unwrap();
        assert_eq!((padel.minutes, padel.counted), (90, false));
        assert_eq!(s.disciplines.len(), 6);

        assert!(checks(&lib, &p).is_empty());
        assert!(clashes(&p).is_empty());
        assert_eq!(
            no_checks_message(&p),
            "No conflicts or load problems found."
        );
        assert_eq!(
            no_checks_message(&Plan::new("e")),
            "Add sessions to see checks."
        );
    }

    /// Golden values from actually running the artifact's JS `dayCalc` (node, weight 73,
    /// default base 2600/150/315/80) over `starter_days()`, for the days `starter_week_numbers`
    /// doesn't already pin.
    #[test]
    fn full_week_day_calc_parity() {
        let lib = default_library();
        let p = starter_plan("s");
        let prof = Profile::default();
        let d = |i: usize| day_calc(&lib, &prof, &p.days[i]);

        let tue = d(1);
        assert_eq!((tue.kc, tue.p, tue.c, tue.f), (3087, 150, 418, 88));
        assert_eq!(
            (tue.ld, tue.lvl, tue.train_min),
            (7.75, Level::Moderate, 100)
        );

        let wed = d(2);
        assert_eq!((wed.kc, wed.p, wed.c, wed.f), (3281, 150, 460, 91));
        assert_eq!(
            (wed.ld, wed.lvl, wed.train_min),
            (5.0, Level::Moderate, 100)
        );

        let fri = d(4);
        assert_eq!((fri.kc, fri.p, fri.c, fri.f), (2989, 150, 398, 86));
        assert_eq!(
            (fri.ld, fri.lvl, fri.train_min),
            (5.583_333_333_333_333, Level::Moderate, 85)
        );

        let sun = d(6);
        assert_eq!((sun.kc, sun.p, sun.c, sun.f), (3579, 150, 523, 96));
        assert_eq!((sun.ld, sun.lvl, sun.train_min), (12.0, Level::Hard, 180));

        let s = summary(&lib, &p);
        for (key, minutes, counted) in [
            ("swim", 135, true),
            ("bike", 315, true),
            ("run", 75, true),
            ("gym", 115, true),
            ("brick", 0, true),
            ("padel", 90, false),
        ] {
            let disc = s.disciplines.iter().find(|d| d.key == key).unwrap();
            assert_eq!((disc.minutes, disc.counted), (minutes, counted), "{key}");
        }
    }

    /// Golden wording from the artifact's `guide()` for branches `long_ride_guide` and
    /// `other_kind_uses_endurance_branch` don't already cover.
    #[test]
    fn guide_branches_parity() {
        let lib = default_library();
        let w = 73.0;

        let padel = guide(&lib, w, &it(&lib, "padel", "match", "11:30", 90));
        assert_eq!(
            padel.before.as_deref(),
            Some("Normal meal 2–3 h before. Warm up calves and ankles properly.")
        );
        assert_eq!(
            padel.during.as_deref(),
            Some("Water plus electrolytes; 30–40 g carbs if it runs past 90 min.")
        );
        assert_eq!(
            padel.after.as_deref(),
            Some("25–40 g protein and carbs at the next meal.")
        );
        assert_eq!(
            padel.note.as_deref(),
            Some(
                "Stops, lunges and jumps load calves and Achilles like running does. Count it as run-type load."
            )
        );

        let gym_heavy = guide(&lib, w, &it(&lib, "gym", "heavy", "07:00", 55));
        assert_eq!(
            gym_heavy.before.as_deref(),
            Some("Keep it 48 h away from long runs, long rides and key bike sessions.")
        );
        assert_eq!(
            gym_heavy.after.as_deref(),
            Some("25–40 g protein within 1–2 h.")
        );

        let gym_maint = guide(&lib, w, &it(&lib, "gym", "maint", "07:00", 40));
        assert_eq!(
            gym_maint.before.as_deref(),
            Some("Any normal meal 1–3 h before.")
        );
        assert_eq!(
            gym_maint.after.as_deref(),
            Some("25–40 g protein within 1–2 h.")
        );

        let gym_mobility = guide(&lib, w, &it(&lib, "gym", "mobility", "07:00", 25));
        assert_eq!(gym_mobility.after.as_deref(), Some("Nothing special."));

        let swim_long = guide(&lib, w, &it(&lib, "swim", "long", "07:00", 90));
        assert_eq!(
            swim_long.during.as_deref(),
            Some("Bottle on deck; 30 g carbs if over 75 min.")
        );
        let swim_short = guide(&lib, w, &it(&lib, "swim", "technique", "07:00", 45));
        assert_eq!(swim_short.during.as_deref(), Some("Water only."));

        // Under 75 min, not hard: water only.
        let run_easy_35 = guide(&lib, w, &it(&lib, "run", "easy", "07:00", 35));
        assert_eq!(run_easy_35.during.as_deref(), Some("Water only."));
        // Under 75 min, hard: water plus optional carbs.
        let run_tempo_45 = guide(&lib, w, &it(&lib, "run", "tempo", "07:00", 45));
        assert_eq!(
            run_tempo_45.during.as_deref(),
            Some("Water; optional 20–30 g carbs.")
        );
        // 75 to 150 min.
        let run_easy_100 = guide(&lib, w, &it(&lib, "run", "easy", "07:00", 100));
        assert_eq!(
            run_easy_100.during.as_deref(),
            Some("40–60 g carbs per hour (about 83 g total).")
        );
        // Over 150 min.
        let bike_long_200 = guide(&lib, w, &it(&lib, "bike", "long", "07:00", 200));
        assert_eq!(
            bike_long_200.during.as_deref(),
            Some(
                "60–90 g carbs per hour (about 250 g total), glucose–fructose mix, 300–600 mg sodium per hour. Rehearse race fuel."
            )
        );

        let runwalk = guide(&lib, w, &it(&lib, "run", "runwalk", "07:00", 25));
        assert_eq!(
            runwalk.note.as_deref(),
            Some(
                "Talk-test pace. Next-morning shins and Achilles decide progression, not how easy it felt."
            )
        );

        let commute_bike = guide(&lib, w, &it(&lib, "commute", "bike", "07:30", 30));
        assert_eq!(
            commute_bike.during.as_deref(),
            Some("Water. Counts toward the day's energy.")
        );
        let commute_transit = guide(&lib, w, &it(&lib, "commute", "transit", "07:30", 60));
        assert_eq!(commute_transit, Guide::default());

        let rest_off = guide(&lib, w, &it(&lib, "rest", "off", "20:00", 0));
        assert_eq!(
            rest_off.after.as_deref(),
            Some("Aim for 8+ hours of sleep; this is where the adaptation happens.")
        );

        let work_home = guide(&lib, w, &it(&lib, "work", "home", "09:00", 480));
        assert_eq!(
            work_home.during.as_deref(),
            Some("Keep a protein snack and water at hand; stand or walk between calls.")
        );
    }

    #[test]
    fn long_ride_guide() {
        let lib = default_library();
        let p = starter_plan("s");
        let g = guide(&lib, 73.0, &p.days[5][0]);
        assert_eq!(
            g.before.as_deref(),
            Some("Carb-rich meal 2–3 h before (about 110 g carbs), low fibre and fat.")
        );
        assert_eq!(
            g.during.as_deref(),
            Some(
                "60–90 g carbs per hour (about 300 g total), glucose–fructose mix, 300–600 mg sodium per hour. Rehearse race fuel."
            )
        );
        assert_eq!(
            g.after.as_deref(),
            Some("Within 1 h: 25–40 g protein plus about 73 g carbs.")
        );
        assert_eq!(g.note, None);
        assert!(fuel_on_the_go(&lib, &p.days[5][0]));
        assert_eq!(label(&lib, &p.days[5][0]), "Bike (long ride)");
    }

    #[test]
    fn other_kind_uses_endurance_branch() {
        let mut lib = default_library();
        let mut row = lib.get("bike").unwrap().clone();
        row.key = "row".into();
        row.label = "Row".into();
        row.kind = Kind::Other;
        lib.types.push(row);
        let g = guide(&lib, 73.0, &it(&lib, "row", "z2", "07:00", 90));
        assert_eq!(
            g.before.as_deref(),
            Some("Light carb snack 30–60 min before.")
        );
        assert_eq!(
            g.during.as_deref(),
            Some("40–60 g carbs per hour (about 75 g total).")
        );
        assert_eq!(g.after.as_deref(), Some("Normal next meal."));
        assert!(is_train(&lib, &it(&lib, "row", "z2", "07:00", 90)));
    }

    #[test]
    fn overlap_fires_and_clashes() {
        let lib = default_library();
        let a = it(&lib, "swim", "technique", "07:00", 45);
        let b = it(&lib, "run", "easy", "07:30", 30);
        let ids = [a.id.clone(), b.id.clone()];
        let mut days: [Vec<Item>; 7] = Default::default();
        days[1] = vec![b, a];
        let p = plan(days);
        assert!(
            checks(&lib, &p).contains(&"Tue: Swim (technique) overlaps Run (easy).".to_string())
        );
        assert_eq!(clashes(&p), HashSet::from(ids));
    }

    #[test]
    fn heavy_legs_within_48h() {
        let lib = default_library();
        let mut days: [Vec<Item>; 7] = Default::default();
        days[1] = vec![it(&lib, "gym", "heavy", "07:00", 55)];
        days[3] = vec![it(&lib, "bike", "intervals", "18:00", 60)];
        let w = checks(&lib, &plan(days));
        assert!(w.contains(&"Heavy legs on Tue is within 48 h of Bike (intervals) on Thu.".into()));
    }

    #[test]
    fn heavy_legs_wraps_week_sun_to_mon() {
        let lib = default_library();
        let mut days: [Vec<Item>; 7] = Default::default();
        days[6] = vec![it(&lib, "gym", "heavy", "20:00", 55)]; // Sun
        days[0] = vec![it(&lib, "run", "long", "07:00", 75)]; // Mon, next day
        let w = checks(&lib, &plan(days));
        assert!(w.contains(&"Heavy legs on Sun is within 48 h of Run (long run) on Mon.".into()));
    }

    #[test]
    fn checks_dedup_identical_messages() {
        let lib = default_library();
        let mut days: [Vec<Item>; 7] = Default::default();
        // Three identical, fully overlapping items produce the same "overlaps" text twice
        // (item0-item1, item1-item2); it must appear only once in the result.
        days[0] = vec![
            it(&lib, "swim", "technique", "07:00", 45),
            it(&lib, "swim", "technique", "07:00", 45),
            it(&lib, "swim", "technique", "07:00", 45),
        ];
        let w = checks(&lib, &plan(days));
        let msg = "Mon: Swim (technique) overlaps Swim (technique).";
        assert_eq!(w.iter().filter(|x| x.as_str() == msg).count(), 1);
    }

    #[test]
    fn run_before_long_ride() {
        let lib = default_library();
        let mut days: [Vec<Item>; 7] = Default::default();
        days[4] = vec![it(&lib, "run", "easy", "07:00", 35)];
        days[5] = vec![it(&lib, "bike", "z2", "09:00", 150)];
        let w = checks(&lib, &plan(days));
        assert!(w.contains(&"Run on Fri the day before the long ride on Sat.".into()));
    }

    #[test]
    fn padel_before_key_run() {
        let lib = default_library();
        let mut days: [Vec<Item>; 7] = Default::default();
        days[6] = vec![it(&lib, "padel", "match", "11:30", 90)];
        days[0] = vec![it(&lib, "run", "tempo", "07:00", 45)];
        let w = checks(&lib, &plan(days));
        assert!(w.contains(&"Competitive padel on Sun the day before Run (tempo) on Mon.".into()));
    }

    #[test]
    fn three_hard_days_and_no_rest_day() {
        let lib = default_library();
        let days: [Vec<Item>; 7] =
            std::array::from_fn(|_| vec![it(&lib, "swim", "threshold", "07:00", 50)]);
        let w = checks(&lib, &plan(days));
        for d in DAYS {
            assert!(w.contains(&format!("Three hard or long days in a row from {d}.")));
        }
        assert!(w.contains(
            &"No lighter day this week. Keep at least one real rest or recovery day.".into()
        ));
        assert!(w.contains(&"No running this week. Tendons need frequent, small doses.".into()));
        assert_eq!(w.len(), 9);
    }

    #[test]
    fn long_run_share() {
        let lib = default_library();
        let mut days: [Vec<Item>; 7] = Default::default();
        days[2] = vec![it(&lib, "run", "easy", "07:00", 30)];
        days[6] = vec![it(&lib, "run", "long", "09:00", 90)];
        let w = checks(&lib, &plan(days));
        assert!(w.contains(&"Long run is over half of weekly run time. Spread it out.".into()));
    }

    #[test]
    fn empty_week_has_no_checks() {
        assert!(checks(&default_library(), &Plan::new("e")).is_empty());
    }
}
