//! The JSON API the Android app talks to. Its `OpenAPI` document generates the app's client.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use tapas_core::engine::Course;
use tapas_core::{Aggregation, Goal, GoalKind, Measurement, Observation, Series, Workout};
use tapas_sources::google::oauth::{GoogleApi, WebClient};
use tokio::sync::Notify;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::{self, IdTokenVerifier, Session};
use crate::seal::Sealer;
use crate::store::Store;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub http: reqwest::Client,
    pub google: Arc<WebClient>,
    pub verifier: Arc<IdTokenVerifier>,
    pub sealer: Arc<Sealer>,
    /// Wakes the sync loop, e.g. right after a new grant.
    pub sync_now: Arc<Notify>,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not signed in")]
    Unauthorized,
    #[error("{0}")]
    BadRequest(String),
    #[expect(dead_code, reason = "constructed once the slice 1 handlers land")]
    #[error("not found")]
    NotFound,
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Internal(error) => {
                tracing::error!("{error:#}");
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        (status, self.to_string()).into_response()
    }
}

struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        openapi
            .components
            .get_or_insert_default()
            .add_security_scheme(
                "session",
                SecurityScheme::Http(HttpBuilder::new().scheme(HttpAuthScheme::Bearer).build()),
            );
    }
}

#[derive(OpenApi)]
#[openapi(
    info(title = "tapas", license(name = "MIT", identifier = "MIT")),
    modifiers(&BearerAuth)
)]
struct ApiDoc;

pub fn router() -> (axum::Router<AppState>, utoipa::openapi::OpenApi) {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(healthz))
        .routes(routes!(sign_in, sign_out))
        .routes(routes!(me))
        .routes(routes!(connect_google))
        .routes(routes!(recent))
        .routes(routes!(goals, create_goal))
        .routes(routes!(update_goal, delete_goal))
        .routes(routes!(series, create_series))
        .routes(routes!(series_weeks))
        .routes(routes!(put_manual_entry))
        .routes(routes!(course))
        .split_for_parts()
}

#[utoipa::path(get, path = "/healthz", responses((status = 200, description = "The server is up", body = String)))]
async fn healthz() -> &'static str {
    "ok"
}

#[derive(Deserialize, ToSchema)]
struct SignIn {
    /// A Google ID token issued for the server's Web client id.
    id_token: String,
}

#[derive(Serialize, ToSchema)]
struct SessionToken {
    token: String,
}

#[utoipa::path(
    post,
    path = "/api/session",
    request_body = SignIn,
    responses((status = 200, description = "Signed in", body = SessionToken),
        (status = 401, description = "Not the allowed Google account"))
)]
async fn sign_in(
    State(state): State<AppState>,
    Json(body): Json<SignIn>,
) -> Result<Json<SessionToken>, ApiError> {
    if let Err(error) = state.verifier.verify(&state.http, &body.id_token).await {
        tracing::warn!("sign-in refused: {error:#}");
        return Err(ApiError::Unauthorized);
    }
    let token = auth::issue_session(&state).await?;
    Ok(Json(SessionToken { token }))
}

#[utoipa::path(delete, path = "/api/session", responses((status = 204, description = "Signed out")), security(("session" = [])))]
async fn sign_out(State(state): State<AppState>, session: Session) -> Result<StatusCode, ApiError> {
    state.store.delete_session(&session.token_hash).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, ToSchema)]
struct Me {
    email: String,
    grants: Vec<GrantStatus>,
}

#[derive(Serialize, ToSchema)]
struct GrantStatus {
    api: GoogleApi,
    granted_at: DateTime<Utc>,
    synced_at: Option<DateTime<Utc>>,
    /// The last sync's error; `None` once a sync succeeds.
    sync_error: Option<String>,
    /// Google revoked the grant; the app asks for consent again.
    needs_consent: bool,
}

