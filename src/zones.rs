//! Heart-rate zones, time in zone and training load.

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use crate::google::health::Workout;

/// Five heart-rate zones bounded by four upper limits.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct HrZones {
    pub max: u16,
    pub resting: u16,
    pub lthr: Option<u16>,
    /// `upper[i]` is the highest bpm of zone `i + 1`; anything above `upper[3]` is zone 5.
    pub upper: [u16; 4],
}

const FRIEL_LTHR_PERCENT: [u32; 4] = [85, 89, 94, 99];
const MAX_HR_PERCENT: [u32; 4] = [60, 70, 80, 90];

/// Zone upper limits from Friel's LTHR percentages when `lthr` is known, else from `max`.
#[must_use]
pub fn default_upper(max: u16, lthr: Option<u16>) -> [u16; 4] {
    let (base, percents) = match lthr {
        Some(lthr) => (lthr, FRIEL_LTHR_PERCENT),
        None => (max, MAX_HR_PERCENT),
    };
    percents.map(|percent| {
        let rounded = (u32::from(base) * percent + 50) / 100;
        u16::try_from(rounded).unwrap_or(u16::MAX)
    })
}

impl HrZones {
    #[must_use]
    pub fn new(max: u16, resting: u16, lthr: Option<u16>) -> Self {
        Self {
            max,
            resting,
            lthr,
            upper: default_upper(max, lthr),
        }
    }

    /// Zone number 1..=5 of `bpm`.
    #[must_use]
    pub fn zone(&self, bpm: u16) -> usize {
        1 + self.upper.iter().take_while(|&&upper| bpm > upper).count()
    }
}

impl Default for HrZones {
    fn default() -> Self {
        Self::new(185, 50, None)
    }
}

pub type HrSample = (DateTime<Utc>, u16);

/// Longer silences between samples are treated as the sensor dropping out.
pub const MAX_SAMPLE_GAP: TimeDelta = TimeDelta::seconds(30);

/// Seconds spent in each zone during `[start, end)`; `samples` must be sorted by time.
///
/// Each sample holds until the next one, and the last one until `end`, unless that span
/// exceeds [`MAX_SAMPLE_GAP`], in which case it counts nothing.
#[must_use]
pub fn time_in_zones(
    samples: &[HrSample],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    zones: &HrZones,
) -> [u32; 5] {
    let first = samples.partition_point(|&(at, _)| at < start);
    let past_end = samples.partition_point(|&(at, _)| at < end).max(first);
    let in_window = &samples[first..past_end];

    let mut millis = [0_i64; 5];
    for (i, &(at, bpm)) in in_window.iter().enumerate() {
        let held_until = in_window.get(i + 1).map_or(end, |&(next, _)| next);
        let held = held_until - at;
        if held > TimeDelta::zero() && held <= MAX_SAMPLE_GAP {
            millis[zones.zone(bpm) - 1] += held.num_milliseconds();
        }
    }
    millis.map(|ms| u32::try_from((ms + 500) / 1000).unwrap_or(u32::MAX))
}

/// Edwards TRIMP: minutes in each zone weighted by the zone number.
#[must_use]
pub fn trimp(zone_seconds: &[u32; 5]) -> f64 {
    weighted_minutes(zone_seconds)
}

fn weighted_minutes(seconds: &[u32]) -> f64 {
    seconds
        .iter()
        .zip(1_u32..)
        .map(|(&secs, weight)| f64::from(secs) / 60.0 * f64::from(weight))
        .sum()
}

/// Time in zone, from the user's own zones or, lacking HR samples, Google's four bands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ZoneTime {
    Own([u32; 5]),
    /// Seconds in Google Health's light, moderate, vigorous and peak bands.
    Google([u32; 4]),
}

impl ZoneTime {
    /// Edwards TRIMP for own zones; for Google's bands an approximation weighting them 1..=4.
    #[must_use]
    pub fn trimp(&self) -> f64 {
        weighted_minutes(self.seconds())
    }

