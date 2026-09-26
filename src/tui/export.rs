//! Screen 5: file export, Google Calendar push and Google login state.

use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use chrono::{Datelike, Local, NaiveDate, Weekday};
use directories::UserDirs;
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent},
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Paragraph, Row, Table, Wrap},
};

use super::{
    app::{App, BgResult, FormKind, SyncStatus, Task},
    widgets::{DIM, Field, Form, LINE, OK, WARN},
};
use crate::{
    export,
    google::{auth::Api, calendar::PushReport},
    health_cache,
    model::{DAY_FMT, week_monday},
    services::{self, Prompt},
};

/// Monday of the first exported week.
fn from_monday(app: &App) -> NaiveDate {
    week_monday(app.first_monday)
}

pub fn on_key(app: &mut App, k: KeyEvent) {
    match k.code {
        KeyCode::Enter | KeyCode::Char('e') => {
            let s = &app.store.export;
            let form = Form::new(
                "Export settings",
                Color::White,
                vec![
                    Field::text("First Monday", app.first_monday.to_string()),
                    Field::text("Weeks 1-52", s.weeks.to_string()),
                    Field::toggle("Work & commute", s.include_life),
                    Field::toggle("Auto-push actuals", s.auto_push_actuals),
                    Field::text("Calendar name", s.calendar_name.clone()),
                ],
            );
            app.open_form(form, FormKind::Export);
        }
        KeyCode::Char('i') => write(app, "ics"),
        KeyCode::Char('c') => write(app, "csv"),
        KeyCode::Char('g') => {
            let (from, weeks) = (from_monday(app), app.store.export.weeks);
            start_push_weeks(app, from, weeks, false);
        }
        KeyCode::Char('s') => super::sync::start(app, true),
        _ => {}
    }
}

pub fn apply(app: &mut App, form: &Form) -> Result<(), String> {
    let monday: NaiveDate = form
        .text(0)
        .parse()
        .map_err(|_| "First Monday: use YYYY-MM-DD".to_string())?;
    if monday.weekday() != Weekday::Mon {
        return Err(format!("First Monday: {monday} is a {}", monday.weekday()));
    }
    let weeks: u32 = form.parse(1)?;
    if !(1..=52).contains(&weeks) {
        return Err("Weeks 1-52: between 1 and 52".into());
    }
    let name = form.text(4);
    if name.is_empty() {
        return Err("Calendar name: can't be empty".into());
    }
    app.first_monday = monday;
    let s = &mut app.store.export;
    s.weeks = weeks;
    s.include_life = form.toggle(2);
    s.auto_push_actuals = form.toggle(3);
    if s.calendar_name != name {
        // A different name means a different calendar next push.
        name.clone_into(&mut s.calendar_name);
        s.calendar_id = None;
    }
    app.commit();
    Ok(())
}

