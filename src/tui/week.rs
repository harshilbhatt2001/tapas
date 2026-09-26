//! Screen 1: the week board, checks and discipline bars; for dated weeks, planned against done.

use std::collections::HashSet;

use chrono::{Datelike, Days, Local, NaiveDate, NaiveTime};
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent},
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    symbols::border,
    text::{Line, Span},
    widgets::{Block, Borders, LineGauge, List, ListItem, ListState, Paragraph, Wrap},
};
use uuid::Uuid;

use super::{
    app::{Action, App, BgResult, Board, FormKind, Hit, Modal, Picker, Task},
    editor, plans,
    widgets::{DIM, Field, Form, LINE, OK, SURFACE, WARN, cells, frame_popup, hex, popup_area},
};
use crate::{
    actuals::{self, MISSED_ON_PURPOSE, Slot, TypeTotals},
    calc::{self, Level},
    google::{auth::Api, health::Workout},
    health_cache::{self, WeekActuals},
    library,
    model::{DAY_FMT, DAYS, Item, Library, TIME_FMT, hm, week_monday, week_title},
    services::{self, Prompt},
    zones::{self, Effort},
};

/// Below this width the board shows one day at a time.
pub const WIDE: u16 = 140;

#[must_use]
pub fn this_monday() -> NaiveDate {
    week_monday(Local::now().date_naive())
}

/// `Week of Mon 21 Sep 2026 · from "Build 2"`, or `Template: Build 2`.
#[must_use]
pub fn board_title(app: &App) -> String {
    let name = &app.plan().name;
    match app.board_monday() {
        Some(monday) => week_title(monday, name),
        None => format!("Template: {name}"),
    }
}

/// A dated week on the board with no stored `Week` yet.
fn is_unstored_week(app: &App) -> bool {
    app.board_monday()
        .is_some_and(|m| app.store.week(m).is_none())
}

/// The board one week before or after the shown one; from a template, this week.
fn step_week(app: &mut App, forward: bool) {
    let monday = match app.board_monday() {
        Some(m) if forward => m.checked_add_days(Days::new(7)),
        Some(m) => m.checked_sub_days(Days::new(7)),
        None => Some(this_monday()),
    };
    if let Some(m) = monday {
        app.set_board(Board::Week(m));
    }
}

/// Fetch the week of `monday` from Google Health in the background when its cache is stale,
/// Health is logged in and this session has not fetched it yet.
pub fn auto_fetch_actuals(app: &mut App, monday: NaiveDate) {
    let fetch = &app.actuals_fetch;
    if fetch.started.contains(&monday) || fetch.running == Some(monday) {
        return;
    }
    let cached = app
        .actuals
        .as_ref()
        .filter(|(m, _)| *m == monday)
        .map(|(_, a)| a);
    if health_cache::needs_fetch(cached, monday, Local::now())
        && services::is_logged_in(&app.paths, Api::Health)
    {
        request_actuals(app, monday);
    }
}

/// Fetch the week of `monday` now, or after the running fetch in place of any week waiting.
fn request_actuals(app: &mut App, monday: NaiveDate) {
    let fetch = &mut app.actuals_fetch;
    if fetch.running.is_none() {
        start_actuals_fetch(app, monday);
    } else if fetch.running != Some(monday) {
        fetch.queued = Some(monday);
    }
}

fn start_actuals_fetch(app: &mut App, monday: NaiveDate) {
    let paths = app.paths.clone();
    app.spawn(Task::Workouts, async move {
        let res = services::fetch_week_actuals(&paths, monday, Prompt::Never).await;
        BgResult::Workouts { monday, res }
    });
    if app.busy.contains(&Task::Workouts) {
        app.actuals_fetch.running = Some(monday);
        app.actuals_fetch.started.insert(monday);
    }
}

/// `r`: fetch the dated week on the board again.
fn refetch_actuals(app: &mut App) {
    let Some(monday) = app.board_monday() else {
        app.error("Templates have no Google Health data; t goes to this week");
        return;
    };
    if let Err(e) = services::require_login(&app.paths, Api::Health) {
        app.error(format!("{e:#}"));
        return;
    }
    request_actuals(app, monday);
}

/// A finished fetch is already in the cache; show it when its week is on the board, then
/// start the week waiting.
pub fn on_actuals(app: &mut App, monday: NaiveDate, res: anyhow::Result<WeekActuals>) {
    app.actuals_fetch.running = None;
    match res {
        Ok(actuals) if app.board_monday() == Some(monday) => {
            let weight = app.store.profile.weight;
            let w = &actuals.workouts;
            let commutes = w.iter().filter(|x| services::is_commute(x, weight)).count();
            app.info(format!(
                "Google Health: {} workouts and {commutes} commute rides this week (c shows \
                 commutes)",
                w.len() - commutes
            ));
            app.actuals = Some((monday, actuals));
        }
        Ok(_) => {}
        Err(e) => app.error(format!("Google Health, week of {monday}: {e:#}")),
    }
    if let Some(next) = app.actuals_fetch.queued.take() {
        start_actuals_fetch(app, next);
    }
}

