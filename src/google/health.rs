//! Google Health API v4 reads: latest body weight and completed workouts.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Days, Local, NaiveDate, Utc};
use serde::Deserialize;

const BASE: &str = "https://health.googleapis.com/v4/users/me/dataTypes";

#[derive(Debug, Clone, PartialEq)]
pub struct Workout {
    /// Raw Google exercise type, e.g. `RUNNING`; see [`map_exercise_type`].
    pub exercise_type: String,
    pub start: DateTime<Local>,
    pub minutes: u32,
    pub kcal: Option<f64>,
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
    metrics_summary: Option<MetricsSummary>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Interval {
    start_time: Option<DateTime<Utc>>,
    end_time: Option<DateTime<Utc>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct MetricsSummary {
    calories_kcal: Option<f64>,
}

/// How to read a data type's points.
#[derive(Clone, Copy)]
enum Read {
    /// Every point from every source (`dataPoints`).
    List,
    /// One point per real-world event, merged across sources (`dataPoints:reconcile`), so a
    /// ride synced by both Fitbit and Health Connect counts once. Exercise pages hold at most 25.
    Reconcile,
}

impl Read {
    fn path(self) -> &'static str {
        match self {
            Read::List => "dataPoints",
            Read::Reconcile => "dataPoints:reconcile",
        }
    }
    fn page_size(self) -> &'static str {
        match self {
            Read::List => "1000",
            Read::Reconcile => "25",
        }
    }
}

/// All data points of `data_type` matching `filter`, following `nextPageToken` and parsing
/// each page with `parse`.
async fn fetch_all<T>(
    token: &str,
    data_type: &str,
    read: Read,
    filter: &str,
    parse: fn(&str) -> Result<Parsed<T>>,
) -> Result<Vec<T>> {
    let client = reqwest::Client::new();
    let url = format!("{BASE}/{data_type}/{}", read.path());
    let mut out = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let mut query = vec![("filter", filter), ("pageSize", read.page_size())];
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

/// One exercise response page: its complete points in page order, and the next page token.
pub fn parse_workouts(body: &str) -> Result<Parsed<Workout>> {
    let page: Page<ExercisePoint> = serde_json::from_str(body)?;
    let workouts = page
        .data_points
        .into_iter()
        .filter_map(|p| {
            let e = p.exercise?;
            let interval = e.interval?;
            let (start, end) = (interval.start_time?, interval.end_time?);
            Some(Workout {
                exercise_type: e.exercise_type.unwrap_or_default(),
                start: start.with_timezone(&Local),
                minutes: u32::try_from((end - start).num_minutes()).unwrap_or(0),
                kcal: e.metrics_summary.and_then(|m| m.calories_kcal),
            })
        })
        .collect();
    Ok((workouts, page.next_page_token))
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
    Ok(
        fetch_all(token, "weight", Read::List, &filter, parse_weights)
            .await?
            .into_iter()
            .max_by_key(|&(_, at)| at),
    )
}

/// Workouts starting (civil time) on any day from `from` to `to`, both inclusive.
pub async fn workouts(token: &str, from: NaiveDate, to: NaiveDate) -> Result<Vec<Workout>> {
    let end = to + Days::new(1);
    let filter = format!(
        "exercise.interval.civil_start_time >= \"{from}T00:00:00\" AND \
         exercise.interval.civil_start_time < \"{end}T00:00:00\""
    );
    let mut out = fetch_all(token, "exercise", Read::Reconcile, &filter, parse_workouts).await?;
    out.sort_by_key(|w| w.start);
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
    fn parses_exercise_page() {
        let body = r#"{
          "dataPoints": [
            {"exercise": {
              "interval": {"startTime": "2026-02-23T17:00:00Z", "endTime": "2026-02-23T17:45:30Z",
                "startUtcOffset": "3600s", "endUtcOffset": "3600s",
                "civilStartTime": {}, "civilEndTime": {}},
              "exerciseType": "RUNNING",
              "metricsSummary": {"caloriesKcal": 512.5, "distanceMillimeters": 8000000, "steps": 7000}}},
            {"exercise": {
              "interval": {"startTime": "2026-02-22T07:00:00Z", "endTime": "2026-02-22T08:00:00Z"},
              "exerciseType": "STRENGTH_TRAINING"}},
            {"exercise": {"exerciseType": "YOGA"}}
          ]
        }"#;
        let (w, next) = parse_workouts(body).unwrap();
        assert!(next.is_none());
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].exercise_type, "RUNNING");
        assert_eq!((w[0].minutes, w[0].kcal), (45, Some(512.5)));
        assert_eq!(w[1].exercise_type, "STRENGTH_TRAINING");
        assert_eq!((w[1].minutes, w[1].kcal), (60, None));
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