    /// Index of the band with the most time (the first on a tie), `None` when all are zero.
    #[must_use]
    pub fn dominant(&self) -> Option<usize> {
        let seconds = self.seconds();
        let most = *seconds.iter().max().filter(|&&most| most > 0)?;
        seconds.iter().position(|&s| s == most)
    }

    #[must_use]
    pub fn labels(&self) -> &'static [&'static str] {
        match self {
            Self::Own(_) => &["Z1", "Z2", "Z3", "Z4", "Z5"],
            Self::Google(_) => &["light", "moderate", "vigorous", "peak"],
        }
    }

    #[must_use]
    pub fn seconds(&self) -> &[u32] {
        match self {
            Self::Own(seconds) => seconds,
            Self::Google(seconds) => seconds,
        }
    }

    #[must_use]
    pub fn is_google(&self) -> bool {
        matches!(self, Self::Google(_))
    }
}

/// Metabolic equivalent: kcal per kg of body weight per hour.
#[must_use]
pub fn met(kcal: f64, weight_kg: f64, minutes: u32) -> Option<f64> {
    (minutes > 0 && weight_kg > 0.0).then(|| kcal / weight_kg / (f64::from(minutes) / 60.0))
}

/// Time in `zones` from `samples` over the workout, else Google's bands, else `None`.
#[must_use]
pub fn workout_zones(workout: &Workout, samples: &[HrSample], zones: &HrZones) -> Option<ZoneTime> {
    let own = time_in_zones(
        samples,
        workout.start.with_timezone(&Utc),
        workout.end.with_timezone(&Utc),
        zones,
    );
    if own.iter().any(|&secs| secs > 0) {
        Some(ZoneTime::Own(own))
    } else {
        workout.google_zones.map(ZoneTime::Google)
    }
}

/// How hard a recorded workout was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Effort {
    pub zones: Option<ZoneTime>,
    pub trimp: Option<f64>,
    pub met: Option<f64>,
}

#[must_use]
pub fn workout_effort(
    workout: &Workout,
    samples: &[HrSample],
    zones: &HrZones,
    weight_kg: f64,
) -> Effort {
    let zones = workout_zones(workout, samples, zones);
    Effort {
        zones,
        trimp: zones.map(|time| time.trimp()),
        met: workout
            .kcal
            .and_then(|kcal| met(kcal, weight_kg, workout.minutes)),
    }
}

