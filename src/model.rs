//! Persistent data: the session library, plan templates, dated weeks, profile and settings.

use chrono::{DateTime, Days, NaiveDate, NaiveTime, TimeDelta, Timelike, Utc, Weekday};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::zones::HrZones;

pub const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// chrono format of start times, in the store and on screen.
pub const TIME_FMT: &str = "%H:%M";

/// chrono format of a date in messages: `Mon 21 Sep`.
pub const DAY_FMT: &str = "%a %-d %b";

/// Serde `with` module storing a `NaiveTime` as `"HH:MM"`.
pub mod hhmm {
    use super::TIME_FMT;
    use chrono::NaiveTime;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(t: &NaiveTime, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&t.format(TIME_FMT))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<NaiveTime, D::Error> {
        let s = String::deserialize(d)?;
        NaiveTime::parse_from_str(s.trim(), TIME_FMT).map_err(D::Error::custom)
    }
}

/// Which rule set the fuelling guidance and checks apply to a session type.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Swim,
    Bike,
    Run,
    Gym,
    Brick,
    Padel,
    Work,
    Commute,
    Rest,
    Other,
}

impl Kind {
    pub const ALL: [Kind; 10] = [
        Kind::Swim,
        Kind::Bike,
        Kind::Run,
        Kind::Gym,
        Kind::Brick,
        Kind::Padel,
        Kind::Work,
        Kind::Commute,
        Kind::Rest,
        Kind::Other,
    ];
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Kind::Swim => "swim",
            Kind::Bike => "bike",
            Kind::Run => "run",
            Kind::Gym => "gym",
            Kind::Brick => "brick",
            Kind::Padel => "padel",
            Kind::Work => "work",
            Kind::Commute => "commute",
            Kind::Rest => "rest",
            Kind::Other => "other",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Train,
    Life,
    Rest,
}

