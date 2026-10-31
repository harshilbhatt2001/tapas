//! The SQLite store. Schema in `schema.sql`.

use std::path::Path;

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{DateTime, Days, NaiveDate, Utc, Weekday};
use sqlx::AssertSqlSafe;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool};
use tapas_core::{Aggregation, Feed, Goal, Measurement, Metric, Observation, Series, Workout};
use tapas_sources::google::oauth::GoogleApi;

#[derive(Clone)]
pub struct Store(SqlitePool);

pub struct GrantRow {
    pub api: GoogleApi,
    pub sealed_refresh_token: Vec<u8>,
    pub granted_at: DateTime<Utc>,
    pub synced_at: Option<DateTime<Utc>>,
    pub sync_error: Option<String>,
    pub needs_consent: bool,
}

impl Store {
    pub async fn open(path: &Path) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true);
        Self::with_options(options, 4).await
    }

    /// One connection, since each in-memory connection is its own database.
    #[cfg(test)]
    pub async fn in_memory() -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true);
        Self::with_options(options, 1).await
    }

    async fn with_options(options: SqliteConnectOptions, max_connections: u32) -> Result<Self> {
        let pool = sqlx::pool::PoolOptions::new()
            .max_connections(max_connections)
            .connect_with(options)
            .await?;
        sqlx::raw_sql(include_str!("schema.sql"))
            .execute(&pool)
            .await?;
        Ok(Self(pool))
    }

    pub async fn insert_session(&self, token_hash: &[u8], expires_at: DateTime<Utc>) -> Result<()> {
        sqlx::query("INSERT INTO sessions (token_hash, expires_at) VALUES (?, ?)")
            .bind(token_hash)
            .bind(expires_at)
            .execute(&self.0)
            .await?;
        Ok(())
    }

    pub async fn session_valid(&self, token_hash: &[u8], now: DateTime<Utc>) -> Result<bool> {
        let found: Option<(i64,)> =
            sqlx::query_as("SELECT 1 FROM sessions WHERE token_hash = ? AND expires_at > ?")
                .bind(token_hash)
                .bind(now)
                .fetch_optional(&self.0)
                .await?;
        Ok(found.is_some())
    }

    pub async fn delete_session(&self, token_hash: &[u8]) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
            .bind(token_hash)
            .execute(&self.0)
            .await?;
        Ok(())
    }

    pub async fn upsert_grant(
        &self,
        api: GoogleApi,
        sealed_refresh_token: &[u8],
        now: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO google_grants (api, sealed_refresh_token, granted_at) VALUES (?, ?, ?)
             ON CONFLICT (api) DO UPDATE SET sealed_refresh_token = excluded.sealed_refresh_token,
                 granted_at = excluded.granted_at, sync_error = NULL, needs_consent = 0",
        )
        .bind(api.as_str())
        .bind(sealed_refresh_token)
        .bind(now)
        .execute(&self.0)
        .await?;
        Ok(())
    }

    pub async fn grants(&self) -> Result<Vec<GrantRow>> {
        #[derive(sqlx::FromRow)]
        struct Row {
            api: String,
            sealed_refresh_token: Vec<u8>,
            granted_at: DateTime<Utc>,
            synced_at: Option<DateTime<Utc>>,
            sync_error: Option<String>,
            needs_consent: bool,
        }
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT api, sealed_refresh_token, granted_at, synced_at, sync_error, needs_consent
             FROM google_grants ORDER BY api",
        )
        .fetch_all(&self.0)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(GrantRow {
                    api: from_text(&r.api)?,
                    sealed_refresh_token: r.sealed_refresh_token,
                    granted_at: r.granted_at,
                    synced_at: r.synced_at,
                    sync_error: r.sync_error,
                    needs_consent: r.needs_consent,
                })
            })
            .collect()
    }

    pub async fn record_sync(
        &self,
        api: GoogleApi,
        at: DateTime<Utc>,
        error: Option<&str>,
        needs_consent: bool,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE google_grants SET synced_at = CASE WHEN ? IS NULL THEN ? ELSE synced_at END,
                 sync_error = ?, needs_consent = ? WHERE api = ?",
        )
        .bind(error)
        .bind(at)
        .bind(error)
        .bind(needs_consent)
        .bind(api.as_str())
        .execute(&self.0)
        .await?;
        Ok(())
    }

    pub async fn upsert_measurements(&self, measurements: &[Measurement]) -> Result<()> {
        let mut tx = self.0.begin().await?;
        for m in measurements {
            sqlx::query(
                "INSERT INTO measurements (metric, source, at, value) VALUES (?, ?, ?, ?)
                 ON CONFLICT DO UPDATE SET value = excluded.value",
            )
            .bind(enum_text(&m.metric))
            .bind(enum_text(&m.source))
            .bind(m.at)
            .bind(m.value)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn upsert_workouts(&self, workouts: &[Workout]) -> Result<()> {
        let mut tx = self.0.begin().await?;
        for w in workouts {
            sqlx::query(
                "INSERT INTO workouts (source, source_id, activity, start, end, active_secs,
                     distance_m, avg_hr, kcal) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT DO UPDATE SET activity = excluded.activity, start = excluded.start,
                     end = excluded.end, active_secs = excluded.active_secs,
                     distance_m = excluded.distance_m, avg_hr = excluded.avg_hr,
                     kcal = excluded.kcal",
            )
            .bind(enum_text(&w.source))
            .bind(&w.source_id)
            .bind(&w.activity)
            .bind(w.start)
            .bind(w.end)
            .bind(w.active_secs)
            .bind(w.distance_m)
            .bind(w.avg_hr)
            .bind(w.kcal)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn recent_measurements(&self, limit: u32) -> Result<Vec<Measurement>> {
        let rows: Vec<(String, String, DateTime<Utc>, f64)> = sqlx::query_as(
            "SELECT metric, source, at, value FROM measurements ORDER BY at DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.0)
        .await?;
        rows.into_iter()
            .map(|(metric, source, at, value)| {
                Ok(Measurement {
                    metric: from_text(&metric)?,
                    source: from_text(&source)?,
                    at,
                    value,
                })
            })
            .collect()
    }

    pub async fn recent_workouts(&self, limit: u32) -> Result<Vec<Workout>> {
        #[derive(sqlx::FromRow)]
        struct Row {
            source: String,
            source_id: String,
            activity: String,
            start: DateTime<Utc>,
            end: DateTime<Utc>,
            active_secs: Option<u32>,
            distance_m: Option<f64>,
            avg_hr: Option<u16>,
            kcal: Option<f64>,
        }
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT source, source_id, activity, start, end, active_secs, distance_m, avg_hr, kcal
             FROM workouts ORDER BY start DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.0)
        .await?;
        rows.into_iter()
            .map(|r| {
                Ok(Workout {
                    source: from_text(&r.source)?,
                    source_id: r.source_id,
                    activity: r.activity,
                    start: r.start,
                    end: r.end,
                    active_secs: r.active_secs,
                    distance_m: r.distance_m,
                    avg_hr: r.avg_hr,
                    kcal: r.kcal,
                })
            })
            .collect()
    }

    /// Creates the master goal unless one exists.
    pub async fn ensure_master(&self) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO goals (title, kind, weight) VALUES ('Target state', 'master', 1)",
        )
        .execute(&self.0)
        .await?;
        Ok(())
    }

    pub async fn goals(&self) -> Result<Vec<Goal>> {
        let rows: Vec<GoalRow> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT {GOAL_COLUMNS} FROM goals ORDER BY id"
        )))
        .fetch_all(&self.0)
        .await?;
        rows.into_iter().map(GoalRow::into_goal).collect()
    }

    pub async fn goal(&self, id: i64) -> Result<Option<Goal>> {
        let row: Option<GoalRow> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT {GOAL_COLUMNS} FROM goals WHERE id = ?"
        )))
        .bind(id)
        .fetch_optional(&self.0)
        .await?;
        row.map(GoalRow::into_goal).transpose()
    }

    /// Stores `goal` under a new id, ignoring `goal.id`, and returns that id.
    pub async fn insert_goal(&self, goal: &Goal) -> Result<i64> {
        self.upsert_goal(None, goal).await
    }

    /// Overwrites the stored goal with `goal.id`.
    pub async fn update_goal(&self, goal: &Goal) -> Result<()> {
        self.upsert_goal(Some(goal.id), goal).await?;
        Ok(())
    }

    async fn upsert_goal(&self, id: Option<i64>, goal: &Goal) -> Result<i64> {
        let (id,): (i64,) = sqlx::query_as(AssertSqlSafe(format!(
            "INSERT INTO goals ({GOAL_COLUMNS}) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET parent_id = excluded.parent_id,
                 title = excluded.title, kind = excluded.kind, due = excluded.due,
                 start = excluded.start, series_id = excluded.series_id,
                 start_value = excluded.start_value, target_value = excluded.target_value,
                 done_on = excluded.done_on, weight = excluded.weight,
                 weekly_hours = excluded.weekly_hours, capacity_hours = excluded.capacity_hours,
                 max_ramp = excluded.max_ramp
             RETURNING id"
        )))
        .bind(id)
        .bind(goal.parent_id)
        .bind(&goal.title)
        .bind(enum_text(&goal.kind))
        .bind(goal.due)
        .bind(goal.start)
        .bind(goal.series_id)
        .bind(goal.start_value)
        .bind(goal.target_value)
        .bind(goal.done_on)
        .bind(goal.weight)
        .bind(goal.weekly_hours)
        .bind(goal.capacity_hours)
        .bind(goal.max_ramp)
        .fetch_one(&self.0)
        .await?;
        Ok(id)
    }

    /// Deletes the goal and its subtree.
    pub async fn delete_goal(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM goals WHERE id = ?")
            .bind(id)
            .execute(&self.0)
            .await?;
        Ok(())
    }

    pub async fn all_series(&self) -> Result<Vec<Series>> {
        let rows: Vec<SeriesRow> =
            sqlx::query_as("SELECT id, name, unit, aggregation, feed FROM series ORDER BY id")
                .fetch_all(&self.0)
                .await?;
        rows.into_iter().map(SeriesRow::into_series).collect()
    }

    pub async fn series(&self, id: i64) -> Result<Option<Series>> {
        let row: Option<SeriesRow> =
            sqlx::query_as("SELECT id, name, unit, aggregation, feed FROM series WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.0)
                .await?;
        row.map(SeriesRow::into_series).transpose()
    }

    /// Creates a manually fed series.
    pub async fn insert_series(
        &self,
        name: &str,
        unit: &str,
        aggregation: Aggregation,
    ) -> Result<Series> {
        let (id,): (i64,) = sqlx::query_as(
            "INSERT INTO series (name, unit, aggregation, feed) VALUES (?, ?, ?, ?) RETURNING id",
        )
        .bind(name)
        .bind(unit)
        .bind(enum_text(&aggregation))
        .bind(enum_text(&Feed::Manual))
        .fetch_one(&self.0)
        .await?;
        Ok(Series {
            id,
            name: name.to_owned(),
            unit: unit.to_owned(),
            aggregation,
            feed: Feed::Manual,
        })
    }

    pub async fn upsert_manual_entry(
        &self,
        series_id: i64,
        day: NaiveDate,
        value: f64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO manual_entries (series_id, day, value) VALUES (?, ?, ?)
             ON CONFLICT DO UPDATE SET value = excluded.value",
        )
        .bind(series_id)
        .bind(day)
        .bind(value)
        .execute(&self.0)
        .await?;
        Ok(())
    }

    /// The series' weekly values through the ISO week of `until`, oldest first.
    pub async fn series_weeks(
        &self,
        series: &Series,
        until: NaiveDate,
    ) -> Result<Vec<Observation>> {
        let readings: Vec<(NaiveDate, f64)> = match series.feed {
            Feed::BodyWeight => {
                let mut rows: Vec<(DateTime<Utc>, f64)> =
                    sqlx::query_as("SELECT at, value FROM measurements WHERE metric = ?")
                        .bind(enum_text(&Metric::BodyWeightKg))
                        .fetch_all(&self.0)
                        .await?;
                rows.sort_by_key(|&(at, _)| at);
                rows.into_iter()
                    .map(|(at, value)| (at.date_naive(), value))
                    .collect()
            }
            Feed::TrainingHours => {
                let rows: Vec<(DateTime<Utc>, DateTime<Utc>, Option<u32>)> =
                    sqlx::query_as("SELECT start, end, active_secs FROM workouts")
                        .fetch_all(&self.0)
                        .await?;
                rows.into_iter()
                    .map(|(start, end, active_secs)| {
                        let secs =
                            active_secs.map_or_else(|| (end - start).num_seconds(), i64::from);
                        #[expect(clippy::cast_precision_loss, reason = "a workout's seconds")]
                        let hours = secs as f64 / 3600.0;
                        (start.date_naive(), hours)
                    })
                    .collect()
            }
            Feed::Manual => {
                sqlx::query_as(
                    "SELECT day, value FROM manual_entries WHERE series_id = ? ORDER BY day",
                )
                .bind(series.id)
                .fetch_all(&self.0)
                .await?
            }
        };
        Ok(weekly(readings, series.aggregation, until))
    }
}

