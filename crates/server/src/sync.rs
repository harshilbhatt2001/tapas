//! Pulls every connected source on a timer, or at once when woken. Slice 2 grows this into the
//! full tick (measure, compute, decide, act).

use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Days, Utc};
use tapas_sources::google::oauth::{self, GoogleApi};
use tapas_sources::google::{calendar, health};

use crate::api::AppState;
use crate::store::GrantRow;

const INTERVAL: Duration = Duration::from_mins(30);
/// How far back the first sync of a new grant reaches.
const BACKFILL: Days = Days::new(365);
/// Re-read this much before the last sync, since sources deliver late (a watch syncs hours later).
const OVERLAP: Days = Days::new(7);

pub async fn run(state: AppState) {
    loop {
        if let Err(error) = sync_all(&state).await {
            tracing::error!("sync: {error:#}");
        }
        tokio::select! {
            () = tokio::time::sleep(INTERVAL) => {}
            () = state.sync_now.notified() => {}
        }
    }
}

async fn sync_all(state: &AppState) -> Result<()> {
    for grant in state.store.grants().await? {
        if grant.needs_consent {
            continue;
        }
        let now = Utc::now();
        let error = sync_grant(state, &grant, now).await.err();
        let needs_consent = error
            .as_ref()
            .is_some_and(|e| matches!(e.downcast_ref(), Some(oauth::Error::InvalidGrant)));
        let error = error.map(|e| format!("{e:#}"));
        if let Some(error) = &error {
            tracing::warn!("sync {}: {error}", grant.api.as_str());
        }
        state
            .store
            .record_sync(grant.api, now, error.as_deref(), needs_consent)
            .await?;
    }
    Ok(())
}

async fn sync_grant(state: &AppState, grant: &GrantRow, now: DateTime<Utc>) -> Result<()> {
    let refresh_token = state.sealer.open(&grant.sealed_refresh_token)?;
    let access_token = state
        .google
        .access_token(&state.http, &refresh_token)
        .await?;
    let since = grant.synced_at.map_or(now - BACKFILL, |at| at - OVERLAP);
    match grant.api {
        GoogleApi::Health => {
            let client = health::Client {
                http: &state.http,
                access_token: &access_token,
            };
            let weights = client.weights_since(since).await?;
            let workouts = client
                .workouts(since.date_naive(), now.date_naive())
                .await?;
            state.store.upsert_measurements(&weights).await?;
            state.store.upsert_workouts(&workouts).await?;
            tracing::info!(
                "Google Health: {} weights, {} workouts since {since}",
                weights.len(),
                workouts.len()
            );
        }
        GoogleApi::Calendar => {
            let busy = calendar::busy(access_token, now, now + Days::new(7)).await?;
            tracing::info!("Google Calendar: {} busy blocks this week", busy.len());
        }
    }
    Ok(())
}
