//! Google Health API v4 reads: latest body weight, completed workouts and heart rate.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Days, Local, NaiveDate, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_with::{DisplayFromStr, PickFirst, serde_as};

use crate::zones::HrSample;

const BASE: &str = "https://health.googleapis.com/v4/users/me/dataTypes";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workout {
    /// Google's `dataPointName`, or [`synthesized_id`] when Google gives none.
    pub id: String,
    /// Raw Google exercise type, e.g. `RUNNING`; see [`map_exercise_type`].
    pub exercise_type: String,
    /// The civil date the workout started on where it was recorded: the day it belongs to, even
    /// when this machine is in another time zone.
    pub day: NaiveDate,
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    /// Wall-clock minutes from start to end.
    pub minutes: u32,
    /// Minutes of movement, without pauses.
    pub active_min: Option<u32>,
    pub kcal: Option<f64>,
    pub avg_hr: Option<u16>,
    pub distance_m: Option<f64>,
    pub elevation_m: Option<f64>,
    pub pace_s_per_km: Option<f64>,
    pub swim_lengths: Option<u32>,
    /// Google's active zone minutes.
    pub azm: Option<u32>,
    /// Seconds in Google's light, moderate, vigorous and peak heart-rate bands.
    pub google_zones: Option<[u32; 4]>,
    pub notes: Option<String>,
    pub display_name: Option<String>,
}