/// Collapses dated readings, oldest first, into one value per ISO week through the week of
/// `until`. A [`Aggregation::Sum`] reads 0 in the weeks without readings after its first one.
fn weekly(
    readings: Vec<(NaiveDate, f64)>,
    aggregation: Aggregation,
    until: NaiveDate,
) -> Vec<Observation> {
    let last_week = week_of(until);
    let mut weeks = BTreeMap::new();
    for (day, value) in readings {
        let week = week_of(day);
        if week > last_week {
            continue;
        }
        match aggregation {
            Aggregation::Level => {
                weeks.insert(week, value);
            }
            Aggregation::Sum => *weeks.entry(week).or_insert(0.0) += value,
        }
    }
    if aggregation == Aggregation::Sum
        && let Some(&first_week) = weeks.keys().next()
    {
        let mut week = first_week;
        while week <= last_week {
            weeks.entry(week).or_insert(0.0);
            week = week + Days::new(7);
        }
    }
    weeks
        .into_iter()
        .map(|(week, value)| Observation { week, value })
        .collect()
}

fn week_of(day: NaiveDate) -> NaiveDate {
    day.week(Weekday::Mon).first_day()
}

const GOAL_COLUMNS: &str = "id, parent_id, title, kind, due, start, series_id, start_value,
    target_value, done_on, weight, weekly_hours, capacity_hours, max_ramp";

