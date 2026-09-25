//! Drive sync from the TUI: at start, 3 s after the last edit, on `s` (screen 5) and on quit.
//! Runs as `Task::Sync` in the background; offline retries every 60 s.

use std::{
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};

use chrono::Local;
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use super::{
    app::{App, BgResult, SyncStatus, Task},
    widgets::{DIM, OK, WARN},
};
use crate::{
    model::Store,
    services::{self, SyncError},
    storage,
    sync::{Outcome, Report, merge},
};

/// Push this long after the last edit, so a burst of edits is one upload.
pub const PUSH_DELAY: Duration = Duration::from_secs(3);
/// Retry this often while offline.
pub const RETRY: Duration = Duration::from_secs(60);
/// The longest quitting waits for a last sync.
pub const QUIT_TIMEOUT: Duration = Duration::from_secs(5);

fn is_off(app: &App) -> bool {
    matches!(app.sync, SyncStatus::Off(_))
}

/// A commit saved local edits: push them after [`PUSH_DELAY`].
pub fn edited(app: &mut App) {
    app.unsynced = true;
    if !is_off(app) {
        app.push_at = Some(Instant::now() + PUSH_DELAY);
    }
}

/// Start a background sync unless one is running. When sync is off only a `manual` start looks
/// again (the user may have logged in meanwhile) and says why it is off.
pub fn start(app: &mut App, manual: bool) {
    if manual && is_off(app) {
        app.sync = services::sync_off(&app.paths).map_or(SyncStatus::Idle, SyncStatus::Off);
    }
    if let SyncStatus::Off(why) = &app.sync {
        if manual {
            app.error(format!("Sync is off: {why}"));
        }
        return;
    }
    if app.busy.contains(&Task::Sync) {
        if manual {
            app.info("Already syncing with Google Drive…");
        }
        return;
    }
    app.push_at = None;
    app.retry_at = None;
    let (paths, device, sent) = (app.paths.clone(), app.device.clone(), app.store.clone());
    app.spawn(Task::Sync, async move {
        let mut store = sent.clone();
        let res = services::sync_store(&paths, &mut store, &device, None).await;
        BgResult::Synced {
            manual,
            sent: Box::new(sent),
            store: Box::new(store),
            res,
        }
    });
}

/// Called from the event loop: start a sync that is due.
pub fn tick(app: &mut App, now: Instant) {
    let due = |at: Option<Instant>| at.is_some_and(|at| now >= at);
    if (due(app.push_at) || due(app.retry_at)) && !app.busy.contains(&Task::Sync) {
        start(app, false);
    }
}

fn summary(report: &Report) -> String {
    let text = match &report.outcome {
        Outcome::UpToDate => "Up to date with Google Drive".to_owned(),
        Outcome::Pushed => "Pushed to Google Drive".to_owned(),
        Outcome::Pulled => "Pulled changes from Google Drive".to_owned(),
        Outcome::Created => "Uploaded this store to Google Drive".to_owned(),
        Outcome::Merged { conflicts } if conflicts.is_empty() => {
            "Merged changes from another machine".to_owned()
        }
        Outcome::Merged { conflicts } => format!(
            "Merged changes from another machine; {} changed on both sides, the newer edit won \
             (`tapas sync status`, old copy in the sync dir)",
            conflicts.len()
        ),
        Outcome::Kept(_) => "Replaced one side".to_owned(),
    };
    match report.warnings.first() {
        Some(w) => format!("{text}; {w}"),
        None => text,
    }
}

/// Take in a finished sync. `sent` is the store it started from and `store` the one it left
/// on disk; edits made while it ran are merged on top and pushed again.
pub fn on_synced(
    app: &mut App,
    manual: bool,
    sent: &Store,
    store: &Store,
    res: Result<Report, SyncError>,
) {
    let report = match res {
        Ok(r) => r,
        Err(SyncError::Offline) => {
            app.sync = SyncStatus::Offline;
            app.retry_at = Some(Instant::now() + RETRY);
            if manual {
                app.info("Offline: edits stay here and sync when Google is reachable");
            }
            return;
        }
        Err(SyncError::Off(why)) => {
            app.sync = SyncStatus::Off(why);
            return;
        }
        Err(SyncError::LoginNeeded(hint)) => {
            app.sync = SyncStatus::LoginNeeded;
            app.error(format!("Sync: {hint}"));
            return;
        }
        Err(SyncError::Failed(e)) => {
            app.sync = SyncStatus::Error;
            app.error(format!("Sync: {e:#}"));
            return;
        }
    };
    let edited = app.store != *sent;
    if store != sent || edited {
        // The sync wrote the store file, and so may a commit that ran meanwhile: write the
        // result of both once more.
        let saved = if edited {
            let m = merge::merge(sent, &app.store, store, app.device.device_id, None);
            app.store = m.store;
            app.store.normalize();
            storage::save(&app.paths, &mut app.store)
        } else {
            app.store = store.clone();
            storage::write_store(&app.paths, &app.store)
        };
        app.clamp();
        if let Err(e) = saved {
            app.error(format!("Save failed: {e:#}"));
        }
    }
    app.unsynced = app.store != *store;
    if app.unsynced {
        app.push_at = Some(Instant::now() + PUSH_DELAY);
    }
    app.sync = match &report.outcome {
        Outcome::Merged { conflicts } if !conflicts.is_empty() => {
            SyncStatus::Merged(conflicts.len())
        }
        _ => SyncStatus::Synced(Local::now()),
    };
    let notable = matches!(report.outcome, Outcome::Merged { .. } | Outcome::Pulled)
        || !report.warnings.is_empty();
    if manual || notable {
        app.info(summary(&report));
    }
}