/// Library kind key for a Google exercise type, if tapas tracks it.
#[must_use]
pub fn map_exercise_type(exercise_type: &str) -> Option<&'static str> {
    Some(match exercise_type {
        "SWIMMING" | "SWIMMING_POOL" | "SWIMMING_OPEN_WATER" => "swim",
        "CYCLING" | "BIKING" | "BIKING_STATIONARY" | "CYCLING_STATIONARY" | "MOUNTAIN_BIKING"
        | "SPINNING" => "bike",
        "RUNNING" | "RUNNING_TREADMILL" | "TRAIL_RUNNING" | "JOGGING" => "run",
        "STRENGTH_TRAINING"
        | "WEIGHTLIFTING"
        | "CIRCUIT_TRAINING"
        | "CROSSFIT"
        | "HIGH_INTENSITY_INTERVAL_TRAINING"
        | "CALISTHENICS" => "gym",
        "PADEL" => "padel",
        _ => return None,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Page<T> {
    #[serde(default = "Vec::new")]
    data_points: Vec<T>,
    #[serde(default)]
    next_page_token: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct WeightPoint {
    weight: Option<Weight>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Weight {
    sample_time: Option<SampleTime>,
    weight_grams: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SampleTime {
    physical_time: Option<DateTime<Utc>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct ExercisePoint {
    data_point_name: Option<String>,
    exercise: Option<Exercise>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Exercise {
    interval: Option<Interval>,
    #[expect(
        clippy::struct_field_names,
        reason = "mirrors the API's `exerciseType`"
    )]
    exercise_type: Option<String>,
    display_name: Option<String>,
    notes: Option<String>,
    active_duration: Option<String>,
    metrics_summary: Option<MetricsSummary>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[expect(clippy::struct_field_names, reason = "mirrors the API's names")]
struct Interval {
    start_time: Option<DateTime<Utc>>,
    end_time: Option<DateTime<Utc>>,
    civil_start_time: Option<CivilDateTime>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CivilDateTime {
    date: Option<CivilDate>,
}

#[derive(Debug, Deserialize)]
struct CivilDate {
    year: i32,
    month: u32,
    day: u32,
}

/// Google sends int64 fields as strings; `PickFirst` also takes plain numbers.
#[serde_as]
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct MetricsSummary {
    calories_kcal: Option<f64>,
    distance_millimeters: Option<f64>,
    #[serde_as(as = "Option<PickFirst<(_, DisplayFromStr)>>")]
    average_heart_rate_beats_per_minute: Option<u16>,
    elevation_gain_millimeters: Option<f64>,
    average_pace_seconds_per_meter: Option<f64>,
    average_speed_millimeters_per_second: Option<f64>,
    #[serde_as(as = "Option<PickFirst<(_, DisplayFromStr)>>")]
    active_zone_minutes: Option<u32>,
    total_swim_lengths: Option<f64>,
    heart_rate_zone_durations: Option<ZoneDurations>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[expect(clippy::struct_field_names, reason = "mirrors the API's names")]
struct ZoneDurations {
    light_time: Option<String>,
    moderate_time: Option<String>,
    vigorous_time: Option<String>,
    peak_time: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct HeartRatePoint {
    heart_rate: Option<HeartRate>,
}

#[serde_as]
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct HeartRate {
    sample_time: Option<SampleTime>,
    #[serde_as(as = "Option<PickFirst<(_, DisplayFromStr)>>")]
    beats_per_minute: Option<u16>,
}

/// How to read a data type's points.
#[derive(Clone, Copy)]
enum Read {
    /// Every point from every source (`dataPoints`).
    List,
    /// One point per real-world event, merged across sources (`dataPoints:reconcile`), so a
    /// ride synced by both Fitbit and Health Connect counts once.
    Reconcile,
}

impl Read {
    fn path(self) -> &'static str {
        match self {
            Read::List => "dataPoints",
            Read::Reconcile => "dataPoints:reconcile",
        }
    }
}

const WEIGHT_PAGE_SIZE: &str = "1000";
/// The API's maximum for exercise.
const EXERCISE_PAGE_SIZE: &str = "25";

/// All data points of `data_type` matching `filter`, following `nextPageToken` and parsing
/// each page with `parse`.
async fn fetch_all<T>(
    token: &str,
    data_type: &str,
    read: Read,
    filter: &str,
    page_size: &str,
    parse: fn(&str) -> Result<Parsed<T>>,
) -> Result<Vec<T>> {
    let client = reqwest::Client::new();
    let url = format!("{BASE}/{data_type}/{}", read.path());
    let mut out = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let mut query = vec![("filter", filter), ("pageSize", page_size)];
        if let Some(p) = &page_token {
            query.push(("pageToken", p));
        }
        let res = client
            .get(&url)
            .bearer_auth(token)
            .query(&query)
            .send()
            .await
            .with_context(|| format!("requesting Google Health {data_type}"))?;
        let status = res.status();
        let body = res.text().await.context("reading Google Health response")?;
        if !status.is_success() {
            bail!("Google Health {data_type} failed ({status}): {body}");
        }
        let (items, next) =
            parse(&body).with_context(|| format!("parsing Google Health {data_type}: {body}"))?;
        out.extend(items);
        page_token = next.filter(|t| !t.is_empty());
        if page_token.is_none() {
            return Ok(out);
        }
    }
}

/// One parsed response page: its items and the next page token.
pub type Parsed<T> = (Vec<T>, Option<String>);

/// One weight response page: `(kg, measured at)` per complete point, and the next page token.
pub fn parse_weights(body: &str) -> Result<Parsed<(f64, DateTime<Utc>)>> {
    let page: Page<WeightPoint> = serde_json::from_str(body)?;
    let weights = page
        .data_points
        .into_iter()
        .filter_map(|p| {
            let w = p.weight?;
            Some((w.weight_grams? / 1000.0, w.sample_time?.physical_time?))
        })
        .collect();
    Ok((weights, page.next_page_token))
}

/// Seconds of a protobuf JSON duration such as `3600s` or `3.5s`.
#[must_use]
pub fn parse_duration_secs(duration: &str) -> Option<f64> {
    duration.strip_suffix('s')?.parse().ok()
}

/// A quantity rounded to a whole number, clamped to `u32`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to u32"
)]
fn whole(x: f64) -> Option<u32> {
    x.is_finite()
        .then(|| x.round().clamp(0.0, f64::from(u32::MAX)) as u32)
}

fn whole_secs(duration: Option<&str>) -> Option<u32> {
    duration.and_then(parse_duration_secs).and_then(whole)
}

/// A stable id for a workout Google gives no `dataPointName`.
#[must_use]
pub fn synthesized_id(exercise_type: &str, start: DateTime<Utc>) -> String {
    format!(
        "{exercise_type}@{}",
        start.to_rfc3339_opts(SecondsFormat::AutoSi, true)
    )
}

fn workout(data_point_name: Option<String>, e: Exercise) -> Option<Workout> {
    let interval = e.interval?;
    let (start, end) = (interval.start_time?, interval.end_time?);
    let exercise_type = e.exercise_type.unwrap_or_default();
    let id = data_point_name
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| synthesized_id(&exercise_type, start));
    let (start, end) = (start.with_timezone(&Local), end.with_timezone(&Local));
    let day = interval
        .civil_start_time
        .and_then(|c| c.date)
        .and_then(|d| NaiveDate::from_ymd_opt(d.year, d.month, d.day))
        .unwrap_or_else(|| start.date_naive());
    let m = e.metrics_summary.unwrap_or_default();
    let pace_s_per_km = m.average_pace_seconds_per_meter.map(|p| p * 1000.0).or(m
        .average_speed_millimeters_per_second
        .filter(|&v| v > 0.0)
        .map(|v| 1_000_000.0 / v));
    let google_zones = m.heart_rate_zone_durations.map(|z| {
        [z.light_time, z.moderate_time, z.vigorous_time, z.peak_time]
            .map(|t| whole_secs(t.as_deref()).unwrap_or(0))
    });
    Some(Workout {
        id,
        exercise_type,
        day,
        start,
        end,
        minutes: u32::try_from((end - start).num_minutes()).unwrap_or(0),
        active_min: whole_secs(e.active_duration.as_deref()).map(|s| (s + 30) / 60),
        kcal: m.calories_kcal,
        avg_hr: m.average_heart_rate_beats_per_minute,
        distance_m: m.distance_millimeters.map(|mm| mm / 1000.0),
        elevation_m: m.elevation_gain_millimeters.map(|mm| mm / 1000.0),
        pace_s_per_km,
        swim_lengths: m.total_swim_lengths.and_then(whole),
        azm: m.active_zone_minutes,
        google_zones,
        notes: e.notes.filter(|n| !n.is_empty()),
        display_name: e.display_name.filter(|n| !n.is_empty()),
    })
}

/// One exercise response page: its complete points in page order, and the next page token.
pub fn parse_workouts(body: &str) -> Result<Parsed<Workout>> {
    let page: Page<ExercisePoint> = serde_json::from_str(body)?;
    let workouts = page
        .data_points
        .into_iter()
        .filter_map(|p| workout(p.data_point_name, p.exercise?))
        .collect();
    Ok((workouts, page.next_page_token))
}

/// One heart-rate response page: its complete samples in page order, and the next page token.
pub fn parse_heart_rate(body: &str) -> Result<Parsed<HrSample>> {
    let page: Page<HeartRatePoint> = serde_json::from_str(body)?;
    let samples = page
        .data_points
        .into_iter()
        .filter_map(|p| {
            let hr = p.heart_rate?;
            Some((hr.sample_time?.physical_time?, hr.beats_per_minute?))
        })
        .collect();
    Ok((samples, page.next_page_token))
}

/// Most recent weight in kg and when it was measured, looking back `since_days`.
pub async fn latest_weight_kg(
    token: &str,
    since_days: u32,
) -> Result<Option<(f64, DateTime<Utc>)>> {
    let since = Utc::now() - chrono::Duration::days(i64::from(since_days));
    let filter = format!(
        "weight.sample_time.physical_time >= \"{}\"",
        since.format("%Y-%m-%dT%H:%M:%SZ")
    );
    let weights = fetch_all(
        token,
        "weight",
        Read::List,
        &filter,
        WEIGHT_PAGE_SIZE,
        parse_weights,
    )
    .await?;
    Ok(weights.into_iter().max_by_key(|&(_, at)| at))
}

/// Workouts starting (civil time) on any day from `from` to `to`, both inclusive.
pub async fn workouts(token: &str, from: NaiveDate, to: NaiveDate) -> Result<Vec<Workout>> {
    let end = to + Days::new(1);
    let filter = format!(
        "exercise.interval.civil_start_time >= \"{from}T00:00:00\" AND \
         exercise.interval.civil_start_time < \"{end}T00:00:00\""
    );
    let mut out = fetch_all(
        token,
        "exercise",
        Read::Reconcile,
        &filter,
        EXERCISE_PAGE_SIZE,
        parse_workouts,
    )
    .await?;
    out.sort_by_key(|w| w.start);
    Ok(out)
}

/// The API's maximum for a `dataPoints` list.
const HEART_RATE_PAGE_SIZE: &str = "10000";

/// Heart-rate samples measured in `[from, to)`. The `heart_rate` token follows the proto field
/// naming of the exercise filter; the docs only show `weight`, so it awaits a live check.
fn heart_rate_filter(from: DateTime<Utc>, to: DateTime<Utc>) -> String {
    let at = |t: DateTime<Utc>| t.format("%Y-%m-%dT%H:%M:%SZ");
    format!(
        "heart_rate.sample_time.physical_time >= \"{}\" AND \
         heart_rate.sample_time.physical_time < \"{}\"",
        at(from),
        at(to)
    )
}

/// Heart-rate samples measured in `[from, to)`, sorted by time.
pub async fn heart_rate(
    token: &str,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<HrSample>> {
    let mut out = fetch_all(
        token,
        "heart-rate",
        Read::List,
        &heart_rate_filter(from, to),
        HEART_RATE_PAGE_SIZE,
        parse_heart_rate,
    )
    .await?;
    out.sort_by_key(|&(at, _)| at);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_weight_page_and_picks_latest() {
        let body = r#"{
          "dataPoints": [
            {"name": "a", "weight": {"sampleTime": {"physicalTime": "2026-09-01T06:30:00Z",
              "utcOffset": "7200s", "civilTime": {}}, "weightGrams": 72400}},
            {"name": "b", "weight": {"sampleTime": {"physicalTime": "2026-09-20T06:10:00.123Z"},
              "weightGrams": 71850.5}},
            {"name": "c", "weight": {"weightGrams": 70000}}
          ],
          "nextPageToken": "abc"
        }"#;
        let (weights, next) = parse_weights(body).unwrap();
        assert_eq!(next.as_deref(), Some("abc"));
        assert_eq!(weights.len(), 2);
        let &(kg, at) = weights.iter().max_by_key(|&&(_, at)| at).unwrap();
        assert!((kg - 71.8505).abs() < 1e-9);
        assert_eq!(
            at.date_naive(),
            NaiveDate::from_ymd_opt(2026, 9, 20).unwrap()
        );
    }

    #[test]
    fn parses_empty_page() {
        let (weights, next) = parse_weights("{}").unwrap();
        assert!(weights.is_empty() && next.is_none());
    }

    #[test]
    fn parses_reconciled_exercise_page() {
        let body = r#"{
          "dataPoints": [
            {"dataPointName": "users/me/dataTypes/exercise/dataPoints/abc123",
             "exercise": {
              "interval": {"startTime": "2026-09-20T22:30:00Z", "endTime": "2026-09-20T23:15:30Z",
                "startUtcOffset": "7200s", "endUtcOffset": "7200s",
                "civilStartTime": {"date": {"year": 2026, "month": 9, "day": 21},
                  "time": {"hours": 0, "minutes": 30}},
                "civilEndTime": {"date": {"year": 2026, "month": 9, "day": 21},
                  "time": {"hours": 1, "minutes": 15, "seconds": 30}}},
              "exerciseType": "RUNNING",
              "displayName": "Midnight run",
              "notes": "",
              "activeDuration": "2580.5s",
              "metricsSummary": {"caloriesKcal": 512.5, "distanceMillimeters": 8000000,
                "averageHeartRateBeatsPerMinute": "142", "elevationGainMillimeters": 45500,
                "averagePaceSecondsPerMeter": 0.3225, "averageSpeedMillimetersPerSecond": 3100,
                "activeZoneMinutes": "38", "steps": 7000,
                "heartRateZoneDurations": {"lightTime": "600s", "moderateTime": "1500.4s",
                  "vigorousTime": "480s"}}}},
            {"exercise": {
              "interval": {"startTime": "2026-09-22T06:00:00Z", "endTime": "2026-09-22T06:40:00Z",
                "civilStartTime": {}},
              "exerciseType": "SWIMMING",
              "notes": "400 easy, 8x100",
              "metricsSummary": {"averageHeartRateBeatsPerMinute": 131,
                "averageSpeedMillimetersPerSecond": 800, "totalSwimLengths": 64,
                "activeZoneMinutes": 20}}},
            {"dataPointName": "", "exercise": {
              "interval": {"startTime": "2026-09-23T07:00:00Z", "endTime": "2026-09-23T08:00:00Z"},
              "exerciseType": "STRENGTH_TRAINING"}},
            {"exercise": {"exerciseType": "YOGA"}}
          ],
          "nextPageToken": "next"
        }"#;
        let (w, next) = parse_workouts(body).unwrap();
        assert_eq!(next.as_deref(), Some("next"));
        assert_eq!(w.len(), 3);

        let run = &w[0];
        assert_eq!(run.id, "users/me/dataTypes/exercise/dataPoints/abc123");
        assert_eq!(run.day, NaiveDate::from_ymd_opt(2026, 9, 21).unwrap());
        assert_eq!((run.minutes, run.active_min), (45, Some(43)));
        assert_eq!(
            (run.kcal, run.avg_hr, run.azm),
            (Some(512.5), Some(142), Some(38))
        );
        assert_eq!(
            (run.distance_m, run.elevation_m),
            (Some(8000.0), Some(45.5))
        );
        assert!((run.pace_s_per_km.unwrap() - 322.5).abs() < 1e-9);
        assert_eq!(run.google_zones, Some([600, 1500, 480, 0]));
        assert_eq!(run.display_name.as_deref(), Some("Midnight run"));
        assert_eq!(run.notes, None);

        let swim = &w[1];
        assert_eq!(swim.id, "SWIMMING@2026-09-22T06:00:00Z");
        let start: DateTime<Utc> = "2026-09-22T06:00:00Z".parse().unwrap();
        assert_eq!(swim.day, start.with_timezone(&Local).date_naive());
        assert_eq!(
            (swim.avg_hr, swim.azm, swim.swim_lengths),
            (Some(131), Some(20), Some(64))
        );
        assert!((swim.pace_s_per_km.unwrap() - 1250.0).abs() < 1e-9);
        assert_eq!((swim.active_min, swim.google_zones), (None, None));
        assert_eq!(swim.notes.as_deref(), Some("400 easy, 8x100"));

        assert_eq!(w[2].id, "STRENGTH_TRAINING@2026-09-23T07:00:00Z");
        assert_eq!(
            (w[2].minutes, w[2].kcal, w[2].pace_s_per_km),
            (60, None, None)
        );
    }

    #[test]
    fn parses_heart_rate_page() {
        let body = r#"{
          "dataPoints": [
            {"heartRate": {"sampleTime": {"physicalTime": "2026-05-12T08:00:01Z",
              "utcOffset": "7200s"}, "beatsPerMinute": "72"}},
            {"heartRate": {"sampleTime": {"physicalTime": "2026-05-12T08:00:00.500Z"},
              "beatsPerMinute": 71}},
            {"heartRate": {"beatsPerMinute": "90"}},
            {"heartRate": {"sampleTime": {"physicalTime": "2026-05-12T08:00:02Z"}}}
          ],
          "nextPageToken": "p2"
        }"#;
        let (samples, next) = parse_heart_rate(body).unwrap();
        assert_eq!(next.as_deref(), Some("p2"));
        let at = |s: &str| s.parse::<DateTime<Utc>>().unwrap();
        assert_eq!(
            samples,
            [
                (at("2026-05-12T08:00:01Z"), 72),
                (at("2026-05-12T08:00:00.500Z"), 71)
            ]
        );
    }

    #[test]
    fn maps_exercise_types() {
        assert_eq!(map_exercise_type("RUNNING"), Some("run"));
        assert_eq!(map_exercise_type("CYCLING"), Some("bike"));
        assert_eq!(map_exercise_type("SWIMMING"), Some("swim"));
        assert_eq!(map_exercise_type("STRENGTH_TRAINING"), Some("gym"));
        assert_eq!(map_exercise_type("PADEL"), Some("padel"));
        assert_eq!(map_exercise_type("SKYDIVING"), None);
    }
}