#[derive(sqlx::FromRow)]
struct GoalRow {
    id: i64,
    parent_id: Option<i64>,
    title: String,
    kind: String,
    due: Option<NaiveDate>,
    start: Option<NaiveDate>,
    series_id: Option<i64>,
    start_value: Option<f64>,
    target_value: Option<f64>,
    done_on: Option<NaiveDate>,
    weight: f64,
    weekly_hours: Option<f64>,
    capacity_hours: Option<f64>,
    max_ramp: Option<f64>,
}

impl GoalRow {
    fn into_goal(self) -> Result<Goal> {
        Ok(Goal {
            id: self.id,
            parent_id: self.parent_id,
            title: self.title,
            kind: from_text(&self.kind)?,
            due: self.due,
            start: self.start,
            series_id: self.series_id,
            start_value: self.start_value,
            target_value: self.target_value,
            done_on: self.done_on,
            weight: self.weight,
            weekly_hours: self.weekly_hours,
            capacity_hours: self.capacity_hours,
            max_ramp: self.max_ramp,
        })
    }
}

#[derive(sqlx::FromRow)]
struct SeriesRow {
    id: i64,
    name: String,
    unit: String,
    aggregation: String,
    feed: String,
}

impl SeriesRow {
    fn into_series(self) -> Result<Series> {
        Ok(Series {
            id: self.id,
            name: self.name,
            unit: self.unit,
            aggregation: from_text(&self.aggregation)?,
            feed: from_text(&self.feed)?,
        })
    }
}