/// On quit, give unsynced edits (and a sync already running) up to [`QUIT_TIMEOUT`]. Returns
/// what to tell the user when edits stay behind.
pub fn on_quit(app: &mut App, rx: &Receiver<BgResult>) -> Option<String> {
    if is_off(app) || !(app.unsynced || app.busy.contains(&Task::Sync)) {
        return None;
    }
    let deadline = Instant::now() + QUIT_TIMEOUT;
    let mut started = false;
    loop {
        if !app.busy.contains(&Task::Sync) {
            if !app.unsynced || started {
                break;
            }
            start(app, false);
            started = true;
            if !app.busy.contains(&Task::Sync) {
                break;
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(r) => app.on_bg(r),
            Err(_) => {
                return Some(
                    "tapas: Google Drive sync timed out; edits are saved here and sync next time"
                        .into(),
                );
            }
        }
    }
    app.unsynced.then(|| {
        let why = match app.sync {
            SyncStatus::Offline => "offline",
            SyncStatus::LoginNeeded => "login needed",
            _ => "sync failed",
        };
        format!("tapas: {why}; edits are saved here and go to Google Drive next time")
    })
}

/// `Sync: synced 14:05`, for the header.
pub fn status_line(app: &App) -> Line<'static> {
    let (text, color) = if app.busy.contains(&Task::Sync) {
        ("syncing…".to_owned(), Color::White)
    } else {
        match &app.sync {
            SyncStatus::Off(_) => ("off".to_owned(), DIM),
            SyncStatus::Idle => ("not synced yet".to_owned(), DIM),
            SyncStatus::Synced(at) => (format!("synced {}", at.format("%H:%M")), OK),
            SyncStatus::Merged(n) => (format!("merged {n}"), WARN),
            SyncStatus::Offline => ("offline".to_owned(), WARN),
            SyncStatus::LoginNeeded => ("login needed".to_owned(), WARN),
            SyncStatus::Error => ("error".to_owned(), WARN),
        }
    };
    let mut spans = vec![
        Span::styled("Sync: ", Style::new().fg(DIM)),
        Span::styled(text, Style::new().fg(color)),
    ];
    if app.unsynced && !is_off(app) && !app.busy.contains(&Task::Sync) {
        spans.push(Span::styled("  unsynced changes", Style::new().fg(DIM)));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{google::auth::DRIVE_SCOPE, model::Device, storage::Paths};

    /// An app whose sync is set up (client secret and a token with the Drive scope).
    fn app(dir: &std::path::Path) -> App {
        let paths = Paths::under(dir);
        storage::write_atomic(&paths.client_secret_file(), b"{}").unwrap();
        let token = format!(r#"[{{"scopes":["{DRIVE_SCOPE}"],"token":{{}}}}]"#);
        storage::write_atomic(&paths.tokens_file("calendar"), token.as_bytes()).unwrap();
        App::new(paths, Store::default(), Device::default())
    }

    fn report(outcome: Outcome) -> Report {
        Report {
            outcome,
            conflict_files: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn off_without_setup_and_never_scheduled() {
        let mut a = App::new(
            Paths::under(std::path::Path::new("/nonexistent/tapas")),
            Store::default(),
            Device::default(),
        );
        assert!(matches!(a.sync, SyncStatus::Off(_)));
        edited(&mut a);
        assert_eq!(a.push_at, None);
        start(&mut a, true);
        assert!(a.status.error);
        assert!(a.busy.is_empty());
    }

    #[test]
    fn edits_schedule_a_push_and_offline_retries() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = app(dir.path());
        assert_eq!(a.sync, SyncStatus::Idle);
        let before = Instant::now();
        edited(&mut a);
        assert!(a.push_at.unwrap() >= before + PUSH_DELAY);

        let sent = a.store.clone();
        let before = Instant::now();
        on_synced(&mut a, false, &sent, &sent, Err(SyncError::Offline));
        assert_eq!(a.sync, SyncStatus::Offline);
        assert!(a.retry_at.unwrap() >= before + RETRY);
        assert!(!a.status.error);
    }

    #[test]
    fn pulled_store_replaces_local_and_edits_meanwhile_merge_in() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = app(dir.path());
        let sent = a.store.clone();
        let mut pulled = sent.clone();
        pulled.plans[0].name = "From Drive".into();
        pulled.plans[0].updated_at = chrono::Utc::now();
        // Clean: take it as it is, nothing left to push.
        on_synced(&mut a, false, &sent, &pulled, Ok(report(Outcome::Pulled)));
        assert_eq!(a.store, pulled);
        assert!(!a.unsynced && a.push_at.is_none());
        assert!(matches!(a.sync, SyncStatus::Synced(_)));
        assert_eq!(storage::load(&a.paths).unwrap().0, pulled);

        // Edited while the sync ran: both apply, and the result is pushed again.
        let mut a = app(dir.path());
        a.store.profile.weight = 70.0;
        on_synced(&mut a, false, &sent, &pulled, Ok(report(Outcome::Pulled)));
        assert_eq!(a.store.plans[0].name, "From Drive");
        assert!((a.store.profile.weight - 70.0).abs() < f64::EPSILON);
        assert!(a.unsynced && a.push_at.is_some());
    }
}
