//! Glue between the core model and the Google clients, shared by the CLI and the TUI.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Days, NaiveDate, Utc, Weekday};

use crate::{
    calc,
    export::{self, ExportEvent, ExportOpts},
    google::{
        auth::{self, Api, Auth, DRIVE_SCOPE},
        calendar::{self, CalEvent, PushReport},
        drive::{Drive, RemoteMeta},
        health::{self, Workout},
    },
    model::{Device, Kind, Library, Plan, Store, hm},
    storage::Paths,
    sync::{self, Drift, Report, merge::Side},
};

#[must_use]
pub fn cal_events(events: &[ExportEvent]) -> Vec<CalEvent> {
    events
        .iter()
        .map(|e| CalEvent {
            item_id: e.item_id.clone(),
            summary: e.summary.clone(),
            description: e.description.clone(),
            date: e.date,
            start: e.start,
            end: e.end,
        })
        .collect()
}

/// The tapas kind of a Google workout, if it maps to one.
#[must_use]
pub fn workout_kind(w: &Workout) -> Option<Kind> {
    let key = health::map_exercise_type(&w.exercise_type)?;
    Kind::ALL.into_iter().find(|k| k.name() == key)
}

/// `run`, or the lower-cased Google type for unmapped workouts.
#[must_use]
pub fn workout_label(w: &Workout) -> String {
    workout_kind(w).map_or_else(
        || w.exercise_type.to_lowercase().replace('_', " "),
        |k| k.name().to_owned(),
    )
}

/// `bike 2h22, walking 21m`
#[must_use]
pub fn workouts_text(workouts: &[Workout]) -> String {
    workouts
        .iter()
        .map(|w| format!("{} {}", workout_label(w), hm(w.minutes.into())))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Monday of the week containing `d`.
#[must_use]
pub fn week_monday(d: NaiveDate) -> NaiveDate {
    d.week(Weekday::Mon).first_day()
}

/// Bike rides shorter than this are commutes, not training.
pub const MIN_TRAINING_RIDE_MIN: u32 = 30;
/// Bike rides below this effort are commutes. MET is Google's calories per kg of body weight per
/// hour; on real data commutes measured 5.3–5.6 and training rides 6.9–8.4.
pub const MIN_TRAINING_RIDE_MET: f64 = 6.0;

/// A bike ride too short or too easy to count as training, such as a commute. Without calories,
/// duration alone decides.
#[must_use]
pub fn is_commute(w: &Workout, weight_kg: f64) -> bool {
    if workout_kind(w) != Some(Kind::Bike) {
        return false;
    }
    if w.minutes < MIN_TRAINING_RIDE_MIN {
        return true;
    }
    w.kcal.is_some_and(|kcal| {
        kcal / weight_kg / (f64::from(w.minutes) / 60.0) < MIN_TRAINING_RIDE_MET
    })
}

/// Planned training and recorded workouts of one day.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DayDone {
    pub planned: usize,
    pub planned_min: u32,
    /// Workouts that count as training.
    pub done: Vec<Workout>,
    /// Commute rides: kept for display but never counted (see [`is_commute`]).
    pub commutes: Vec<Workout>,
}

impl DayDone {
    #[must_use]
    pub fn done_min(&self) -> u32 {
        self.done.iter().map(|w| w.minutes).sum()
    }
}

/// Training items of `plan` next to the workouts that started in the week from `monday`.
/// `weight_kg` turns calories into effort for [`is_commute`].
#[must_use]
pub fn planned_vs_done(
    lib: &Library,
    plan: &Plan,
    monday: NaiveDate,
    workouts: &[Workout],
    weight_kg: f64,
) -> [DayDone; 7] {
    let mut out: [DayDone; 7] = Default::default();
    for (d, items) in plan.days.iter().enumerate() {
        let train = items.iter().filter(|x| calc::is_train(lib, x));
        out[d].planned = train.clone().count();
        out[d].planned_min = train.map(|x| x.dur).sum();
    }
    for w in workouts {
        let day = (w.start.date_naive() - monday).num_days();
        if let Ok(d @ 0..7) = usize::try_from(day) {
            let list = if is_commute(w, weight_kg) {
                &mut out[d].commutes
            } else {
                &mut out[d].done
            };
            list.push(w.clone());
        }
    }
    out
}

#[must_use]
pub fn tokens_file(paths: &Paths, api: Api) -> PathBuf {
    paths.tokens_file(api.name())
}

#[must_use]
pub fn is_logged_in(paths: &Paths, api: Api) -> bool {
    auth::is_logged_in(&tokens_file(paths, api))
}