fn open_go_to(app: &mut App) {
    let shown = app
        .board_monday()
        .and_then(|m| m.checked_add_days(Days::new(app.day as u64)))
        .unwrap_or_else(|| Local::now().date_naive());
    let form = Form::new(
        "Go to date",
        Color::White,
        vec![Field::text("Date YYYY-MM-DD", shown.to_string())],
    );
    app.open_form(form, FormKind::GoTo);
}

/// Show the week of the form's date with that day selected.
pub fn go_to(app: &mut App, form: &Form) -> Result<(), String> {
    let date: NaiveDate = form
        .text(0)
        .parse()
        .map_err(|_| "Date: use YYYY-MM-DD".to_string())?;
    app.set_board(Board::Week(week_monday(date)));
    app.day = date.weekday().num_days_from_monday() as usize;
    app.card = 0;
    app.clamp();
    Ok(())
}

fn open_template_picker(app: &mut App) {
    if app.board_monday().is_none() {
        app.error("Templates apply to dated weeks; t goes to this week");
        return;
    }
    let name = &app.plan().name;
    let selected = app
        .store
        .plans
        .iter()
        .position(|p| p.name == *name)
        .unwrap_or(0);
    app.modal = Modal::Picker(
        Picker::Template,
        ListState::default().with_selected(Some(selected)),
    );
}

/// Apply template `i` to the week on the board, asking first when that replaces sessions.
pub fn pick_template(app: &mut App, i: usize) {
    let (Some(monday), Some(t)) = (app.board_monday(), app.store.plans.get(i)) else {
        return;
    };
    let template = t.id.clone();
    let n: usize = app.plan().days.iter().map(Vec::len).sum();
    if n == 0 {
        plans::apply_template(app, &template, monday, 1);
    } else {
        let msg = format!("Replace the {n} sessions of this week with \"{}\"?", t.name);
        app.confirm(
            msg,
            Action::ApplyTemplate {
                template,
                from: monday,
                weeks: 1,
            },
        );
    }
}

/// The board's blocks per day: a dated week's items reconciled with its Google Health workouts
/// as of today, or a template's items as planned. Commute rides only when `show_commutes`.
pub fn board_slots(app: &App) -> [Vec<Slot<'_>>; 7] {
    let plan = app.plan();
    let mut days = match app.board_monday() {
        Some(monday) => actuals::reconcile(
            &app.store.library,
            plan,
            monday,
            app.board_actuals().map_or(&[], |a| a.workouts.as_slice()),
            app.store.profile.weight,
            Local::now().date_naive(),
        ),
        None => std::array::from_fn(|d| {
            plan.sorted_day(d)
                .into_iter()
                .map(|item| Slot::Planned { item })
                .collect()
        }),
    };
    if !app.show_commutes {
        for day in &mut days {
            day.retain(|slot| !matches!(slot, Slot::Commute { .. }));
        }
    }
    days
}

/// The visible blocks of `day`, indexed by `App::card`.
pub fn day_slots(app: &App, day: usize) -> Vec<Slot<'_>> {
    board_slots(app).into_iter().nth(day).unwrap_or_default()
}

fn selected_slot(app: &App) -> Option<Slot<'_>> {
    day_slots(app, app.day).into_iter().nth(app.card)
}

fn slot_item<'a>(slot: &Slot<'a>) -> Option<&'a Item> {
    match slot {
        Slot::Done { item, .. } | Slot::Missed { item } | Slot::Planned { item } => Some(*item),
        Slot::Extra { .. } | Slot::Commute { .. } => None,
    }
}

pub fn selected_id(app: &App) -> Option<String> {
    selected_slot(app)
        .as_ref()
        .and_then(slot_item)
        .map(|item| item.id.clone())
}

/// The selected block's item id; on a bare workout, a hint that it has no session to edit.
fn selected_item_id(app: &mut App) -> Option<String> {
    let id = selected_id(app);
    if id.is_none() && selected_slot(app).is_some() {
        app.info("A Google Health workout, not a planned session: Enter shows its metrics");
    }
    id
}

/// Select item `id` wherever it is now.
fn select(app: &mut App, id: &str) {
    if let Some((d, _)) = app.plan().find(id) {
        let card = day_slots(app, d)
            .iter()
            .position(|slot| slot_item(slot).is_some_and(|item| item.id == id));
        app.day = d;
        app.card = card.unwrap_or(0);
    }
}

/// Enter: every metric of a done block or a workout, else the session editor.
fn open_selected(app: &mut App) {
    let opened = selected_slot(app).map(|slot| match slot {
        Slot::Done { item, workouts } => Ok(Modal::WorkoutDetails {
            title: app.store.library.title(item),
            workouts: workouts_metrics(app, &workouts),
        }),
        Slot::Extra { workout } | Slot::Commute { workout } => Ok(Modal::WorkoutDetails {
            title: actuals::workout_name(workout),
            workouts: workouts_metrics(app, &[workout]),
        }),
        Slot::Missed { item } | Slot::Planned { item } => Err(item.id.clone()),
    });
    match opened {
        Some(Ok(details)) => app.modal = details,
        Some(Err(id)) => editor::open(app, &id),
        None => {}
    }
}