#[utoipa::path(get, path = "/api/me", responses((status = 200, description = "The account and its Google grants", body = Me)), security(("session" = [])))]
async fn me(State(state): State<AppState>, _: Session) -> Result<Json<Me>, ApiError> {
    let grants = state
        .store
        .grants()
        .await?
        .into_iter()
        .map(|g| GrantStatus {
            api: g.api,
            granted_at: g.granted_at,
            synced_at: g.synced_at,
            sync_error: g.sync_error,
            needs_consent: g.needs_consent,
        })
        .collect();
    Ok(Json(Me {
        email: state.verifier.allowed_email.clone(),
        grants,
    }))
}

#[derive(Deserialize, ToSchema)]
struct ConnectGoogle {
    api: GoogleApi,
    /// From the app's authorization request with offline access for the server's Web client.
    server_auth_code: String,
}

#[utoipa::path(
    post,
    path = "/api/google/grants",
    request_body = ConnectGoogle,
    responses((status = 204, description = "Connected"),
        (status = 400, description = "Google refused the auth code")),
    security(("session" = []))
)]
async fn connect_google(
    State(state): State<AppState>,
    _: Session,
    Json(body): Json<ConnectGoogle>,
) -> Result<StatusCode, ApiError> {
    let grant = state
        .google
        .exchange_code(&state.http, &body.server_auth_code)
        .await
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let sealed = state.sealer.seal(&grant.refresh_token)?;
    state
        .store
        .upsert_grant(body.api, &sealed, Utc::now())
        .await?;
    state.sync_now.notify_one();
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize, ToSchema)]
struct Recent {
    measurements: Vec<Measurement>,
    workouts: Vec<Workout>,
}

#[utoipa::path(get, path = "/api/recent", responses((status = 200, description = "Latest measurements and workouts", body = Recent)), security(("session" = [])))]
async fn recent(State(state): State<AppState>, _: Session) -> Result<Json<Recent>, ApiError> {
    Ok(Json(Recent {
        measurements: state.store.recent_measurements(20).await?,
        workouts: state.store.recent_workouts(20).await?,
    }))
}

/// A goal as the app creates or edits it: every [`Goal`] field but the id.
#[derive(Deserialize, ToSchema)]
#[expect(dead_code, reason = "read once the slice 1 handlers land")]
struct GoalInput {
    parent_id: Option<i64>,
    title: String,
    kind: GoalKind,
    due: Option<NaiveDate>,
    /// Where the reference path starts; without one a checkpoint starts where the previous
    /// checkpoint in its stack ends, in date and value.
    start: Option<NaiveDate>,
    /// Controlled goals: the series steered on a straight path from `start_value` to
    /// `target_value`.
    series_id: Option<i64>,
    /// For a `sum` series, a running total since `start`.
    start_value: Option<f64>,
    /// The goal merges once its series reaches this. For a `sum` series, a running total since
    /// `start`.
    target_value: Option<f64>,
    /// Milestones: the day it was ticked done.
    done_on: Option<NaiveDate>,
    /// Share of the parent's error.
    weight: f64,
    /// Weekly time this goal needs.
    weekly_hours: Option<f64>,
    /// Master only: total weekly hours available.
    capacity_hours: Option<f64>,
    /// Largest allowed week-on-week rise of the weekly amount, as a fraction (0.1 = +10 %).
    max_ramp: Option<f64>,
}

#[utoipa::path(get, path = "/api/goals", responses((status = 200, description = "The goal tree, flat", body = Vec<Goal>)), security(("session" = [])))]
async fn goals(State(_state): State<AppState>, _: Session) -> Result<Json<Vec<Goal>>, ApiError> {
    todo!("slice 1 server")
}

#[utoipa::path(
    post,
    path = "/api/goals",
    request_body = GoalInput,
    responses((status = 200, description = "Created", body = Goal),
        (status = 400, description = "Not a valid goal")),
    security(("session" = []))
)]
async fn create_goal(
    State(_state): State<AppState>,
    _: Session,
    Json(_body): Json<GoalInput>,
) -> Result<Json<Goal>, ApiError> {
    todo!("slice 1 server")
}