impl Category {
    pub const ALL: [Category; 3] = [Category::Train, Category::Life, Category::Rest];
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Category::Train => "train",
            Category::Life => "life",
            Category::Rest => "rest",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Effort {
    pub key: String,
    pub label: String,
    pub rpe: f64,
    pub met: f64,
    /// Default duration in minutes.
    pub dur: u32,
    /// Heavy leg work: keep 48 h away from key run/bike sessions.
    pub legs: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SessionType {
    pub key: String,
    pub label: String,
    /// `#RRGGBB`
    pub color: String,
    pub kind: Kind,
    pub category: Category,
    /// Default start.
    #[serde(with = "hhmm")]
    pub start: NaiveTime,
    /// Counts toward the weekly training hours.
    pub counted: bool,
    pub efforts: Vec<Effort>,
    /// Last change, stamped by `storage::save`; the merge's last-writer-wins clock.
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Library {
    pub types: Vec<SessionType>,
}

impl Library {
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&SessionType> {
        self.types.iter().find(|t| t.key == key)
    }

    /// Resolve an item's type and effort; an unknown effort falls back to the type's first one.
    #[must_use]
    pub fn resolve(&self, it: &Item) -> Option<(&SessionType, &Effort)> {
        let t = self.get(&it.type_key)?;
        let e = t
            .efforts
            .iter()
            .find(|e| e.key == it.effort)
            .or_else(|| t.efforts.first())?;
        Some((t, e))
    }

    /// `Swim: Technique`
    #[must_use]
    pub fn title(&self, it: &Item) -> String {
        match self.resolve(it) {
            Some((t, e)) => format!("{}: {}", t.label, e.label),
            None => format!("{}: {}", it.type_key, it.effort),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    #[serde(rename = "type")]
    pub type_key: String,
    pub effort: String,
    #[serde(with = "hhmm")]
    pub start: NaiveTime,
    /// Minutes; 0 means an all-day marker.
    pub dur: u32,
    pub notes: String,
    /// Google Health workout id this item was done as, linked by hand.
    pub workout: Option<String>,
    /// Last change, stamped by `storage::save`; the merge's last-writer-wins clock.
    pub updated_at: DateTime<Utc>,
}

impl Item {
    /// Start as minutes after midnight.
    #[must_use]
    pub fn start_min(&self) -> u32 {
        self.start.num_seconds_from_midnight() / 60
    }
    /// End time of day, wrapping past midnight.
    #[must_use]
    pub fn end(&self) -> NaiveTime {
        let (t, _) = self
            .start
            .overflowing_add_signed(TimeDelta::minutes(self.dur.into()));
        t
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Plan {
    pub id: String,
    pub name: String,
    pub days: [Vec<Item>; 7],
    /// Last change of the plan itself (its name); items carry their own.
    pub updated_at: DateTime<Utc>,
}

impl Plan {
    pub fn new(name: impl Into<String>) -> Self {
        Plan {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            days: Default::default(),
            updated_at: DateTime::UNIX_EPOCH,
        }
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.days.iter().all(std::vec::Vec::is_empty)
    }
    #[must_use]
    pub fn find(&self, id: &str) -> Option<(usize, usize)> {
        self.days
            .iter()
            .enumerate()
            .find_map(|(d, items)| items.iter().position(|x| x.id == id).map(|k| (d, k)))
    }
    /// Items of a day ordered by start time.
    #[must_use]
    pub fn sorted_day(&self, d: usize) -> Vec<&Item> {
        let mut v: Vec<&Item> = self.days[d].iter().collect();
        v.sort_by_key(|x| x.start);
        v
    }
    #[must_use]
    pub fn uses_type(&self, key: &str) -> bool {
        self.days.iter().flatten().any(|x| x.type_key == key)
    }
}

/// Monday of the week containing `d`.
#[must_use]
pub fn week_monday(d: NaiveDate) -> NaiveDate {
    d.week(Weekday::Mon).first_day()
}

/// `count` consecutive Mondays from the week containing `from`.
pub fn mondays(from: NaiveDate, count: u32) -> impl Iterator<Item = NaiveDate> {
    let first = week_monday(from);
    (0..count).map(move |i| first + Days::new(7 * u64::from(i)))
}

/// A dated week: its own copy of a template's items, edited without touching the template.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Week {
    pub monday: NaiveDate,
    /// `id` is the Monday (`2026-09-21`); `name` is the template it was last applied from,
    /// empty when the week was started by hand.
    pub plan: Plan,
}

/// `Week of Mon 21 Sep 2026 · from "Build 2"`; `template` is the name it was applied from.
#[must_use]
pub fn week_title(monday: NaiveDate, template: &str) -> String {
    let week = format!("Week of {}", monday.format("%a %-d %b %Y"));
    if template.is_empty() {
        week
    } else {
        format!("{week} · from \"{template}\"")
    }
}

/// Namespace of the ids of items applied to the week of `monday`.
fn week_namespace(monday: NaiveDate) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("tapas-week:{monday}").as_bytes(),
    )
}

impl Week {
    #[must_use]
    pub fn new(monday: NaiveDate) -> Week {
        Week {
            monday,
            plan: Plan {
                id: monday.to_string(),
                name: String::new(),
                days: Default::default(),
                updated_at: DateTime::UNIX_EPOCH,
            },
        }
    }

    /// The week of `monday` holding `template`'s items. Item ids derive from the Monday and the
    /// template item's id, so every machine applying the same template gets the same items.
    #[must_use]
    pub fn apply(template: &Plan, monday: NaiveDate) -> Week {
        let ns = week_namespace(monday);
        let mut week = Week::new(monday);
        template.name.clone_into(&mut week.plan.name);
        week.plan.days = template.days.clone().map(|day| {
            day.into_iter()
                .map(|it| Item {
                    id: Uuid::new_v5(&ns, it.id.as_bytes()).to_string(),
                    ..it
                })
                .collect()
        });
        week
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Base {
    pub kcal: f64,
    #[serde(rename = "P")]
    pub p: f64,
    #[serde(rename = "C")]
    pub c: f64,
    #[serde(rename = "F")]
    pub f: f64,
}

impl Default for Base {
    fn default() -> Self {
        Base {
            kcal: 2600.0,
            p: 150.0,
            c: 315.0,
            f: 80.0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Profile {
    pub weight: f64,
    pub base: Base,
    pub hr: HrZones,
    /// Last change, stamped by `storage::save`; the merge's last-writer-wins clock.
    pub updated_at: DateTime<Utc>,
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            weight: 73.0,
            base: Base::default(),
            hr: HrZones::default(),
            updated_at: DateTime::UNIX_EPOCH,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ExportSettings {
    pub include_life: bool,
    pub weeks: u32,
    pub calendar_name: String,
    /// Google Calendar id of the calendar this app created, once it exists.
    pub calendar_id: Option<String>,
    /// Last change, stamped by `storage::save`; the merge's last-writer-wins clock.
    pub updated_at: DateTime<Utc>,
}

impl Default for ExportSettings {
    fn default() -> Self {
        ExportSettings {
            include_life: false,
            weeks: 1,
            calendar_name: "Training".into(),
            calendar_id: None,
            updated_at: DateTime::UNIX_EPOCH,
        }
    }
}

/// Schema version of [`Store`] this binary reads and writes.
pub const STORE_VERSION: u32 = 4;

/// The synced document. Per-machine state lives in [`Device`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Store {
    pub version: u32,
    pub profile: Profile,
    pub library: Library,
    /// Undated templates; there is always at least one.
    pub plans: Vec<Plan>,
    /// Sorted by `monday`.
    pub weeks: Vec<Week>,
    pub export: ExportSettings,
}

impl Default for Store {
    fn default() -> Self {
        Store {
            version: STORE_VERSION,
            profile: Profile::default(),
            library: crate::library::default_library(),
            plans: vec![Plan::new("Week 1")],
            weeks: Vec::new(),
            export: ExportSettings::default(),
        }
    }
}

impl Store {
    /// Repair invariants after loading or editing.
    pub fn normalize(&mut self) {
        if self.plans.is_empty() {
            self.plans.push(Plan::new("Week 1"));
        }
        self.weeks.sort_by_key(|w| w.monday);
        self.export.weeks = self.export.weeks.clamp(1, 52);
    }
    /// The template named `name` (any case) or with id `name`.
    #[must_use]
    pub fn plan_by_name(&self, name: &str) -> Option<&Plan> {
        self.plans
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name) || p.id == name)
    }
    /// Templates, then the plans of dated weeks.
    pub fn every_plan(&self) -> impl Iterator<Item = &Plan> {
        self.plans.iter().chain(self.weeks.iter().map(|w| &w.plan))
    }
    #[must_use]
    pub fn week(&self, monday: NaiveDate) -> Option<&Week> {
        let i = self
            .weeks
            .binary_search_by_key(&monday, |w| w.monday)
            .ok()?;
        Some(&self.weeks[i])
    }
    /// The stored week of `monday`, inserted empty in Monday order when there is none.
    pub fn week_mut_or_insert(&mut self, monday: NaiveDate) -> &mut Week {
        let i = match self.weeks.binary_search_by_key(&monday, |w| w.monday) {
            Ok(i) => i,
            Err(i) => {
                self.weeks.insert(i, Week::new(monday));
                i
            }
        };
        &mut self.weeks[i]
    }
    /// Mondays of the `count` weeks from the week containing `from` whose stored week has
    /// items.
    #[must_use]
    pub fn filled_weeks(&self, from: NaiveDate, count: u32) -> Vec<NaiveDate> {
        mondays(from, count)
            .filter(|&m| self.week(m).is_some_and(|w| !w.plan.is_empty()))
            .collect()
    }
    /// Replace the items of the `count` weeks from the week containing `from` with copies of
    /// template `template_id`'s. False when there is no such template.
    #[must_use]
    pub fn apply_template(&mut self, template_id: &str, from: NaiveDate, count: u32) -> bool {
        let Some(template) = self.plans.iter().find(|p| p.id == template_id).cloned() else {
            return false;
        };
        for monday in mondays(from, count) {
            let applied = Week::apply(&template, monday).plan;
            let week = self.week_mut_or_insert(monday);
            week.plan.name = applied.name;
            week.plan.days = applied.days;
        }
        true
    }
}

/// State of this machine only, kept next to the store and never synced.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Device {
    pub device_id: Uuid,
}

impl Default for Device {
    fn default() -> Self {
        Device {
            device_id: Uuid::new_v4(),
        }
    }
}

/// `1h05`, `2h`, `45m`
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "session minutes are small and non-negative"
)]
pub fn hm(min: f64) -> String {
    let h = (min / 60.0).floor() as u32;
    let r = (min % 60.0).round() as u32;
    match (h, r) {
        (0, r) => format!("{r}m"),
        (h, 0) => format!("{h}h"),
        (h, r) => format!("{h}h{r:02}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::starter_plan;

    fn day(m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, m, d).unwrap()
    }

    fn item_ids(p: &Plan) -> Vec<&str> {
        p.days.iter().flatten().map(|x| x.id.as_str()).collect()
    }

    #[test]
    fn applying_a_template_gives_ids_fixed_by_the_monday() {
        let t = starter_plan("Build 2");
        let a = Week::apply(&t, day(9, 21));
        assert_eq!(a, Week::apply(&t, day(9, 21)));
        assert_eq!(a.plan.id, "2026-09-21");
        assert_eq!(a.plan.name, "Build 2");
        let next = Week::apply(&t, day(9, 28));
        for ((x, y), z) in item_ids(&a.plan)
            .into_iter()
            .zip(item_ids(&next.plan))
            .zip(item_ids(&t))
        {
            assert_ne!(x, y);
            assert_ne!(x, z);
        }
        for d in 0..7 {
            assert_eq!(a.plan.days[d].len(), t.days[d].len());
            for (applied, template) in a.plan.days[d].iter().zip(&t.days[d]) {
                let same_id = Item {
                    id: template.id.clone(),
                    ..applied.clone()
                };
                assert_eq!(&same_id, template);
            }
        }
    }

    #[test]
    fn apply_template_fills_consecutive_weeks_and_replaces_their_items() {
        let mut s = Store {
            plans: vec![starter_plan("Base"), Plan::new("Taper")],
            ..Store::default()
        };
        let (base, taper) = (s.plans[0].id.clone(), s.plans[1].id.clone());
        s.week_mut_or_insert(day(10, 12)).plan.days[0] = s.plans[0].days[0].clone();
        s.week_mut_or_insert(day(9, 14));
        assert_eq!(s.filled_weeks(day(9, 30), 3), [day(10, 12)]);

        // Any day snaps to its Monday.
        assert!(s.apply_template(&base, day(9, 30), 3));
        let mondays: Vec<_> = s.weeks.iter().map(|w| w.monday).collect();
        assert_eq!(mondays, [day(9, 14), day(9, 28), day(10, 5), day(10, 12)]);
        for m in &mondays[1..] {
            assert_eq!(s.week(*m).unwrap(), &Week::apply(&s.plans[0], *m));
        }

        assert!(s.apply_template(&taper, day(10, 12), 1));
        let w = s.week(day(10, 12)).unwrap();
        assert!(w.plan.is_empty());
        assert_eq!(w.plan.name, "Taper");
        assert!(!s.apply_template("gone", day(10, 12), 1));
    }
}
