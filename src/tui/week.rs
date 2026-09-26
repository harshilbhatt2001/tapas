//! Screen 1: the week board, checks and discipline bars.

use std::collections::HashSet;

use chrono::{Datelike, Days, Local, NaiveDate};
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent},
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
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
    calc::{self, Level},
    google::auth::Api,
    health_cache::{self, WeekActuals},
    library,
    model::{DAY_FMT, DAYS, Item, TIME_FMT, hm, week_monday, week_title},
    services::{self, DayDone, Prompt},
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

/// Items of the selected day in board order.
fn sorted_ids(app: &App, day: usize) -> Vec<String> {
    app.plan()
        .sorted_day(day)
        .into_iter()
        .map(|x| x.id.clone())
        .collect()
}

pub fn selected_id(app: &App) -> Option<String> {
    sorted_ids(app, app.day).get(app.card).cloned()
}

/// Select item `id` wherever it is now.
fn select(app: &mut App, id: &str) {
    if let Some((d, _)) = app.plan().find(id) {
        app.day = d;
        app.card = sorted_ids(app, d).iter().position(|x| x == id).unwrap_or(0);
    }
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
        KeyCode::Enter | KeyCode::Char('e') => {
            if let Some(id) = selected_id(app) {
                editor::open(app, &id);
            }
        }
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
            if let Some(id) = selected_id(app) {
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
    let Some((d, i)) = selected_id(app).and_then(|id| app.plan().find(&id)) else {
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
    let Some(id) = selected_id(app) else { return };
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

/// One card: time and effort pips, "Type: Effort", meta line, notes.
fn card<'a>(app: &App, it: &Item, clash: bool, selected: bool) -> Paragraph<'a> {
    let lib = &app.store.library;
    let resolved = lib.resolve(it);
    let color = resolved.map_or(Color::Gray, |(t, _)| hex(&t.color));
    let train = calc::is_train(lib, it);
    let time = if it.dur > 0 {
        format!(
            "{}–{}",
            it.start.format(TIME_FMT),
            it.end().format(TIME_FMT)
        )
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
    let mut lines = vec![
        Line::from(top),
        Line::styled(app.store.library.title(it), title_style),
    ];
    if !meta.is_empty() {
        lines.push(Line::styled(meta, Style::new().fg(DIM)));
    }
    if !it.notes.is_empty() {
        lines.push(Line::styled(it.notes.clone(), Style::new().italic()));
    }
    let bar = Block::new()
        .borders(Borders::LEFT)
        .border_set(border::Set {
            vertical_left: "▌",
            ..border::PLAIN
        })
        .border_style(Style::new().fg(if clash { WARN } else { color }));
    let bg = if selected {
        Style::new().bg(SURFACE)
    } else {
        Style::new()
    };
    Paragraph::new(lines)
        .block(bar)
        .style(bg)
        .wrap(Wrap { trim: true })
}

/// `Done 1h05 (2) of 1h30 (2)`, plus a short list of what was done. Commute rides never count;
/// they are listed when `show_commutes`, otherwise only hinted at.
fn done_lines(done: &DayDone, show_commutes: bool) -> Vec<Line<'static>> {
    let ok = done.done_min() >= done.planned_min && !done.done.is_empty();
    let style = if done.done.is_empty() && done.planned > 0 {
        Style::new().fg(WARN)
    } else if ok {
        Style::new().fg(OK)
    } else {
        Style::new()
    };
    let mut head = vec![Span::styled(
        format!(
            "Done {} ({}) of {} ({})",
            hm(done.done_min().into()),
            done.done.len(),
            hm(done.planned_min.into()),
            done.planned
        ),
        style,
    )];
    if !show_commutes && !done.commutes.is_empty() {
        head.push(Span::styled(
            format!(" +{} commute", done.commutes.len()),
            Style::new().fg(DIM),
        ));
    }
    let mut out = vec![Line::from(head)];
    if !done.done.is_empty() {
        out.push(Line::styled(
            services::workouts_text(&done.done),
            Style::new().fg(DIM),
        ));
    }
    if show_commutes && !done.commutes.is_empty() {
        out.push(Line::styled(
            format!("commute: {}", services::workouts_text(&done.commutes)),
            Style::new().fg(DIM).italic(),
        ));
    }
    out
}

fn draw_day(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    d: usize,
    clashes: &HashSet<String>,
    done: Option<&DayDone>,
    wide: bool,
) {
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
    if !wide {
        block = block.title_bottom(Line::from(" h/l ◀ day ▶ ").fg(DIM).right_aligned());
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.hits.push(Hit {
        area,
        day: d,
        card: None,
    });

    let show = app.show_commutes;
    let done_par = Paragraph::new(done.map(|d| done_lines(d, show)).unwrap_or_default())
        .wrap(Wrap { trim: true });
    // Wrapped rows, not lines: narrow columns wrap the Done and commute lists.
    let done_rows = if done.is_some() {
        cells(done_par.line_count(inner.width))
    } else {
        0
    };
    let fuel_rows = if wide { 2 } else { 1 };
    let [gauge, fuel, done_area, _, cards] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(fuel_rows),
        Constraint::Length(done_rows),
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
    f.render_widget(Paragraph::new(fuel_lines(&c, wide)), fuel);
    f.render_widget(done_par, done_area);
    draw_cards(f, app, cards, d, clashes, focused);
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

/// The day's session cards, scrolled so the selected one is visible.
fn draw_cards(
    f: &mut Frame,
    app: &mut App,
    cards: Rect,
    d: usize,
    clashes: &HashSet<String>,
    focused: bool,
) {
    let plan = app.plan();
    let items: Vec<Item> = plan.sorted_day(d).into_iter().cloned().collect();
    if items.is_empty() {
        let msg = if is_unstored_week(app) {
            "empty · p to apply a template"
        } else {
            "Nothing planned. Press a to add."
        };
        f.render_widget(Paragraph::new(msg).fg(DIM).wrap(Wrap { trim: true }), cards);
        return;
    }
    let sel = if focused { Some(app.card) } else { None };
    let paras: Vec<(Paragraph, u16, bool)> = items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let clash = clashes.contains(&it.id);
            let para = card(app, it, clash, sel == Some(i));
            let extra = if clash { 2 } else { 0 };
            let width = cards.width.saturating_sub(extra);
            let height = cells(para.line_count(width)) + extra;
            (para, height, clash)
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
    for (i, (para, h, clash)) in paras.into_iter().enumerate().skip(first) {
        if y >= cards.bottom() {
            break;
        }
        let h = h.min(cards.bottom() - y);
        let mut rect = Rect::new(cards.x, y, cards.width, h);
        app.hits.push(Hit {
            area: rect,
            day: d,
            card: Some(i),
        });
        if clash {
            let outer = Block::bordered().border_style(Style::new().fg(WARN));
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

pub fn draw(f: &mut Frame, app: &mut App, area: Rect) {
    let plan = app.plan().clone();
    let lib = &app.store.library;
    let checks = calc::checks(lib, &plan);
    let summary = calc::summary(lib, &plan);
    let lower_h = (cells(checks.len().max(summary.disciplines.len()).max(1)) + 2).clamp(4, 10);
    let [title, board, lower] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(lower_h),
    ])
    .areas(area);
    draw_title(f, app, title);

    let clashes = calc::clashes(&plan);
    let done = app
        .board_monday()
        .zip(app.board_actuals())
        .map(|(monday, a)| {
            let weight = app.store.profile.weight;
            (
                a.fetched_at,
                services::planned_vs_done(lib, &plan, monday, &a.workouts, weight),
            )
        });
    let done_day = |d: usize| done.as_ref().map(|(_, days)| days[d].clone());
    app.hits.clear();
    let wide = area.width >= WIDE;
    if wide {
        let cols = Layout::horizontal([Constraint::Ratio(1, 7); 7]).split(board);
        for d in 0..7 {
            draw_day(f, app, cols[d], d, &clashes, done_day(d).as_ref(), true);
        }
    } else {
        let d = app.day;
        draw_day(f, app, board, d, &clashes, done_day(d).as_ref(), false);
    }

    let [checks_area, disc_area] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(lower);
    let week_note = done.map_or_else(
        || " Checks ".into(),
        |(fetched_at, _)| {
            let at = fetched_at.with_timezone(&Local).format("%a %-d %b %H:%M");
            format!(" Checks · Done from Google Health, fetched {at} · r refetch ")
        },
    );
    let lines: Vec<Line> = if checks.is_empty() {
        vec![Line::styled(
            calc::no_checks_message(&plan),
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
    draw_disciplines(f, &summary, disc_area);
}

/// The board's title and the keys that move it.
fn draw_title(f: &mut Frame, app: &App, area: Rect) {
    let keys = if app.board_monday().is_some() {
        "[ ] week · t this week · g go to · p apply template "
    } else {
        "t this week · [ ] g dated weeks "
    };
    let [left, right] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(52)]).areas(area);
    f.render_widget(
        Paragraph::new(format!(" {}", board_title(app)).bold()),
        left,
    );
    f.render_widget(Paragraph::new(keys).fg(DIM).right_aligned(), right);
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
        .unwrap_or(0)
        .max(1);
    let rows =
        Layout::vertical(summary.disciplines.iter().map(|_| Constraint::Length(1))).split(inner);
    for (x, row) in summary.disciplines.iter().zip(rows.iter()) {
        let [bar, label] =
            Layout::horizontal([Constraint::Percentage(35), Constraint::Fill(1)]).areas(*row);
        let color = hex(&x.color);
        f.render_widget(
            LineGauge::default()
                .ratio(f64::from(x.minutes) / f64::from(max))
                .label("")
                .filled_symbol("█")
                .unfilled_symbol("░")
                .filled_style(Style::new().fg(color))
                .unfilled_style(Style::new().fg(LINE)),
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

/// The "add a session" type list, or the templates to apply to the week.
pub fn draw_picker(f: &mut Frame, app: &App, picker: Picker, state: &mut ListState) {
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
