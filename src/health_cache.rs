//! Per-machine cache of what Google Health recorded in a week; never synced.

use std::fs;

use anyhow::{Context, Result};
use chrono::{DateTime, Days, Local, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    google::health::Workout, model::week_monday, storage, storage::Paths, zones::HrSample,
};

/// A past week is refetched until this long after its Sunday ended, so late watch and phone
/// syncs land.
const SETTLE_DAYS: u64 = 2;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WeekActuals {
    pub fetched_at: DateTime<Utc>,
    pub workouts: Vec<Workout>,
    /// Heart-rate samples inside the workouts' intervals, sorted by time.
    pub heart_rate: Vec<HrSample>,
}

/// Midnight starting `date` here; the first valid instant after a DST gap at midnight.
#[must_use]
pub fn local_midnight(date: NaiveDate) -> DateTime<Local> {
    let midnight = date.and_hms_opt(0, 0, 0).unwrap_or_default();
    Local
        .from_local_datetime(&midnight)
        .earliest()
        .or_else(|| {
            Local
                .from_local_datetime(&(midnight + chrono::Duration::hours(1)))
                .earliest()
        })
        .unwrap_or_else(|| Local.from_utc_datetime(&midnight))
}

/// The week from `monday`'s midnight to the next Monday's, here.
#[must_use]
pub fn week_span(monday: NaiveDate) -> (DateTime<Local>, DateTime<Local>) {
    (
        local_midnight(monday),
        local_midnight(monday + Days::new(7)),
    )
}

fn file(paths: &Paths, monday: NaiveDate) -> std::path::PathBuf {
    paths.health_dir().join(format!("{monday}.json"))
}

/// The cached week, `None` when missing or unreadable: a refetch replaces it.
#[must_use]
pub fn load(paths: &Paths, monday: NaiveDate) -> Option<WeekActuals> {
    let bytes = fs::read(file(paths, monday)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn save(paths: &Paths, monday: NaiveDate, actuals: &WeekActuals) -> Result<()> {
    let json = serde_json::to_vec_pretty(actuals).context("serializing the Health cache")?;
    storage::write_atomic(&file(paths, monday), &json)
}

/// Whether the week from `monday` should be fetched from Google Health at `now`: never for a
/// future week, always while it runs, and for a past week until a fetch made at least
/// [`SETTLE_DAYS`] after it ended.
#[must_use]
pub fn needs_fetch(actuals: Option<&WeekActuals>, monday: NaiveDate, now: DateTime<Local>) -> bool {
    if monday > week_monday(now.date_naive()) {
        return false;
    }
    let (_, week_end) = week_span(monday);
    if now < week_end {
        return true;
    }
    let settled = local_midnight(monday + Days::new(7 + SETTLE_DAYS));
    actuals.is_none_or(|a| a.fetched_at < settled)
}

/// The samples inside any workout's `[start, end)`; keeps the order of `samples`.
#[must_use]
pub fn samples_in_workouts(samples: Vec<HrSample>, workouts: &[Workout]) -> Vec<HrSample> {
    samples
        .into_iter()
        .filter(|&(at, _)| workouts.iter().any(|w| w.start <= at && at < w.end))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monday() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 21).unwrap()
    }

    fn at(date: NaiveDate, hour: u32) -> DateTime<Local> {
        local_midnight(date) + chrono::Duration::hours(hour.into())
    }

    fn fetched(at: DateTime<Local>) -> WeekActuals {
        WeekActuals {
            fetched_at: at.to_utc(),
            workouts: Vec::new(),
            heart_rate: Vec::new(),
        }
    }

    #[test]
    fn needs_fetch_by_week_and_fetch_time() {
        let m = monday();
        let next = m + Days::new(7);
        let sunday_evening = at(m + Days::new(6), 20);
        let running = fetched(at(m + Days::new(2), 9));

        // The running week: always, cached or not.
        assert!(needs_fetch(None, m, sunday_evening));
        assert!(needs_fetch(Some(&running), m, sunday_evening));

        // A past week: when missing, or fetched before it settled.
        let later = at(next + Days::new(10), 12);
        assert!(needs_fetch(None, m, later));
        assert!(needs_fetch(Some(&running), m, later));
        assert!(needs_fetch(Some(&fetched(at(next, 8))), m, later));
        let almost = local_midnight(next + Days::new(2)) - chrono::Duration::seconds(1);
        assert!(needs_fetch(Some(&fetched(almost)), m, later));
        assert!(!needs_fetch(
            Some(&fetched(local_midnight(next + Days::new(2)))),
            m,
            later
        ));
        assert!(!needs_fetch(
            Some(&fetched(at(next + Days::new(5), 7))),
            m,
            later
        ));

        // A future week: never.
        assert!(!needs_fetch(None, next, sunday_evening));
        assert!(!needs_fetch(None, next + Days::new(7), at(m, 0)));
    }

    #[test]
    fn keeps_samples_inside_workouts() {
        let utc = |h: u32, m: u32| Utc.with_ymd_and_hms(2026, 9, 21, h, m, 0).unwrap();
        let w = |start: u32, end: u32| Workout {
            id: format!("run-{start}"),
            exercise_type: "RUNNING".into(),
            day: monday(),
            start: utc(start, 0).with_timezone(&Local),
            end: utc(end, 0).with_timezone(&Local),
            minutes: 60,
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
        };
        let workouts = [w(7, 8), w(18, 19)];
        let s = |h: u32, m: u32| (utc(h, m), 100);
        let samples = vec![s(6, 59), s(7, 0), s(7, 30), s(8, 0), s(12, 0), s(18, 59)];
        assert_eq!(
            samples_in_workouts(samples, &workouts),
            [s(7, 0), s(7, 30), s(18, 59)]
        );
    }
}