fn workouts_metrics(app: &App, workouts: &[&Workout]) -> Vec<Vec<String>> {
    workouts
        .iter()
        .map(|w| actuals::metrics_lines(w, &effort(app, w)))
        .collect()
}

/// How hard `w` was, from the board week's heart-rate samples and the profile's zones.
fn effort(app: &App, w: &Workout) -> Effort {
    let samples = app
        .board_actuals()
        .map_or(&[][..], |a| a.heart_rate.as_slice());
    let profile = &app.store.profile;
    zones::workout_effort(w, samples, &profile.hr, profile.weight)
}

/// `m`: pick the workout the selected session was done as, or mark it missed on purpose.
fn open_link_picker(app: &mut App) {
    let Some(monday) = app.board_monday() else {
        app.error("Templates have no Google Health data; t goes to this week");
        return;
    };
    let Some(id) = selected_item_id(app) else {
        return;
    };
    let Some((d, i)) = app.plan().find(&id) else {
        return;
    };
    let current = app.plan().days[d][i].workout.clone();
    let date = monday + Days::new(d as u64);
    let mut workouts: Vec<&Workout> = app
        .board_actuals()
        .map(|a| a.workouts.iter().filter(|w| w.day == date).collect())
        .unwrap_or_default();
    workouts.sort_by_key(|w| w.start);
    let links: Vec<Option<String>> = workouts
        .iter()
        .map(|w| Some(w.id.clone()))
        .chain([Some(MISSED_ON_PURPOSE.to_owned()), None])
        .collect();
    let selected = links
        .iter()
        .position(|link| *link == current)
        .unwrap_or(links.len() - 1);
    app.modal = Modal::Picker(
        Picker::Workout { item: id, links },
        ListState::default().with_selected(Some(selected)),
    );
}

/// Set how item `id` was done: a workout id, [`MISSED_ON_PURPOSE`], or `None` to match itself.
pub fn link_workout(app: &mut App, id: &str, link: Option<String>) {
    let Some((d, i)) = app.plan().find(id) else {
        return;
    };
    app.plan_mut().days[d][i].workout = link;
    app.commit();
    select(app, id);
}

pub fn on_key(app: &mut App, k: KeyEvent) {
    match k.code {
        KeyCode::Char('h') | KeyCode::Left => {
            app.day = app.day.saturating_sub(1);
            app.clamp();
        }
        KeyCode::Char('l') | KeyCode::Right => {
            app.day = (app.day + 1).min(6);
            app.clamp();
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.card += 1;
            app.clamp();
        }
        KeyCode::Char('k') | KeyCode::Up => app.card = app.card.saturating_sub(1),
        KeyCode::Char('[') => step_week(app, false),
        KeyCode::Char(']') => step_week(app, true),
        KeyCode::Char('t') => {
            app.set_board(Board::Week(this_monday()));
            app.day = today_index();
            app.clamp();
        }
        KeyCode::Char('g') => open_go_to(app),
        KeyCode::Char('p') => open_template_picker(app),
        KeyCode::Char('r') => refetch_actuals(app),
        KeyCode::Char('a') => {
            if app.store.library.types.is_empty() {
                app.error("The library has no session types");
            } else {
                app.modal =
                    Modal::Picker(Picker::AddType, ListState::default().with_selected(Some(0)));
            }
        }
        KeyCode::Enter => open_selected(app),
        KeyCode::Char('e') => {
            if let Some(id) = selected_item_id(app) {
                editor::open(app, &id);
            }
        }
        KeyCode::Char('m') => open_link_picker(app),
        KeyCode::Char('y') => duplicate(app),
        KeyCode::Char('c') => {
            app.show_commutes = !app.show_commutes;
            app.info(if app.show_commutes {
                "Commute rides shown (not counted as training)"
            } else {
                "Commute rides hidden"
            });
        }
        KeyCode::Char('d') => {
            if let Some(id) = selected_item_id(app) {
                let label = app
                    .plan()
                    .find(&id)
                    .map(|(d, i)| {
                        let it = &app.plan().days[d][i];
                        format!("{} on {}", app.store.library.title(it), DAYS[d])
                    })
                    .unwrap_or_default();
                app.confirm(format!("Delete {label}?"), Action::DeleteItem(id));
            }
        }
        KeyCode::Char('<') => shift(app, -1),
        KeyCode::Char('>') => shift(app, 1),
        KeyCode::Char('S') => {
            if app.plan().is_empty() {
                load_starter(app);
            } else {
                app.confirm(
                    "Replace this week with the starter week?",
                    Action::LoadStarter,
                );
            }
        }
        KeyCode::Char('X') if !app.plan().is_empty() => {
            app.confirm("Clear every session from this week?", Action::ClearWeek);
        }
        _ => {}
    }
}

