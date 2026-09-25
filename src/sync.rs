//! Glue between the core model and the Google clients, shared by the CLI and the TUI.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Days, NaiveDate, Utc, Weekday};

use crate::{
    calc,
    export::{self, ExportEvent, ExportOpts},
    google::{
        auth::{self, ACTIVITY_SCOPE, Auth, METRICS_SCOPE},
        calendar::{self, CalEvent, PushReport},
        health::{self, Workout},
    },
    model::{Kind, Library, Plan, Store},
    storage::Paths,
};

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
pub fn workout_kind(w: &Workout) -> Option<Kind> {
    let key = health::map_exercise_type(&w.exercise_type)?;
    Kind::ALL.into_iter().find(|k| k.name() == key)
}

/// `run`, or the lower-cased Google type for unmapped workouts.
pub fn workout_label(w: &Workout) -> String {
    workout_kind(w).map_or_else(
        || w.exercise_type.to_lowercase().replace('_', " "),
        |k| k.name().to_owned(),
    )
}

/// Monday of the week containing `d`.
pub fn week_monday(d: NaiveDate) -> NaiveDate {
    d.week(Weekday::Mon).first_day()
}

/// Planned training and recorded workouts of one day.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DayDone {
    pub planned: usize,
    pub planned_min: u32,
    pub done: Vec<Workout>,
}

impl DayDone {
    pub fn done_min(&self) -> u32 {
        self.done.iter().map(|w| w.minutes).sum()
    }
}

/// Training items of `plan` next to the workouts that started in the week from `monday`.
pub fn planned_vs_done(
    lib: &Library,
    plan: &Plan,
    monday: NaiveDate,
    workouts: &[Workout],
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
            out[d].done.push(w.clone());
        }
    }
    out
}

/// Fail with a pointer to the right command when Google is not set up yet.
pub fn require_login(paths: &Paths) -> Result<()> {
    if !paths.client_secret_file().exists() {
        bail!("no Google OAuth client yet; run `tapas google setup <client_secret.json>`");
    }
    if !auth::is_logged_in(&paths.tokens_file()) {
        bail!("not logged in to Google; run `tapas google login`");
    }
    Ok(())
}

async fn authenticator(paths: &Paths) -> Result<Auth> {
    require_login(paths)?;
    auth::authenticator(&paths.client_secret_file(), &paths.tokens_file()).await
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
) -> Result<PushReport> {
    require_login(paths)?;
    let events = cal_events(&export::events(store, plan, opts));
    if events.is_empty() {
        bail!("nothing to push");
    }
    let tz = local_tz()?;
    let auth = authenticator(paths).await?;
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
pub async fn latest_weight(paths: &Paths) -> Result<Option<(f64, DateTime<Utc>)>> {
    let auth = authenticator(paths).await?;
    let token = auth::access_token(&auth, &[METRICS_SCOPE]).await?;
    health::latest_weight_kg(&token, 90).await
}

/// Google Health workouts of the week starting `monday`.
pub async fn week_workouts(paths: &Paths, monday: NaiveDate) -> Result<Vec<Workout>> {
    let auth = authenticator(paths).await?;
    let token = auth::access_token(&auth, &[ACTIVITY_SCOPE]).await?;
    health::workouts(&token, monday, monday + Days::new(6)).await
}