/// Fail with a pointer to the right command when Google is not set up yet for `api`.
pub fn require_login(paths: &Paths, api: Api) -> Result<()> {
    if !paths.client_secret_file().exists() {
        bail!("no Google OAuth client yet; run `tapas google setup <client_secret.json>`");
    }
    if !is_logged_in(paths, api) {
        bail!(
            "not logged in to Google {}; run `tapas google login --only {}`",
            api.name(),
            api.name()
        );
    }
    Ok(())
}

/// What to do when a Google call needs a new consent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prompt {
    /// Open the browser consent page (the CLI).
    Browser,
    /// Fail with the `tapas google login` hint (the TUI, background calls).
    Never,
}

async fn authenticator(paths: &Paths, api: Api, prompt: Prompt) -> Result<Auth> {
    require_login(paths, api)?;
    let (secret, tokens) = (paths.client_secret_file(), tokens_file(paths, api));
    match prompt {
        Prompt::Browser => auth::authenticator(&secret, &tokens).await,
        Prompt::Never => auth::background_authenticator(&secret, api, &tokens).await,
    }
}

/// IANA name of the local time zone.
pub fn local_tz() -> Result<String> {
    iana_time_zone::get_timezone().context("detecting the local time zone")
}

/// Replace `plan`'s events in the tapas Google calendar.
pub async fn push_plan(
    paths: &Paths,
    store: &Store,
    plan: &Plan,
    opts: &ExportOpts,
    prompt: Prompt,
) -> Result<PushReport> {
    require_login(paths, Api::Calendar)?;
    let events = cal_events(&export::events(store, plan, opts));
    if events.is_empty() {
        bail!("nothing to push");
    }
    let tz = local_tz()?;
    let auth = authenticator(paths, Api::Calendar, prompt).await?;
    calendar::push(
        &auth,
        &store.export.calendar_name,
        store.export.calendar_id.as_deref(),
        &plan.id,
        &events,
        opts.weeks,
        &tz,
    )
    .await
}

/// Latest weight in kg from Google Health over the last 90 days.
pub async fn latest_weight(paths: &Paths, prompt: Prompt) -> Result<Option<(f64, DateTime<Utc>)>> {
    let auth = authenticator(paths, Api::Health, prompt).await?;
    let token = auth::access_token(&auth, Api::Health.scopes()).await?;
    health::latest_weight_kg(&token, 90).await
}

/// Google Health workouts of the week starting `monday`.
pub async fn week_workouts(
    paths: &Paths,
    monday: NaiveDate,
    prompt: Prompt,
) -> Result<Vec<Workout>> {
    let auth = authenticator(paths, Api::Health, prompt).await?;
    let token = auth::access_token(&auth, Api::Health.scopes()).await?;
    health::workouts(&token, monday, monday + Days::new(6)).await
}