/// Add a new item of `type_key` to the selected day and open it in the editor.
pub fn add(app: &mut App, type_key: &str) {
    let Some(item) = library::new_item(&app.store.library, type_key) else {
        app.error("That type has no efforts");
        return;
    };
    let id = item.id.clone();
    let day = app.day;
    app.plan_mut().days[day].push(item);
    app.commit();
    select(app, &id);
    editor::open(app, &id);
}

fn duplicate(app: &mut App) {
    let Some((d, i)) = selected_item_id(app).and_then(|id| app.plan().find(&id)) else {
        return;
    };
    let copy = Item {
        id: Uuid::new_v4().to_string(),
        ..app.plan().days[d][i].clone()
    };
    let id = copy.id.clone();
    app.plan_mut().days[d].push(copy);
    app.commit();
    select(app, &id);
}

pub fn delete(app: &mut App, id: &str) {
    if let Some((d, i)) = app.plan().find(id) {
        app.plan_mut().days[d].remove(i);
        app.commit();
    }
}

fn shift(app: &mut App, by: isize) {
    let Some(id) = selected_item_id(app) else {
        return;
    };
    let Some((d, i)) = app.plan().find(&id) else {
        return;
    };
    let Some(to) = d.checked_add_signed(by).filter(|&t| t < 7) else {
        return;
    };
    let plan = app.plan_mut();
    let it = plan.days[d].remove(i);
    plan.days[to].push(it);
    app.commit();
    select(app, &id);
}

pub fn load_starter(app: &mut App) {
    app.plan_mut().days = library::starter_days();
    app.card = 0;
    app.commit();
}

pub fn clear(app: &mut App) {
    app.plan_mut().days = Default::default();
    app.card = 0;
    app.commit();
}

/// Filled effort pips, ceil(rpe / 2) of 5.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=5"
)]
fn pips(rpe: f64) -> String {
    let n = (rpe / 2.0).ceil().clamp(0.0, 5.0) as usize;
    format!("{}{}", "▮".repeat(n), "▯".repeat(5 - n))
}

fn level_style(l: Level) -> Style {
    match l {
        Level::Rest => Style::new().fg(DIM).bg(SURFACE),
        Level::Easy => Style::new().fg(Color::Black).bg(OK),
        Level::Moderate => Style::new()
            .fg(Color::Black)
            .bg(Color::Rgb(0xE0, 0xA1, 0x45)),
        Level::Hard | Level::Big => Style::new().fg(Color::Black).bg(Color::White).bold(),
    }
}

fn type_color(lib: &Library, it: &Item) -> Color {
    lib.get(&it.type_key).map_or(Color::Gray, |t| hex(&t.color))
}

/// The coloured bar down a card's left edge.
fn left_bar(color: Color) -> Block<'static> {
    Block::new()
        .borders(Borders::LEFT)
        .border_set(border::Set {
            vertical_left: "▌",
            ..border::PLAIN
        })
        .border_style(Style::new().fg(color))
}

fn span_text(start: NaiveTime, end: NaiveTime) -> String {
    format!("{}–{}", start.format(TIME_FMT), end.format(TIME_FMT))
}

/// One planned card: time and effort pips, "Type: Effort", meta line, notes.
fn planned_card(app: &App, it: &Item, clash: bool) -> Paragraph<'static> {
    let lib = &app.store.library;
    let resolved = lib.resolve(it);
    let color = type_color(lib, it);
    let train = calc::is_train(lib, it);
    let time = if it.dur > 0 {
        span_text(it.start, it.end())
    } else {
        "All day".into()
    };
    let mut top = vec![Span::styled(time, Style::new().fg(DIM))];
    if train && let Some((_, e)) = resolved {
        top.push(Span::raw(" "));
        top.push(Span::styled(pips(e.rpe), Style::new().fg(color)));
    }
    if clash {
        top.push(Span::styled(" clash", Style::new().fg(WARN).bold()));
    }
    let kc = calc::kcal(lib, app.store.profile.weight, it).round();
    let meta = [
        (it.dur > 0).then(|| hm(it.dur.into())),
        (kc > 20.0).then(|| format!("{kc} kcal")),
        calc::fuel_on_the_go(lib, it).then(|| "fuel on the go".to_string()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(", ");
    let life = resolved.is_some_and(|(t, _)| t.category == crate::model::Category::Life);
    let title_style = if life {
        Style::new().fg(color)
    } else {
        Style::new().fg(color).bold()
    };
    let mut lines = vec![Line::from(top), Line::styled(lib.title(it), title_style)];
    if !meta.is_empty() {
        lines.push(Line::styled(meta, Style::new().fg(DIM)));
    }
    if !it.notes.is_empty() {
        lines.push(Line::styled(it.notes.clone(), Style::new().italic()));
    }
    Paragraph::new(lines)
        .block(left_bar(if clash { WARN } else { color }))
        .wrap(Wrap { trim: true })
}

/// A done card: the actual time in the type's colour, metrics, and the plan it was done for.
fn done_card(app: &App, it: &Item, workouts: &[&Workout]) -> Paragraph<'static> {
    let lib = &app.store.library;
    let color = type_color(lib, it);
    let start = workouts.first().map_or(it.start, |w| w.start.time());
    let end = workouts.last().map_or_else(|| it.end(), |w| w.end.time());
    let minutes: u32 = workouts.iter().map(|w| w.minutes).sum();
    let efforts: Vec<Effort> = workouts.iter().map(|w| effort(app, w)).collect();
    let effort_label = lib
        .resolve(it)
        .map_or(it.effort.as_str(), |(_, e)| e.label.as_str());
    let plan_note = [
        Some(format!("plan {effort_label} {}", hm(it.dur.into()))),
        efforts
            .first()
            .and_then(|e| actuals::effort_comparison(lib, it, e)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{} {}", span_text(start, end), hm(minutes.into())),
                Style::new().fg(color),
            ),
            Span::styled(" ✓ done", Style::new().fg(OK).bold()),
        ]),
        Line::styled(lib.title(it), Style::new().fg(color).bold()),
    ];
    lines.extend(
        workouts
            .iter()
            .zip(&efforts)
            .map(|(w, e)| actuals::metrics_text(w, e))
            .filter(|text| !text.is_empty())
            .map(|text| Line::styled(text, Style::new().fg(DIM))),
    );
    lines.push(Line::styled(plan_note, Style::new().fg(DIM)));
    if !it.notes.is_empty() {
        lines.push(Line::styled(it.notes.clone(), Style::new().italic()));
    }
    Paragraph::new(lines)
        .block(left_bar(color))
        .wrap(Wrap { trim: true })
}

