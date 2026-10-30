//! Google Health API v4: body weight and completed workouts.

use chrono::{DateTime, Days, NaiveDate, Utc};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_with::{DisplayFromStr, PickFirst, serde_as};
use tapas_core::{Measurement, Metric, Source, Workout};

const BASE: &str = "https://health.googleapis.com/v4/users/me/dataTypes";
const WEIGHT_PAGE_SIZE: &str = "1000";
/// The API's maximum for `exercise`.
const EXERCISE_PAGE_SIZE: &str = "25";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Google Health request failed")]
    Http(#[from] reqwest::Error),
    #[error("Google Health {data_type} returned {status}: {body}")]
    Status {
        data_type: &'static str,
        status: reqwest::StatusCode,
        body: String,
    },
}

pub struct Client<'a> {
    pub http: &'a reqwest::Client,
    pub access_token: &'a str,
}

impl Client<'_> {
    pub async fn weights_since(&self, since: DateTime<Utc>) -> Result<Vec<Measurement>, Error> {
        let filter = format!(
            "weight.sample_time.physical_time >= \"{}\"",
            since.format("%Y-%m-%dT%H:%M:%SZ")
        );
        let points: Vec<WeightPoint> = self
            .fetch_all("weight", "dataPoints", &filter, WEIGHT_PAGE_SIZE)
            .await?;
        Ok(points
            .into_iter()
            .filter_map(WeightPoint::measurement)
            .collect())
    }

    /// Workouts that started, in the civil time where they were recorded, on any day from `from`
    /// to `to` inclusive. Reconciled, so one ride synced by two devices counts once.
    pub async fn workouts(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<Workout>, Error> {
        let end = to + Days::new(1);
        let filter = format!(
            "exercise.interval.civil_start_time >= \"{from}T00:00:00\" AND \
             exercise.interval.civil_start_time < \"{end}T00:00:00\""
        );
        let points: Vec<ExercisePoint> = self
            .fetch_all(
                "exercise",
                "dataPoints:reconcile",
                &filter,
                EXERCISE_PAGE_SIZE,
            )
            .await?;
        Ok(points
            .into_iter()
            .filter_map(ExercisePoint::workout)
            .collect())
    }

    async fn fetch_all<T: DeserializeOwned>(
        &self,
        data_type: &'static str,
        read: &str,
        filter: &str,
        page_size: &str,
    ) -> Result<Vec<T>, Error> {
        let url = format!("{BASE}/{data_type}/{read}");
        let mut points = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut query = vec![("filter", filter), ("pageSize", page_size)];
            if let Some(token) = page_token.as_deref() {
                query.push(("pageToken", token));
            }
            let response = self
                .http
                .get(&url)
                .bearer_auth(self.access_token)
                .query(&query)
                .send()
                .await?;
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await?;
                return Err(Error::Status {
                    data_type,
                    status,
                    body,
                });
            }
            let page: Page<T> = response.json().await?;
            points.extend(page.data_points);
            page_token = page.next_page_token.filter(|t| !t.is_empty());
            if page_token.is_none() {
                return Ok(points);
            }
        }
    }
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

impl WeightPoint {
    fn measurement(self) -> Option<Measurement> {
        let weight = self.weight?;
        Some(Measurement {
            metric: Metric::BodyWeightKg,
            at: weight.sample_time?.physical_time?,
            value: weight.weight_grams? / 1000.0,
            source: Source::GoogleHealth,
        })
    }
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
    active_duration: Option<String>,
    metrics_summary: Option<MetricsSummary>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Interval {
    start_time: Option<DateTime<Utc>>,
    end_time: Option<DateTime<Utc>>,
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
}

impl ExercisePoint {
    fn workout(self) -> Option<Workout> {
        let exercise = self.exercise?;
        let interval = exercise.interval?;
        let (start, end) = (interval.start_time?, interval.end_time?);
        let activity = exercise.exercise_type.unwrap_or_default();
        let source_id = self
            .data_point_name
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("{activity}@{}", start.to_rfc3339()));
        let metrics = exercise.metrics_summary.unwrap_or_default();
        Some(Workout {
            source: Source::GoogleHealth,
            source_id,
            activity,
            start,
            end,
            active_secs: exercise.active_duration.as_deref().and_then(duration_secs),
            distance_m: metrics.distance_millimeters.map(|mm| mm / 1000.0),
            avg_hr: metrics.average_heart_rate_beats_per_minute,
            kcal: metrics.calories_kcal,
        })
    }
}

/// Whole seconds of a protobuf JSON duration such as `3600s` or `3.5s`.
fn duration_secs(duration: &str) -> Option<u32> {
    let secs: f64 = duration.strip_suffix('s')?.parse().ok()?;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to u32"
    )]
    (secs.is_finite()).then(|| secs.round().clamp(0.0, f64::from(u32::MAX)) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workout_without_name_gets_a_stable_id_and_string_numbers_parse() {
        let page: Page<ExercisePoint> = serde_json::from_str(
            r#"{"dataPoints":[{"exercise":{
                "interval":{"startTime":"2026-10-28T06:00:00Z","endTime":"2026-10-28T07:00:00Z"},
                "exerciseType":"RUNNING","activeDuration":"3540.4s",
                "metricsSummary":{"distanceMillimeters":10000000,"averageHeartRateBeatsPerMinute":"142"}
            }}],"nextPageToken":""}"#,
        )
        .unwrap();
        let workout = page
            .data_points
            .into_iter()
            .next()
            .unwrap()
            .workout()
            .unwrap();
        assert_eq!(workout.source_id, "RUNNING@2026-10-28T06:00:00+00:00");
        assert_eq!(workout.active_secs, Some(3540));
        assert_eq!(workout.distance_m, Some(10_000.0));
        assert_eq!(workout.avg_hr, Some(142));
    }

    #[test]
    fn incomplete_weight_points_are_skipped() {
        let page: Page<WeightPoint> = serde_json::from_str(
            r#"{"dataPoints":[
                {"weight":{"sampleTime":{"physicalTime":"2026-10-28T06:00:00Z"},"weightGrams":81500}},
                {"weight":{"weightGrams":81000}}
            ]}"#,
        )
        .unwrap();
        let kg: Vec<f64> = page
            .data_points
            .into_iter()
            .filter_map(WeightPoint::measurement)
            .map(|m| m.value)
            .collect();
        assert_eq!(kg, [81.5]);
    }
}
