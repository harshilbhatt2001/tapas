//! Screen 5: file export, Google Calendar push and Google Health workouts.

use std::path::PathBuf;

use anyhow::Result;
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
    app::{App, BgResult, FormKind, Screen, Task},
    widgets::{DIM, Field, Form, LINE, OK, WARN},
};
use crate::{
    export::{self, ExportOpts},
    google::{auth::Api, calendar::PushReport, health::Workout},
    services, storage,
};

pub fn opts(app: &App) -> ExportOpts {
    ExportOpts {
        first_monday: app.first_monday,
        weeks: app.store.export.weeks,
        include_life: app.store.export.include_life,
    }
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
                    Field::text("Calendar name", s.calendar_name.clone()),
                ],
            );
            app.open_form(form, FormKind::Export);
        }
        KeyCode::Char('i') => write(app, "ics"),
        KeyCode::Char('c') => write(app, "csv"),
        KeyCode::Char('g') => push(app),
        KeyCode::Char('f') => fetch_workouts(app),
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
    let name = form.text(3);
    if name.is_empty() {
        return Err("Calendar name: can't be empty".into());
    }
    app.first_monday = monday;
    let s = &mut app.store.export;
    s.weeks = weeks;
    s.include_life = form.toggle(2);
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

/// `tapas-base-week.ics`
fn file_name(plan: &str, ext: &str) -> String {
    format!("tapas-{}.{ext}", slug::slugify(plan))
}

fn write(app: &mut App, ext: &str) {
    let res = (|| -> Result<PathBuf> {
        let plan = app.plan();
        let o = opts(app);
        let events = export::events(&app.store, plan, &o);
        anyhow::ensure!(!events.is_empty(), "nothing to export");
        let text = if ext == "ics" {
            export::to_ics(plan, &events, o.weeks)
        } else {
            export::to_google_csv(&events, o.weeks)?
        };
        let path = out_dir().join(file_name(&plan.name, ext));
        storage::write_atomic(&path, text.as_bytes())?;
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

fn push(app: &mut App) {
    if let Err(e) = services::require_login(&app.paths, Api::Calendar) {
        app.error(format!("{e:#}"));
        return;
    }
    let (paths, store, o) = (app.paths.clone(), app.store.clone(), opts(app));
    let plan = app.plan().clone();
    app.spawn(Task::Push, async move {
        let res = services::push_plan(&paths, &store, &plan, &o).await;
        BgResult::Pushed {
            plan_id: plan.id,
            res,
        }
    });
}

pub fn on_pushed(app: &mut App, plan_id: &str, res: Result<PushReport>) {
    match res {
        Ok(r) => {
            let name = app
                .store
                .plans
                .iter()
                .find(|p| p.id == plan_id)
                .map_or("plan", |p| p.name.as_str())
                .to_owned();
            app.store.export.calendar_id = Some(r.calendar_id);
            app.commit();
            app.info(format!(
                "Pushed {name} to \"{}\": {} created, {} replaced",
                app.store.export.calendar_name, r.created, r.deleted
            ));
        }
        Err(e) => app.error(format!("Push: {e:#}")),
    }
}

fn fetch_workouts(app: &mut App) {
    if let Err(e) = services::require_login(&app.paths, Api::Health) {
        app.error(format!("{e:#}"));
        return;
    }
    let monday = services::week_monday(Local::now().date_naive());
    let paths = app.paths.clone();
    app.spawn(Task::Workouts, async move {
        let res = services::week_workouts(&paths, monday).await;
        BgResult::Workouts { monday, res }
    });
}

pub fn on_workouts(app: &mut App, monday: NaiveDate, res: Result<Vec<Workout>>) {
    match res {
        Ok(w) => {
            let weight = app.store.profile.weight;
            let commutes = w.iter().filter(|x| services::is_commute(x, weight)).count();
            app.info(format!(
                "{} workouts and {commutes} commute rides this week from Google Health; see the \
                 Done lines on screen 1 (c shows commutes)",
                w.len() - commutes
            ));
            app.done = Some((monday, w));
            app.screen = Screen::Week;
        }
        Err(e) => app.error(format!("Workouts: {e:#}")),
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

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let s = &app.store.export;
    let [table, text] = Layout::vertical([Constraint::Length(7), Constraint::Fill(1)]).areas(area);
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
                .title(format!(" Export {} ", app.plan().name))
                .title_bottom(
                    Line::from(
                        " e edit · i write .ics · c write CSV · g push to Google Calendar · f fetch workouts ",
                    )
                    .fg(DIM),
                )
                .border_style(Style::new().fg(LINE)),
        ),
        table,
    );
    let mut body = vec![google_state(app), Line::raw("")];
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
        "Push: tapas keeps its own calendar and replaces this plan's events on every push.",
        Style::new().fg(DIM),
    ));
    f.render_widget(
        Paragraph::new(body).wrap(Wrap { trim: true }),
        text.inner(ratatui::layout::Margin::new(1, 1)),
    );
}