fn missed_card(app: &App, it: &Item) -> Paragraph<'static> {
    let crossed = Style::new().fg(DIM).add_modifier(Modifier::CROSSED_OUT);
    Paragraph::new(vec![
        Line::styled(span_text(it.start, it.end()), crossed),
        Line::styled(app.store.library.title(it), crossed),
    ])
    .block(left_bar(LINE))
    .wrap(Wrap { trim: true })
}

/// A workout no session was planned for.
fn extra_card(app: &App, w: &Workout) -> Paragraph<'static> {
    let metrics = actuals::metrics_text(w, &effort(app, w));
    let mut lines = vec![
        Line::from(vec![
            Span::raw(actuals::workout_name(w)).bold(),
            Span::styled(" unplanned", Style::new().fg(DIM)),
        ]),
        Line::styled(
            format!(
                "{} {}",
                span_text(w.start.time(), w.end.time()),
                hm(w.minutes.into())
            ),
            Style::new().fg(DIM),
        ),
    ];
    if !metrics.is_empty() {
        lines.push(Line::styled(metrics, Style::new().fg(DIM)));
    }
    Paragraph::new(lines).wrap(Wrap { trim: true })
}

fn commute_line(w: &Workout) -> Paragraph<'static> {
    Paragraph::new(format!(
        "{} commute {} {}",
        w.start.format(TIME_FMT),
        actuals::workout_name(w),
        hm(w.minutes.into())
    ))
    .style(Style::new().fg(DIM).italic())
    .wrap(Wrap { trim: true })
}

/// A block's text and, for clashing sessions and unplanned workouts, its outline colour.
fn slot_card(
    app: &App,
    slot: &Slot,
    clashes: &HashSet<String>,
) -> (Paragraph<'static>, Option<Color>) {
    match slot {
        Slot::Planned { item } => {
            let clash = clashes.contains(&item.id);
            (planned_card(app, item, clash), clash.then_some(WARN))
        }
        Slot::Done { item, workouts } => (done_card(app, item, workouts), None),
        Slot::Missed { item } => (missed_card(app, item), None),
        Slot::Extra { workout } => (extra_card(app, workout), Some(Color::Gray)),
        Slot::Commute { workout } => (commute_line(workout), None),
    }
}

/// Where and how one day column is drawn.
struct DayView<'a> {
    d: usize,
    slots: &'a [Slot<'a>],
    clashes: &'a HashSet<String>,
    wide: bool,
}

