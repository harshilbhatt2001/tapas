//! The SQLite store. Schema in `schema.sql`.

use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool};
use tapas_core::{Measurement, Workout};
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
}
