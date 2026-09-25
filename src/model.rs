//! Persistent data: the session library, plans (named weeks), profile and settings.

use chrono::{NaiveTime, TimeDelta, Timelike};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// chrono format of start times, in the store and on screen.
pub const TIME_FMT: &str = "%H:%M";

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
    #[serde(default)]
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
    #[serde(default)]
    pub notes: String,
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
}

impl Plan {
    pub fn new(name: impl Into<String>) -> Self {
        Plan {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            days: Default::default(),
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
    #[serde(default)]
    pub base: Base,
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            weight: 73.0,
            base: Base::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ExportSettings {
    pub include_life: bool,
    pub weeks: u32,
    pub calendar_name: String,
    /// Google Calendar id of the calendar this app created, once it exists.
    #[serde(default)]
    pub calendar_id: Option<String>,
}

impl Default for ExportSettings {
    fn default() -> Self {
        ExportSettings {
            include_life: false,
            weeks: 1,
            calendar_name: "Training".into(),
            calendar_id: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Store {
    pub version: u32,
    pub profile: Profile,
    pub library: Library,
    pub plans: Vec<Plan>,
    pub active: usize,
    #[serde(default)]
    pub export: ExportSettings,
}

impl Default for Store {
    fn default() -> Self {
        Store {
            version: 1,
            profile: Profile::default(),
            library: crate::library::default_library(),
            plans: vec![Plan::new("Week 1")],
            active: 0,
            export: ExportSettings::default(),
        }
    }
}

impl Store {
    #[must_use]
    pub fn plan(&self) -> &Plan {
        &self.plans[self.active.min(self.plans.len() - 1)]
    }
    pub fn plan_mut(&mut self) -> &mut Plan {
        let i = self.active.min(self.plans.len() - 1);
        &mut self.plans[i]
    }
    /// Repair invariants after loading or editing.
    pub fn normalize(&mut self) {
        if self.plans.is_empty() {
            self.plans.push(Plan::new("Week 1"));
        }
        self.active = self.active.min(self.plans.len() - 1);
        self.export.weeks = self.export.weeks.clamp(1, 52);
    }
    #[must_use]
    pub fn plan_by_name(&self, name: &str) -> Option<&Plan> {
        self.plans
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name) || p.id == name)
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
