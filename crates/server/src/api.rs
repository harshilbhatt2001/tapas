//! The JSON API the Android app talks to. Its `OpenAPI` document generates the app's client.

use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::Context;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use tapas_core::engine::{self, Course, Readings};
use tapas_core::{Aggregation, Feed, Goal, GoalKind, Measurement, Observation, Series, Workout};
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

impl GoalInput {
    fn into_goal(self, id: i64) -> Goal {
        Goal {
            id,
            parent_id: self.parent_id,
            title: self.title,
            kind: self.kind,
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
        }
    }
}

/// Checks that `goal` fits the tree `goals`, either as a new goal or replacing `previous`.
fn validate_goal(
    goal: &Goal,
    previous: Option<&Goal>,
    goals: &[Goal],
    series: &[Series],
) -> Result<(), ApiError> {
    let refuse = |message: String| Err(ApiError::BadRequest(message));
    match (previous.map(|p| p.kind), goal.kind) {
        (None, GoalKind::Master) => return refuse("the master goal already exists".into()),
        (Some(GoalKind::Master), GoalKind::Master) => {
            if goal.parent_id.is_some() {
                return refuse("the master goal has no parent".into());
            }
        }
        (Some(_), GoalKind::Master) => return refuse("a goal cannot become the master".into()),
        (Some(GoalKind::Master), _) => {
            return refuse("the master goal must stay the master".into());
        }
        _ => {
            let Some(parent_id) = goal.parent_id else {
                return refuse("a goal needs a parent".into());
            };
            if !goals.iter().any(|g| g.id == parent_id) {
                return refuse(format!("parent goal {parent_id} does not exist"));
            }
            if previous.is_some() {
                let mut ancestor = Some(parent_id);
                while let Some(id) = ancestor {
                    if id == goal.id {
                        return refuse("a goal cannot sit under itself or its subgoals".into());
                    }
                    ancestor = goals.iter().find(|g| g.id == id).and_then(|g| g.parent_id);
                }
            }
        }
    }
    match goal.series_id {
        Some(id) if !series.iter().any(|s| s.id == id) => {
            return refuse(format!("series {id} does not exist"));
        }
        None if goal.kind == GoalKind::Controlled => {
            return refuse("a controlled goal needs a series".into());
        }
        _ => {}
    }
    if goal.weight <= 0.0 {
        return refuse("weight must be above 0".into());
    }
    if goal.max_ramp.is_some_and(|ramp| ramp < 0.0) {
        return refuse("max_ramp must not be negative".into());
    }
    Ok(())
}

fn ensure_deletable(goal: &Goal) -> Result<(), ApiError> {
    if goal.kind == GoalKind::Master {
        return Err(ApiError::BadRequest(
            "the master goal cannot be deleted".into(),
        ));
    }
    Ok(())
}