/// The XDG download dir, or the working directory.
fn out_dir() -> PathBuf {
    UserDirs::new()
        .and_then(|d| d.download_dir().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn write(app: &mut App, ext: &str) {
    let res = (|| -> Result<PathBuf> {
        let (from, s) = (from_monday(app), &app.store.export);
        let events = export::range_events(
            &app.store,
            from,
            s.weeks,
            |monday| health_cache::load(&app.paths, monday),
            Local::now().date_naive(),
            s.include_life,
        );
        anyhow::ensure!(!events.is_empty(), "nothing to export");
        let text = if ext == "ics" {
            export::to_ics(&s.calendar_name, &events)
        } else {
            export::to_google_csv(&events)?
        };
        let path = out_dir().join(format!("tapas-{from}.{ext}"));
        fs::write(&path, &text).with_context(|| format!("writing {}", path.display()))?;
        Ok(path)
    })();
    match res {
        Ok(p) => {
            app.info(format!("Wrote {}", p.display()));
            app.last_file = Some(p);
        }
        Err(e) => app.error(format!("Export: {e:#}")),
    }
}

/// Push the `weeks` dated weeks from `from` to Google Calendar in the background; `quiet`
/// reports only a failure.
pub fn start_push_weeks(app: &mut App, from: NaiveDate, weeks: u32, quiet: bool) {
    if let Err(e) = services::require_login(&app.paths, Api::Calendar) {
        app.error(format!("{e:#}"));
        return;
    }
    let (paths, store) = (app.paths.clone(), app.store.clone());
    app.spawn(Task::Push, async move {
        let res = services::push_weeks(&paths, &store, from, weeks, Prompt::Never).await;
        BgResult::Pushed { quiet, res }
    });
}

pub fn on_pushed(app: &mut App, quiet: bool, res: Result<PushReport>) {
    match res {
        Ok(r) => {
            if app.store.export.calendar_id.as_ref() != Some(&r.calendar_id) {
                app.store.export.calendar_id = Some(r.calendar_id);
                app.commit();
            }
            if !quiet {
                app.info(format!(
                    "Pushed to \"{}\": {} new, {} updated, {} removed, {} unchanged",
                    app.store.export.calendar_name, r.inserted, r.updated, r.deleted, r.unchanged
                ));
            }
        }
        Err(e) => app.error(format!("Push: {e:#}")),
    }
}

/// One line on whether Google is usable.
pub fn google_state(app: &App) -> Line<'static> {
    if app.paths.client_secret_file().exists() {
        let mut spans = vec![Span::raw("Google: ")];
        let mut missing = Vec::new();
        for api in Api::ALL {
            let ok = services::is_logged_in(&app.paths, api);
            if !ok {
                missing.push(api.name());
            }
            let (mark, color) = if ok { ("✓", OK) } else { ("✗", WARN) };
            spans.push(Span::styled(
                format!("{} {mark}  ", api.name()),
                Style::new().fg(color),
            ));
        }
        if let [one] = missing[..] {
            spans.push(Span::raw(format!(
                "Run `tapas google login --only {one}` in a terminal."
            )));
        } else if !missing.is_empty() {
            spans.push(Span::raw("Run `tapas google login` in a terminal."));
        }
        Line::from(spans)
    } else {
        Line::from(vec![
            Span::styled("Google: not set up. ", Style::new().fg(WARN)),
            Span::raw("Run `tapas google setup <client_secret.json>`, then `tapas google login`."),
        ])
    }
}

/// `1 week`, `4 weeks`
fn weeks_text(weeks: u32) -> String {
    if weeks == 1 {
        "1 week".into()
    } else {
        format!("{weeks} weeks")
    }
}

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let s = &app.store.export;
    let [table, text] = Layout::vertical([Constraint::Length(8), Constraint::Fill(1)]).areas(area);
    let rows = [
        ("First Monday", app.first_monday.to_string()),
        ("Weeks", s.weeks.to_string()),
        (
            "Work & commute",
            if s.include_life {
                "included"
            } else {
                "left out"
            }
            .into(),
        ),
        (
            "Auto-push",
            if s.auto_push_actuals {
                "actuals after each Health fetch"
            } else {
                "off"
            }
            .into(),
        ),
        ("Calendar name", s.calendar_name.clone()),
        (
            "Calendar id",
            s.calendar_id
                .clone()
                .unwrap_or_else(|| "created on first push".into()),
        ),
    ]
    .map(|(k, v)| Row::new(vec![k.to_owned(), v]));
    f.render_widget(
        Table::new(rows, [Constraint::Length(16), Constraint::Fill(1)]).block(
            Block::bordered()
                .title(format!(
                    " Export {} from {} ",
                    weeks_text(s.weeks),
                    from_monday(app).format(DAY_FMT)
                ))
                .title_bottom(
                    Line::from(
                        " e edit · i write .ics · c write CSV · g push to Google Calendar · s sync",
                    )
                    .fg(DIM),
                )
                .border_style(Style::new().fg(LINE)),
        ),
        table,
    );
    let mut sync = super::sync::status_line(app);
    if let SyncStatus::Off(why) = &app.sync {
        sync.spans.push(Span::raw(format!(" ({why})")));
    }
    let mut body = vec![google_state(app), sync, Line::raw("")];
    body.push(Line::raw(format!("Files go to {}.", out_dir().display())));
    if let Some(p) = &app.last_file {
        body.push(Line::from(vec![
            Span::raw("Last written: "),
            Span::styled(p.display().to_string(), Style::new().bold()),
        ]));
    }
    body.push(Line::raw(""));
    body.push(Line::styled(
        "CSV: in Google Calendar, Settings, Import & export, Import, pick the file and a calendar. Use a separate \"Training\" calendar so you can delete and re-import cleanly.",
        Style::new().fg(DIM),
    ));
    body.push(Line::styled(
        "Push: tapas keeps its own calendar with one event per session of each week, updating only what changed; past days show what Google Health recorded.",
        Style::new().fg(DIM),
    ));
    f.render_widget(
        Paragraph::new(body).wrap(Wrap { trim: true }),
        text.inner(ratatui::layout::Margin::new(1, 1)),
    );
}