#[utoipa::path(
    put,
    path = "/api/goals/{id}",
    params(("id" = i64, Path, description = "The goal's id")),
    request_body = GoalInput,
    responses((status = 200, description = "Updated", body = Goal),
        (status = 400, description = "Not a valid goal"),
        (status = 404, description = "No such goal")),
    security(("session" = []))
)]
async fn update_goal(
    State(_state): State<AppState>,
    _: Session,
    Path(_id): Path<i64>,
    Json(_body): Json<GoalInput>,
) -> Result<Json<Goal>, ApiError> {
    todo!("slice 1 server")
}

#[utoipa::path(
    delete,
    path = "/api/goals/{id}",
    params(("id" = i64, Path, description = "The goal's id")),
    responses((status = 204, description = "Deleted with its subtree"),
        (status = 400, description = "The master goal cannot be deleted"),
        (status = 404, description = "No such goal")),
    security(("session" = []))
)]
async fn delete_goal(
    State(_state): State<AppState>,
    _: Session,
    Path(_id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    todo!("slice 1 server")
}

#[utoipa::path(get, path = "/api/series", responses((status = 200, description = "Every series", body = Vec<Series>)), security(("session" = [])))]
async fn series(State(_state): State<AppState>, _: Session) -> Result<Json<Vec<Series>>, ApiError> {
    todo!("slice 1 server")
}

/// A user-created series; its feed is always manual.
#[derive(Deserialize, ToSchema)]
#[expect(dead_code, reason = "read once the slice 1 handlers land")]
struct SeriesInput {
    name: String,
    unit: String,
    aggregation: Aggregation,
}

#[utoipa::path(
    post,
    path = "/api/series",
    request_body = SeriesInput,
    responses((status = 200, description = "Created", body = Series)),
    security(("session" = []))
)]
async fn create_series(
    State(_state): State<AppState>,
    _: Session,
    Json(_body): Json<SeriesInput>,
) -> Result<Json<Series>, ApiError> {
    todo!("slice 1 server")
}

#[utoipa::path(
    get,
    path = "/api/series/{id}/weeks",
    params(("id" = i64, Path, description = "The series' id")),
    responses((status = 200, description = "Weekly values, oldest first", body = Vec<Observation>),
        (status = 404, description = "No such series")),
    security(("session" = []))
)]
async fn series_weeks(
    State(_state): State<AppState>,
    _: Session,
    Path(_id): Path<i64>,
) -> Result<Json<Vec<Observation>>, ApiError> {
    todo!("slice 1 server")
}

/// A manual series' value for one day.
#[derive(Deserialize, ToSchema)]
#[expect(dead_code, reason = "read once the slice 1 handlers land")]
struct ManualEntry {
    value: f64,
}

#[utoipa::path(
    put,
    path = "/api/series/{id}/entries/{day}",
    params(("id" = i64, Path, description = "The series' id"),
        ("day" = NaiveDate, Path, description = "The day the value is for")),
    request_body = ManualEntry,
    responses((status = 204, description = "Stored, replacing that day's value"),
        (status = 400, description = "The series is not fed manually"),
        (status = 404, description = "No such series")),
    security(("session" = []))
)]
async fn put_manual_entry(
    State(_state): State<AppState>,
    _: Session,
    Path((_id, _day)): Path<(i64, NaiveDate)>,
    Json(_body): Json<ManualEntry>,
) -> Result<StatusCode, ApiError> {
    todo!("slice 1 server")
}

#[utoipa::path(get, path = "/api/course", responses((status = 200, description = "Every goal's status and the master's verdict", body = Course)), security(("session" = [])))]
async fn course(State(_state): State<AppState>, _: Session) -> Result<Json<Course>, ApiError> {
    todo!("slice 1 server")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The app's client is generated from `android/openapi.json`; a stale copy fails here.
    #[test]
    fn committed_openapi_matches_the_server() {
        let generated = router().1.to_pretty_json().unwrap();
        let committed = include_str!("../../../android/openapi.json");
        assert!(
            generated.trim() == committed.trim(),
            "android/openapi.json is stale: run `cargo run -p tapas-server -- openapi > android/openapi.json`"
        );
    }
}