#[utoipa::path(get, path = "/api/goals", responses((status = 200, description = "The goal tree, flat", body = Vec<Goal>)), security(("session" = [])))]
async fn goals(State(state): State<AppState>, _: Session) -> Result<Json<Vec<Goal>>, ApiError> {
    Ok(Json(state.store.goals().await?))
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
    State(state): State<AppState>,
    _: Session,
    Json(body): Json<GoalInput>,
) -> Result<Json<Goal>, ApiError> {
    let mut goal = body.into_goal(0);
    let goals = state.store.goals().await?;
    validate_goal(&goal, None, &goals, &state.store.all_series().await?)?;
    goal.id = state.store.insert_goal(&goal).await?;
    Ok(Json(goal))
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
    State(state): State<AppState>,
    _: Session,
    Path(id): Path<i64>,
    Json(body): Json<GoalInput>,
) -> Result<Json<Goal>, ApiError> {
    let goals = state.store.goals().await?;
    let previous = goals
        .iter()
        .find(|g| g.id == id)
        .ok_or(ApiError::NotFound)?;
    let goal = body.into_goal(id);
    validate_goal(
        &goal,
        Some(previous),
        &goals,
        &state.store.all_series().await?,
    )?;
    state.store.update_goal(&goal).await?;
    Ok(Json(goal))
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
    State(state): State<AppState>,
    _: Session,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let goal = state.store.goal(id).await?.ok_or(ApiError::NotFound)?;
    ensure_deletable(&goal)?;
    state.store.delete_goal(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/api/series", responses((status = 200, description = "Every series", body = Vec<Series>)), security(("session" = [])))]
async fn series(State(state): State<AppState>, _: Session) -> Result<Json<Vec<Series>>, ApiError> {
    Ok(Json(state.store.all_series().await?))
}

/// A user-created series; its feed is always manual.
#[derive(Deserialize, ToSchema)]
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
    State(state): State<AppState>,
    _: Session,
    Json(body): Json<SeriesInput>,
) -> Result<Json<Series>, ApiError> {
    let series = state
        .store
        .insert_series(&body.name, &body.unit, body.aggregation)
        .await?;
    Ok(Json(series))
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
    State(state): State<AppState>,
    _: Session,
    Path(id): Path<i64>,
) -> Result<Json<Vec<Observation>>, ApiError> {
    let series = state.store.series(id).await?.ok_or(ApiError::NotFound)?;
    let weeks = state
        .store
        .series_weeks(&series, Utc::now().date_naive())
        .await?;
    Ok(Json(weeks))
}

/// A manual series' value for one day.
#[derive(Deserialize, ToSchema)]
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
    State(state): State<AppState>,
    _: Session,
    Path((id, day)): Path<(i64, NaiveDate)>,
    Json(body): Json<ManualEntry>,
) -> Result<StatusCode, ApiError> {
    let series = state.store.series(id).await?.ok_or(ApiError::NotFound)?;
    ensure_manual(&series)?;
    state.store.upsert_manual_entry(id, day, body.value).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn ensure_manual(series: &Series) -> Result<(), ApiError> {
    if series.feed != Feed::Manual {
        return Err(ApiError::BadRequest(format!(
            "series {} is fed from a source, not by hand",
            series.id
        )));
    }
    Ok(())
}

#[utoipa::path(get, path = "/api/course", responses((status = 200, description = "Every goal's status and the master's verdict", body = Course)), security(("session" = [])))]
async fn course(State(state): State<AppState>, _: Session) -> Result<Json<Course>, ApiError> {
    let today = Utc::now().date_naive();
    let goals = state.store.goals().await?;
    let series_ids: BTreeSet<i64> = goals.iter().filter_map(|g| g.series_id).collect();
    let mut readings = Vec::with_capacity(series_ids.len());
    for id in series_ids {
        let series = state
            .store
            .series(id)
            .await?
            .with_context(|| format!("goal series {id} vanished"))?;
        let weeks = state.store.series_weeks(&series, today).await?;
        readings.push(Readings { series, weeks });
    }
    Ok(Json(engine::course(&goals, &readings, today)))
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

    fn goal(id: i64, parent_id: Option<i64>, kind: GoalKind) -> Goal {
        Goal {
            id,
            parent_id,
            title: format!("goal {id}"),
            kind,
            due: None,
            start: None,
            series_id: None,
            start_value: None,
            target_value: None,
            done_on: None,
            weight: 1.0,
            weekly_hours: None,
            capacity_hours: None,
            max_ramp: None,
        }
    }

    fn series(id: i64, feed: Feed) -> Series {
        Series {
            id,
            name: format!("series {id}"),
            unit: "h".into(),
            aggregation: Aggregation::Sum,
            feed,
        }
    }

    /// Master 1 with child 2 and grandchild 3.
    fn tree() -> Vec<Goal> {
        vec![
            goal(1, None, GoalKind::Master),
            goal(2, Some(1), GoalKind::Milestone),
            goal(3, Some(2), GoalKind::Milestone),
        ]
    }

    #[test]
    fn a_second_master_is_refused() {
        let master = goal(0, None, GoalKind::Master);
        assert!(validate_goal(&master, None, &tree(), &[]).is_err());
    }

    #[test]
    fn a_goal_cannot_turn_into_the_master() {
        let goals = tree();
        let promoted = goal(2, None, GoalKind::Master);
        assert!(validate_goal(&promoted, Some(&goals[1]), &goals, &[]).is_err());
    }

    #[test]
    fn the_master_can_be_edited_in_place() {
        let goals = tree();
        let mut master = goals[0].clone();
        master.capacity_hours = Some(12.0);
        assert!(validate_goal(&master, Some(&goals[0]), &goals, &[]).is_ok());
    }

    #[test]
    fn moving_a_goal_under_its_descendant_or_itself_is_refused() {
        let goals = tree();
        for parent in [2, 3] {
            let moved = goal(2, Some(parent), GoalKind::Milestone);
            assert!(validate_goal(&moved, Some(&goals[1]), &goals, &[]).is_err());
        }
        let moved = goal(3, Some(1), GoalKind::Milestone);
        assert!(validate_goal(&moved, Some(&goals[2]), &goals, &[]).is_ok());
    }

    #[test]
    fn a_goal_without_an_existing_parent_is_refused() {
        for parent in [None, Some(99)] {
            let orphan = goal(0, parent, GoalKind::Milestone);
            assert!(validate_goal(&orphan, None, &tree(), &[]).is_err());
        }
    }

    #[test]
    fn a_controlled_goal_needs_an_existing_series() {
        let known = [series(1, Feed::BodyWeight)];
        let mut controlled = goal(0, Some(1), GoalKind::Controlled);
        assert!(validate_goal(&controlled, None, &tree(), &known).is_err());
        controlled.series_id = Some(7);
        assert!(validate_goal(&controlled, None, &tree(), &known).is_err());
        controlled.series_id = Some(1);
        assert!(validate_goal(&controlled, None, &tree(), &known).is_ok());
    }

    #[test]
    fn the_master_cannot_be_deleted() {
        let goals = tree();
        assert!(ensure_deletable(&goals[0]).is_err());
        assert!(ensure_deletable(&goals[1]).is_ok());
    }

    #[test]
    fn only_manual_series_take_entries() {
        assert!(ensure_manual(&series(2, Feed::TrainingHours)).is_err());
        assert!(ensure_manual(&series(3, Feed::Manual)).is_ok());
    }
}