/// Why a Drive sync did not happen.
#[derive(Debug)]
pub enum SyncError {
    /// Not set up on this machine; says how to turn it on. Never prompts or retries.
    Off(String),
    /// Google is unreachable.
    Offline,
    /// The saved login no longer works: the `tapas google login` hint.
    LoginNeeded(String),
    Failed(anyhow::Error),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Off(why) => write!(f, "sync is off: {why}"),
            SyncError::Offline => f.write_str("offline: Google is unreachable"),
            SyncError::LoginNeeded(hint) => f.write_str(hint),
            SyncError::Failed(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for SyncError {}

/// Why Drive sync is off here, `None` when it is set up: it needs the OAuth client and a
/// Calendar login that was granted the Drive scope.
#[must_use]
pub fn sync_off(paths: &Paths) -> Option<String> {
    let login = "run `tapas google login --only calendar`";
    if !paths.client_secret_file().exists() {
        return Some(
            "no Google OAuth client; run `tapas google setup <client_secret.json>`".into(),
        );
    }
    let tokens = tokens_file(paths, Api::Calendar);
    if !auth::is_logged_in(&tokens) {
        return Some(format!("not logged in to Google; {login}"));
    }
    if !auth::has_scope(&tokens, DRIVE_SCOPE) {
        return Some(format!("the Google login has no Drive access; {login}"));
    }
    None
}

/// Host of the OAuth token endpoint, which every sync reaches first.
const TOKEN_ENDPOINT: &str = "oauth2.googleapis.com:443";

/// Whether Google accepts a TCP connection within 3 s.
pub async fn online() -> bool {
    let connect = tokio::net::TcpStream::connect(TOKEN_ENDPOINT);
    matches!(
        tokio::time::timeout(Duration::from_secs(3), connect).await,
        Ok(Ok(_))
    )
}

/// Sort a failed sync into offline, login needed or another error. yup-oauth2 turns a failed
/// token refresh into a refused consent whether the token was revoked or the network is down,
/// so only a probe of Google tells the two apart.
async fn classify(err: anyhow::Error) -> SyncError {
    if !online().await {
        return SyncError::Offline;
    }
    match err.downcast::<auth::NeedsLogin>() {
        Ok(hint) => SyncError::LoginNeeded(hint.0),
        Err(e) => SyncError::Failed(e),
    }
}

/// A Drive client on the Calendar login that never opens a browser.
async fn drive(paths: &Paths) -> Result<Drive> {
    let tokens = tokens_file(paths, Api::Calendar);
    let secret = paths.client_secret_file();
    Drive::new(auth::background_authenticator(&secret, Api::Calendar, &tokens).await?)
}

/// [`sync::sync`], or [`sync::keep`] with `keep`, against Google Drive.
pub async fn sync_store(
    paths: &Paths,
    store: &mut Store,
    device: &Device,
    keep: Option<Side>,
) -> Result<Report, SyncError> {
    if let Some(why) = sync_off(paths) {
        return Err(SyncError::Off(why));
    }
    let res = async {
        let drive = drive(paths).await?;
        match keep {
            None => sync::sync(paths, &drive, store, device).await,
            Some(side) => sync::keep(paths, &drive, store, device, side).await,
        }
    }
    .await;
    match res {
        Ok(r) => Ok(r),
        Err(e) => Err(classify(e).await),
    }
}

/// [`sync::remote_status`] against Google Drive.
pub async fn sync_status(paths: &Paths) -> Result<(Drift, Vec<RemoteMeta>), SyncError> {
    if let Some(why) = sync_off(paths) {
        return Err(SyncError::Off(why));
    }
    let res = async { sync::remote_status(paths, &drive(paths).await?).await }.await;
    match res {
        Ok(r) => Ok(r),
        Err(e) => Err(classify(e).await),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Local, TimeZone};

    use super::*;
    use crate::library::default_library;

    fn ride(ty: &str, day: u32, minutes: u32, kcal: Option<f64>) -> Workout {
        Workout {
            exercise_type: ty.into(),
            start: Local.with_ymd_and_hms(2026, 9, day, 12, 0, 0).unwrap(),
            minutes,
            kcal,
        }
    }

    // Real rides from Google Health, weight 71.1 kg.
    #[test]
    fn commute_rides_by_duration_and_effort() {
        let kg = 71.1;
        // Short rides are commutes whatever the effort, and without calories.
        assert!(is_commute(&ride("BIKING", 23, 15, Some(95.0)), kg));
        assert!(is_commute(&ride("BIKING", 23, 11, Some(88.0)), kg));
        assert!(is_commute(&ride("BIKING", 6, 25, None), kg));
        // 33 min but 5.6 MET: a commute.
        assert!(is_commute(&ride("BIKING", 6, 33, Some(219.0)), kg));
        // Training rides: 6.9, 8.4 and 7.5 MET.
        assert!(!is_commute(&ride("BIKING", 18, 149, Some(1220.0)), kg));
        assert!(!is_commute(&ride("BIKING", 16, 56, Some(557.0)), kg));
        assert!(!is_commute(&ride("BIKING", 22, 142, Some(1270.0)), kg));
        // 30 min or more without calories counts.
        assert!(!is_commute(&ride("BIKING", 6, 30, None), kg));
        // Only bikes: a short walk or run is never a commute ride.
        assert!(!is_commute(&ride("WALKING", 22, 21, Some(164.0)), kg));
        assert!(!is_commute(&ride("RUNNING", 22, 20, Some(150.0)), kg));
    }

    #[test]
    fn commutes_are_kept_but_not_counted() {
        let lib = default_library();
        let plan = Plan::new("Week");
        let monday = NaiveDate::from_ymd_opt(2026, 9, 21).unwrap();
        let workouts = [
            ride("BIKING", 22, 142, Some(1270.0)),
            ride("WALKING", 22, 21, Some(164.0)),
            ride("BIKING", 23, 15, Some(95.0)),
            ride("BIKING", 23, 14, Some(109.0)),
        ];
        let days = planned_vs_done(&lib, &plan, monday, &workouts, 71.1);
        assert_eq!(days[1].done.len(), 2);
        assert_eq!(days[1].done_min(), 163);
        assert!(days[1].commutes.is_empty());
        assert!(days[2].done.is_empty());
        assert_eq!(days[2].done_min(), 0);
        assert_eq!(days[2].commutes.len(), 2);
        assert_eq!(workouts_text(&days[2].commutes), "bike 15m, bike 14m");
    }
}