fn draw_day(f: &mut Frame, app: &App, hits: &mut Vec<Hit>, area: Rect, day: &DayView) {
    let d = day.d;
    let lib = &app.store.library;
    let c = calc::day_calc(lib, &app.store.profile, &app.plan().days[d]);
    let focused = d == app.day;
    let border_style = if focused {
        Style::new().fg(Color::White)
    } else {
        Style::new().fg(LINE)
    };
    let date = app
        .board_monday()
        .and_then(|m| m.checked_add_days(Days::new(d as u64)));
    let label = date.map_or_else(|| DAYS[d].to_owned(), |x| x.format("%a %-d").to_string());
    let label = if date == Some(Local::now().date_naive()) {
        Span::styled(
            format!(" {label} "),
            Style::new().fg(Color::Black).bg(Color::White).bold(),
        )
    } else {
        Span::raw(label).bold()
    };
    let mut head = vec![Span::raw(" "), label, Span::raw(" ")];
    head.push(Span::styled(
        format!(" {} ", c.lvl.name()),
        level_style(c.lvl),
    ));
    head.push(Span::raw(" "));
    let mut block = Block::bordered()
        .title(Line::from(head))
        .border_style(border_style);
    if !day.wide {
        block = block.title_bottom(Line::from(" h/l ◀ day ▶ ").fg(DIM).right_aligned());
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    hits.push(Hit {
        area,
        day: d,
        card: None,
    });

    let fuel_rows = if day.wide { 2 } else { 1 };
    let [gauge, fuel, _, cards] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(fuel_rows),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    f.render_widget(
        LineGauge::default()
            .ratio((c.ld / 15.0).clamp(0.0, 1.0))
            // `+ 0.0` turns the -0.0 of an empty sum into 0.0.
            .label(format!("load {:.1}", c.ld + 0.0))
            .filled_style(Style::new().fg(Color::White))
            .unfilled_style(Style::new().fg(LINE)),
        gauge,
    );
    f.render_widget(Paragraph::new(fuel_lines(&c, day.wide)), fuel);
    draw_cards(f, app, hits, cards, day);
}

/// Energy and macros, split over two lines in the narrow wide-board columns.
fn fuel_lines(c: &calc::DayCalc, wide: bool) -> Vec<Line<'static>> {
    let num = |n: i64| Span::raw(n.to_string()).bold();
    let dim = |s: &'static str| Span::styled(s, Style::new().fg(DIM));
    if wide {
        vec![
            Line::from(vec![num(c.kc), dim(" kcal")]),
            Line::from(vec![
                num(c.p),
                dim("P "),
                num(c.c),
                dim("C "),
                num(c.f),
                dim("F"),
            ]),
        ]
    } else {
        vec![Line::from(vec![
            num(c.kc),
            dim(" kcal   "),
            num(c.p),
            dim(" protein g   "),
            num(c.c),
            dim(" carbs g   "),
            num(c.f),
            dim(" fat g"),
        ])]
    }
}

/// The day's blocks, scrolled so the selected one is visible.
fn draw_cards(f: &mut Frame, app: &App, hits: &mut Vec<Hit>, cards: Rect, day: &DayView) {
    if day.slots.is_empty() {
        let msg = if is_unstored_week(app) {
            "empty · p to apply a template"
        } else {
            "Nothing planned. Press a to add."
        };
        f.render_widget(Paragraph::new(msg).fg(DIM).wrap(Wrap { trim: true }), cards);
        return;
    }
    let sel = (day.d == app.day).then_some(app.card);
    let paras: Vec<(Paragraph, u16, Option<Color>)> = day
        .slots
        .iter()
        .enumerate()
        .map(|(i, slot)| {
            let (mut para, outline) = slot_card(app, slot, day.clashes);
            if sel == Some(i) {
                para = para.style(Style::new().bg(SURFACE));
            }
            let border = if outline.is_some() { 2 } else { 0 };
            let width = cards.width.saturating_sub(border);
            let height = cells(para.line_count(width)) + border;
            (para, height, outline)
        })
        .collect();
    // Scroll so the selected card is visible, one row of spacing between cards.
    let mut first = 0;
    if let Some(selected) = sel {
        while first < selected
            && paras[first..=selected].iter().map(|x| x.1 + 1).sum::<u16>() > cards.height
        {
            first += 1;
        }
    }
    let mut y = cards.y;
    for (i, (para, h, outline)) in paras.into_iter().enumerate().skip(first) {
        if y >= cards.bottom() {
            break;
        }
        let h = h.min(cards.bottom() - y);
        let mut rect = Rect::new(cards.x, y, cards.width, h);
        hits.push(Hit {
            area: rect,
            day: day.d,
            card: Some(i),
        });
        if let Some(color) = outline {
            let outer = Block::bordered().border_style(Style::new().fg(color));
            let inner = outer.inner(rect);
            f.render_widget(outer, rect);
            rect = inner;
        }
        f.render_widget(para, rect);
        y += h + 1;
    }
    if first > 0 {
        f.render_widget(
            Paragraph::new(format!("↑ {first} more"))
                .fg(DIM)
                .right_aligned(),
            Rect::new(cards.x, cards.y.saturating_sub(1), cards.width, 1),
        );
    }
}

/// A dated week's planned against done time per discipline and its training load.
struct WeekProgress {
    /// Disciplines with time planned or done.
    totals: Vec<TypeTotals>,
    trimp: f64,
    /// Some load comes from Google's heart-rate bands, for want of samples.
    google_bands: bool,
}

fn week_progress(app: &App, slots: &[Vec<Slot>; 7]) -> WeekProgress {
    let efforts: Vec<Effort> = slots
        .iter()
        .flatten()
        .flat_map(|slot| match slot {
            Slot::Done { workouts, .. } => workouts.clone(),
            Slot::Extra { workout } => vec![*workout],
            Slot::Missed { .. } | Slot::Planned { .. } | Slot::Commute { .. } => vec![],
        })
        .map(|w| effort(app, w))
        .collect();
    WeekProgress {
        totals: actuals::totals(&app.store.library, app.plan(), slots)
            .into_iter()
            .filter(|t| t.planned_min > 0 || t.done_min > 0)
            .collect(),
        // `+ 0.0` turns the -0.0 of an empty sum into 0.0.
        trimp: zones::week_trimp(&efforts) + 0.0,
        google_bands: efforts
            .iter()
            .any(|e| e.zones.is_some_and(|z| z.is_google())),
    }
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let mut hits = Vec::new();
    draw_board(f, app, &mut hits, area);
    app.hits = hits;
}