/// Training load of a week: the sum of its workouts' TRIMP.
#[must_use]
pub fn week_trimp<'a>(efforts: impl IntoIterator<Item = &'a Effort>) -> f64 {
    efforts.into_iter().filter_map(|effort| effort.trimp).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};

    fn t0() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 26, 7, 0, 0).unwrap()
    }

    fn at(secs: i64) -> DateTime<Utc> {
        t0() + TimeDelta::seconds(secs)
    }

    fn series(from: i64, bpms: &[u16]) -> Vec<HrSample> {
        (from..).zip(bpms).map(|(s, &bpm)| (at(s), bpm)).collect()
    }

    #[test]
    fn default_upper_from_lthr_uses_friel_percentages() {
        assert_eq!(default_upper(185, Some(170)), [145, 151, 160, 168]);
    }

    #[test]
    fn default_upper_from_max_hr_rounds_to_nearest() {
        assert_eq!(default_upper(185, None), [111, 130, 148, 167]);
        assert_eq!(HrZones::default().upper, [111, 130, 148, 167]);
    }

    #[test]
    fn zone_boundaries_belong_to_the_lower_zone() {
        let zones = HrZones::default();
        assert_eq!(zones.zone(0), 1);
        assert_eq!(zones.zone(111), 1);
        assert_eq!(zones.zone(112), 2);
        assert_eq!(zones.zone(130), 2);
        assert_eq!(zones.zone(131), 3);
        assert_eq!(zones.zone(148), 3);
        assert_eq!(zones.zone(167), 4);
        assert_eq!(zones.zone(168), 5);
        assert_eq!(zones.zone(u16::MAX), 5);
    }

    #[test]
    fn regular_series_counts_each_second() {
        let zones = HrZones::default();
        let samples = series(0, &[100, 100, 120, 140, 160, 180]);
        assert_eq!(
            time_in_zones(&samples, at(0), at(6), &zones),
            [2, 1, 1, 1, 1]
        );
    }

    #[test]
    fn gap_over_max_drops_the_sample_before_it() {
        let zones = HrZones::default();
        let samples = vec![(at(0), 100), (at(31), 180), (at(61), 140)];
        assert_eq!(
            time_in_zones(&samples, at(0), at(62), &zones),
            [0, 0, 1, 0, 30]
        );
    }

    #[test]
    fn samples_outside_the_window_do_not_count() {
        let zones = HrZones::default();
        let samples = series(-3, &[180, 180, 180, 100, 100, 180, 180]);
        assert_eq!(
            time_in_zones(&samples, at(0), at(2), &zones),
            [2, 0, 0, 0, 0]
        );
    }

    #[test]
    fn last_sample_holds_until_end_only_within_max_gap() {
        let zones = HrZones::default();
        let samples = vec![(at(0), 100)];
        assert_eq!(
            time_in_zones(&samples, at(0), at(30), &zones),
            [30, 0, 0, 0, 0]
        );
        assert_eq!(time_in_zones(&samples, at(0), at(31), &zones), [0; 5]);
    }

    #[test]
    fn empty_series_has_no_time() {
        assert_eq!(
            time_in_zones(&[], at(0), at(60), &HrZones::default()),
            [0; 5]
        );
    }

    #[test]
    fn trimp_weights_minutes_by_zone() {
        assert!((trimp(&[600, 1200, 300, 120, 60]) - 78.0).abs() < 1e-9);
    }

    #[test]
    fn google_fallback_trimp_weights_bands_one_to_four() {
        let time = ZoneTime::Google([600, 1200, 300, 60]);
        assert!((time.trimp() - 69.0).abs() < 1e-9);
        assert!(time.is_google());
    }

    #[test]
    fn dominant_picks_the_band_with_most_time() {
        assert_eq!(ZoneTime::Google([600, 1200, 300, 60]).dominant(), Some(1));
        assert_eq!(ZoneTime::Own([0, 0, 5, 0, 5]).dominant(), Some(2));
        assert_eq!(ZoneTime::Google([0; 4]).dominant(), None);
    }

    #[test]
    fn met_needs_minutes_and_weight() {
        assert_eq!(met(700.0, 70.0, 60), Some(10.0));
        assert_eq!(met(700.0, 70.0, 0), None);
        assert_eq!(met(700.0, 0.0, 60), None);
    }

    fn workout(google_zones: Option<[u32; 4]>) -> Workout {
        let start = t0().with_timezone(&Local);
        Workout {
            id: "run".into(),
            exercise_type: "RUNNING".into(),
            day: start.date_naive(),
            start,
            end: start + TimeDelta::seconds(4),
            minutes: 0,
            active_min: None,
            kcal: None,
            avg_hr: None,
            distance_m: None,
            elevation_m: None,
            pace_s_per_km: None,
            swim_lengths: None,
            azm: None,
            google_zones,
            notes: None,
            display_name: None,
        }
    }

    #[test]
    fn workout_zones_prefers_own_samples_over_google_bands() {
        let zones = HrZones::default();
        let samples = series(-2, &[180, 180, 100, 120, 140, 160, 180, 180]);
        assert_eq!(
            workout_zones(&workout(Some([60, 0, 0, 0])), &samples, &zones),
            Some(ZoneTime::Own([1, 1, 1, 1, 0]))
        );
    }

    #[test]
    fn workout_zones_falls_back_to_google_bands_without_samples_inside() {
        let zones = HrZones::default();
        let before = series(-40, &[150, 150]);
        assert_eq!(
            workout_zones(&workout(Some([60, 30, 0, 0])), &before, &zones),
            Some(ZoneTime::Google([60, 30, 0, 0]))
        );
        assert_eq!(workout_zones(&workout(None), &before, &zones), None);
    }
}