/// The serde name of a unit enum variant, as stored in TEXT columns.
fn enum_text<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        other => unreachable!("stored enums serialize to strings, got {other:?}"),
    }
}

fn from_text<T: serde::de::DeserializeOwned>(text: &str) -> Result<T> {
    Ok(serde_json::from_value(serde_json::Value::String(
        text.to_owned(),
    ))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tapas_core::{Metric, Source};

    #[tokio::test]
    async fn resyncing_a_window_updates_instead_of_duplicating() {
        let store = Store::in_memory().await.unwrap();
        let at = "2026-10-28T06:00:00Z".parse().unwrap();
        let weight = |value| Measurement {
            metric: Metric::BodyWeightKg,
            at,
            value,
            source: Source::GoogleHealth,
        };
        store.upsert_measurements(&[weight(81.0)]).await.unwrap();
        store.upsert_measurements(&[weight(80.5)]).await.unwrap();
        assert_eq!(store.recent_measurements(10).await.unwrap(), [weight(80.5)]);
    }

    #[tokio::test]
    async fn a_failed_sync_keeps_the_last_success_time() {
        let store = Store::in_memory().await.unwrap();
        let t0 = "2026-10-28T06:00:00Z".parse().unwrap();
        let t1 = "2026-10-28T06:30:00Z".parse().unwrap();
        store
            .upsert_grant(GoogleApi::Health, b"sealed", t0)
            .await
            .unwrap();
        store
            .record_sync(GoogleApi::Health, t0, None, false)
            .await
            .unwrap();
        store
            .record_sync(GoogleApi::Health, t1, Some("403"), false)
            .await
            .unwrap();
        let grant = store.grants().await.unwrap().remove(0);
        assert_eq!(grant.synced_at, Some(t0));
        assert_eq!(grant.sync_error.as_deref(), Some("403"));
    }

    fn day(text: &str) -> NaiveDate {
        text.parse().unwrap()
    }

    fn week(monday: &str, value: f64) -> Observation {
        Observation {
            week: day(monday),
            value,
        }
    }

    #[tokio::test]
    async fn a_level_series_reads_each_weeks_last_value_without_filling_gaps() {
        let store = Store::in_memory().await.unwrap();
        let weight = |at: &str, value| Measurement {
            metric: Metric::BodyWeightKg,
            at: at.parse().unwrap(),
            value,
            source: Source::GoogleHealth,
        };
        store
            .upsert_measurements(&[
                weight("2026-10-25T07:00:00Z", 80.0),
                weight("2026-10-20T07:00:00Z", 81.0),
                weight("2026-11-03T07:00:00Z", 79.5),
            ])
            .await
            .unwrap();
        let body_weight = store.series(1).await.unwrap().unwrap();
        assert_eq!(
            store
                .series_weeks(&body_weight, day("2026-11-12"))
                .await
                .unwrap(),
            [week("2026-10-19", 80.0), week("2026-11-02", 79.5)]
        );
    }

    #[tokio::test]
    async fn training_hours_total_each_week_of_start_and_read_zero_in_empty_weeks() {
        let store = Store::in_memory().await.unwrap();
        let workout = |id: &str, start: &str, end: &str, active_secs| Workout {
            source: Source::GoogleHealth,
            source_id: id.to_owned(),
            activity: "RUNNING".to_owned(),
            start: start.parse().unwrap(),
            end: end.parse().unwrap(),
            active_secs,
            distance_m: None,
            avg_hr: None,
            kcal: None,
        };
        store
            .upsert_workouts(&[
                workout(
                    "sunday-late",
                    "2026-10-25T23:30:00Z",
                    "2026-10-26T00:45:00Z",
                    Some(3600),
                ),
                workout(
                    "monday-no-active-time",
                    "2026-10-26T06:00:00Z",
                    "2026-10-26T07:30:00Z",
                    None,
                ),
                workout(
                    "monday-evening",
                    "2026-10-26T18:00:00Z",
                    "2026-10-26T19:00:00Z",
                    Some(1800),
                ),
                workout(
                    "after-until",
                    "2026-11-16T18:00:00Z",
                    "2026-11-16T19:00:00Z",
                    Some(3600),
                ),
            ])
            .await
            .unwrap();
        let training_hours = store.series(2).await.unwrap().unwrap();
        assert_eq!(
            store
                .series_weeks(&training_hours, day("2026-11-12"))
                .await
                .unwrap(),
            [
                week("2026-10-19", 1.0),
                week("2026-10-26", 2.0),
                week("2026-11-02", 0.0),
                week("2026-11-09", 0.0),
            ]
        );
    }

    #[tokio::test]
    async fn a_manual_entry_replaces_that_days_value() {
        let store = Store::in_memory().await.unwrap();
        let pages = store
            .insert_series("Pages read", "pages", Aggregation::Sum)
            .await
            .unwrap();
        for value in [10.0, 12.0] {
            store
                .upsert_manual_entry(pages.id, day("2026-10-28"), value)
                .await
                .unwrap();
        }
        store
            .upsert_manual_entry(pages.id, day("2026-10-29"), 5.0)
            .await
            .unwrap();
        assert_eq!(
            store.series_weeks(&pages, day("2026-10-31")).await.unwrap(),
            [week("2026-10-26", 17.0)]
        );
    }

    #[tokio::test]
    async fn the_master_is_created_once_and_updates_in_place() {
        let store = Store::in_memory().await.unwrap();
        store.ensure_master().await.unwrap();
        store.ensure_master().await.unwrap();
        let mut master = store.goals().await.unwrap().remove(0);
        master.capacity_hours = Some(12.0);
        store.update_goal(&master).await.unwrap();
        assert_eq!(store.goals().await.unwrap(), [master]);
    }
}