fn draw_board(f: &mut Frame, app: &App, hits: &mut Vec<Hit>, area: Rect) {
    let plan = app.plan();
    let lib = &app.store.library;
    let slots = board_slots(app);
    let checks = calc::checks(lib, plan);
    let progress = app.board_monday().map(|_| week_progress(app, &slots));
    let summary = calc::summary(lib, plan);
    let bar_rows = progress
        .as_ref()
        .map_or(summary.disciplines.len(), |p| 2 * p.totals.len() + 1);
    let lower_h = (cells(checks.len().max(bar_rows).max(1)) + 2).clamp(4, 12);
    let [title, board, lower] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(lower_h),
    ])
    .areas(area);
    draw_title(f, app, title);

    let clashes = calc::clashes(plan);
    let wide = area.width >= WIDE;
    let view = |d: usize| DayView {
        d,
        slots: &slots[d],
        clashes: &clashes,
        wide,
    };
    if wide {
        let cols = Layout::horizontal([Constraint::Ratio(1, 7); 7]).split(board);
        for (d, col) in cols.iter().enumerate() {
            draw_day(f, app, hits, *col, &view(d));
        }
    } else {
        draw_day(f, app, hits, board, &view(app.day));
    }

    let [checks_area, disc_area] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(lower);
    let week_note = app.board_actuals().map_or_else(
        || " Checks ".into(),
        |a| {
            let at = a.fetched_at.with_timezone(&Local).format("%a %-d %b %H:%M");
            format!(" Checks · Done from Google Health, fetched {at} · r refetch ")
        },
    );
    let lines: Vec<Line> = if checks.is_empty() {
        vec![Line::styled(
            calc::no_checks_message(plan),
            Style::new().fg(OK),
        )]
    } else {
        checks
            .iter()
            .map(|c| {
                Line::from(vec![
                    Span::styled("! ", Style::new().fg(WARN)),
                    Span::raw(c),
                ])
            })
            .collect()
    };
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            Block::bordered()
                .title(week_note)
                .border_style(Style::new().fg(LINE)),
        ),
        checks_area,
    );
    match &progress {
        Some(progress) => draw_progress(f, lib, progress, disc_area),
        None => draw_disciplines(f, &summary, disc_area),
    }
}

/// The board's title and the keys that move it.
fn draw_title(f: &mut Frame, app: &App, area: Rect) {
    let keys = if app.board_monday().is_some() {
        "[ ] week · t this week · g go to · p template · m link · Enter details · c commutes "
    } else {
        "t this week · [ ] g dated weeks "
    };
    let title = format!(" {}", board_title(app));
    let [left, right] = Layout::horizontal([
        Constraint::Length(cells(title.chars().count() + 1)),
        Constraint::Fill(1),
    ])
    .areas(area);
    f.render_widget(Paragraph::new(title.bold()), left);
    f.render_widget(Paragraph::new(keys).fg(DIM).right_aligned(), right);
}

/// A bar of `minutes` against the longest bar, `max`.
fn minutes_bar(minutes: u32, max: u32, filled: Style) -> LineGauge<'static> {
    LineGauge::default()
        .ratio(f64::from(minutes) / f64::from(max.max(minutes).max(1)))
        .label("")
        .filled_symbol("█")
        .unfilled_symbol("░")
        .filled_style(filled)
        .unfilled_style(Style::new().fg(LINE))
}

/// Bars of planned time per discipline.
fn draw_disciplines(f: &mut Frame, summary: &calc::Summary, area: Rect) {
    let block = Block::bordered()
        .title(" Time by discipline ")
        .border_style(Style::new().fg(LINE));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let max = summary
        .disciplines
        .iter()
        .map(|x| x.minutes)
        .max()
        .unwrap_or(0);
    let rows =
        Layout::vertical(summary.disciplines.iter().map(|_| Constraint::Length(1))).split(inner);
    for (x, row) in summary.disciplines.iter().zip(rows.iter()) {
        let [bar, label] =
            Layout::horizontal([Constraint::Percentage(35), Constraint::Fill(1)]).areas(*row);
        f.render_widget(
            minutes_bar(x.minutes, max, Style::new().fg(hex(&x.color))),
            bar,
        );
        let amount = if x.minutes > 0 {
            hm(x.minutes.into())
        } else {
            "none".into()
        };
        let note = if x.counted {
            ""
        } else {
            " (not in weekly hours)"
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(format!(" {} {amount}", x.label)),
                Span::styled(note, Style::new().fg(DIM)),
            ])),
            label,
        );
    }
}

/// Planned (dim) over done bars per discipline, `Run 3h / 2h40`, then the week's load.
fn draw_progress(f: &mut Frame, lib: &Library, progress: &WeekProgress, area: Rect) {
    let block = Block::bordered()
        .title(" Planned / done by discipline ")
        .border_style(Style::new().fg(LINE));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let max = progress
        .totals
        .iter()
        .flat_map(|t| [t.planned_min, t.done_min])
        .max()
        .unwrap_or(0);
    let rows = Layout::vertical((0..=2 * progress.totals.len()).map(|_| Constraint::Length(1)))
        .split(inner);
    let (pairs, _) = rows.as_chunks::<2>();
    for (t, [planned_row, done_row]) in progress.totals.iter().zip(pairs) {
        let (label, color) = lib
            .get(&t.type_key)
            .map_or((t.type_key.as_str(), Color::Gray), |ty| {
                (ty.label.as_str(), hex(&ty.color))
            });
        let [planned_bar, text] =
            Layout::horizontal([Constraint::Percentage(35), Constraint::Fill(1)])
                .areas(*planned_row);
        let [done_bar, _] =
            Layout::horizontal([Constraint::Percentage(35), Constraint::Fill(1)]).areas(*done_row);
        f.render_widget(
            minutes_bar(
                t.planned_min,
                max,
                Style::new().fg(color).add_modifier(Modifier::DIM),
            ),
            planned_bar,
        );
        f.render_widget(
            minutes_bar(t.done_min, max, Style::new().fg(color)),
            done_bar,
        );
        f.render_widget(
            Paragraph::new(format!(
                " {label} {} / {}",
                hm(t.planned_min.into()),
                hm(t.done_min.into())
            )),
            text,
        );
    }
    if let Some(&row) = rows.last() {
        let mut load = vec![Span::raw(format!("load {:.0} (TRIMP)", progress.trimp))];
        if progress.google_bands {
            load.push(Span::styled(" (Google bands)", Style::new().fg(DIM)));
        }
        f.render_widget(Paragraph::new(Line::from(load)), row);
    }
}

fn link_label(app: &App, link: Option<&str>) -> Line<'static> {
    match link {
        None => Line::raw("auto: clear the link"),
        Some(MISSED_ON_PURPOSE) => Line::raw("none: missed on purpose"),
        Some(id) => {
            let workout = app
                .board_actuals()
                .and_then(|a| a.workouts.iter().find(|w| w.id == id));
            match workout {
                Some(w) => Line::from(vec![
                    Span::raw(actuals::workout_name(w)),
                    Span::styled(
                        format!("  {} · {}", w.start.format(TIME_FMT), hm(w.minutes.into())),
                        Style::new().fg(DIM),
                    ),
                ]),
                None => Line::raw(id.to_owned()),
            }
        }
    }
}

/// The "add a session" type list, the templates to apply to the week, or the workouts to link.
pub fn draw_picker(f: &mut Frame, app: &App, picker: &Picker, state: &mut ListState) {
    let (title, items, confirm): (String, Vec<ListItem>, &str) = match picker {
        Picker::AddType => (
            format!("Add to {}", DAYS[app.day]),
            app.store
                .library
                .types
                .iter()
                .map(|t| {
                    ListItem::new(Line::from(vec![
                        Span::styled("● ", Style::new().fg(hex(&t.color))),
                        Span::raw(t.label.clone()),
                        Span::styled(format!("  {}", t.category.name()), Style::new().fg(DIM)),
                    ]))
                })
                .collect(),
            "add",
        ),
        Picker::Template => (
            app.board_monday().map_or_else(String::new, |m| {
                format!("Apply to the week of {}", m.format(DAY_FMT))
            }),
            app.store
                .plans
                .iter()
                .map(|p| {
                    let hours = calc::summary(&app.store.library, p).hours;
                    ListItem::new(Line::from(vec![
                        Span::raw(p.name.clone()),
                        Span::styled(format!("  {hours} h"), Style::new().fg(DIM)),
                    ]))
                })
                .collect(),
            "apply",
        ),
        Picker::Workout { item, links } => (
            app.plan().find(item).map_or_else(String::new, |(d, i)| {
                format!(
                    "{} was done as",
                    app.store.library.title(&app.plan().days[d][i])
                )
            }),
            links
                .iter()
                .map(|link| ListItem::new(link_label(app, link.as_deref())))
                .collect(),
            "link",
        ),
    };
    let area = popup_area(f.area(), 44, cells(items.len()) + 4);
    let inner = frame_popup(f, area, &title, Color::White);
    let [list, hint] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
    f.render_stateful_widget(
        List::new(items)
            .highlight_style(Style::new().bg(SURFACE).bold())
            .highlight_symbol("› "),
        list,
        state,
    );
    f.render_widget(
        Paragraph::new(format!("j/k move · Enter {confirm} · Esc cancel")).fg(DIM),
        hint,
    );
}

/// Today's index in the week, for the initial selection.
pub fn today_index() -> usize {
    Local::now().weekday().num_days_from_monday() as usize
}
