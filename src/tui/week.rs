//! Screen 1: the week board, checks and discipline bars.

use std::collections::HashSet;

use chrono::Local;
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
    app::{Action, App, Hit, Modal},
    editor,
    widgets::{DIM, LINE, OK, SURFACE, WARN, frame_popup, hex, popup_area},
};
use crate::{
    calc::{self, Level},
    library,
    model::{DAYS, Item, TIME_FMT, hm},
    sync::{self, DayDone},
};

/// Below this width the board shows one day at a time.
pub const WIDE: u16 = 140;

/// Items of the selected day in board order.
fn sorted_ids(app: &App, day: usize) -> Vec<String> {
    app.store
        .plan()
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
    if let Some((d, _)) = app.store.plan().find(id) {
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
        KeyCode::Char('a') => {
            if app.store.library.types.is_empty() {
                app.error("The library has no session types");
            } else {
                app.modal = Modal::AddPicker(ListState::default().with_selected(Some(0)));
            }
        }
        KeyCode::Enter | KeyCode::Char('e') => {
            if let Some(id) = selected_id(app) {
                editor::open(app, &id);
            }
        }
        KeyCode::Char('y') => duplicate(app),
        KeyCode::Char('d') => {
            if let Some(id) = selected_id(app) {
                let label = app
                    .store
                    .plan()
                    .find(&id)
                    .map(|(d, i)| {
                        let it = &app.store.plan().days[d][i];
                        format!("{} on {}", title(app, it), DAYS[d])
                    })
                    .unwrap_or_default();
                app.confirm(format!("Delete {label}?"), Action::DeleteItem(id));
            }
        }
        KeyCode::Char('<') => shift(app, -1),
        KeyCode::Char('>') => shift(app, 1),
        KeyCode::Char('S') => {
            if app.store.plan().is_empty() {
                load_starter(app);
            } else {
                app.confirm(
                    "Replace this week with the starter week?",
                    Action::LoadStarter,
                );
            }
        }
        KeyCode::Char('X') if !app.store.plan().is_empty() => {
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
    app.store.plan_mut().days[app.day].push(item);
    app.commit();
    select(app, &id);
    editor::open(app, &id);
}

fn duplicate(app: &mut App) {
    let Some((d, i)) = selected_id(app).and_then(|id| app.store.plan().find(&id)) else {
        return;
    };
    let copy = Item {
        id: Uuid::new_v4().to_string(),
        ..app.store.plan().days[d][i].clone()
    };
    let id = copy.id.clone();
    app.store.plan_mut().days[d].push(copy);
    app.commit();
    select(app, &id);
}

pub fn delete(app: &mut App, id: &str) {
    if let Some((d, i)) = app.store.plan().find(id) {
        app.store.plan_mut().days[d].remove(i);
        app.commit();
    }
}

fn shift(app: &mut App, by: isize) {
    let Some(id) = selected_id(app) else { return };
    let Some((d, i)) = app.store.plan().find(&id) else {
        return;
    };
    let Some(to) = d.checked_add_signed(by).filter(|&t| t < 7) else {
        return;
    };
    let plan = app.store.plan_mut();
    let it = plan.days[d].remove(i);
    plan.days[to].push(it);
    app.commit();
    select(app, &id);
}

pub fn load_starter(app: &mut App) {
    app.store.plan_mut().days = library::starter_days();
    app.card = 0;
    app.commit();
}

pub fn clear(app: &mut App) {
    app.store.plan_mut().days = Default::default();
    app.card = 0;
    app.commit();
}

/// `Swim: Technique`
fn title(app: &App, it: &Item) -> String {
    match app.store.library.resolve(it) {
        Some((t, e)) => format!("{}: {}", t.label, e.label),
        None => format!("{}: {}", it.type_key, it.effort),
    }
}

/// Filled effort pips, ceil(rpe / 2) of 5.
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
    let mut lines = vec![Line::from(top), Line::styled(title(app, it), title_style)];
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

/// `Done 1h05 (2) of 1h30 (2)`, plus a short list of what was done.
fn done_lines(done: &DayDone) -> Vec<Line<'static>> {
    let ok = done.done_min() >= done.planned_min && !done.done.is_empty();
    let style = if done.done.is_empty() && done.planned > 0 {
        Style::new().fg(WARN)
    } else if ok {
        Style::new().fg(OK)
    } else {
        Style::new()
    };
    let mut out = vec![Line::styled(
        format!(
            "Done {} ({}) of {} ({})",
            hm(done.done_min().into()),
            done.done.len(),
            hm(done.planned_min.into()),
            done.planned
        ),
        style,
    )];
    if !done.done.is_empty() {
        let what = done
            .done
            .iter()
            .map(|w| format!("{} {}", sync::workout_label(w), hm(w.minutes.into())))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(Line::styled(what, Style::new().fg(DIM)));
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
    let c = calc::day_calc(lib, &app.store.profile, &app.store.plan().days[d]);
    let focused = d == app.day;
    let border_style = if focused {
        Style::new().fg(Color::White)
    } else {
        Style::new().fg(LINE)
    };
    let mut head = vec![Span::raw(" "), Span::raw(DAYS[d]).bold(), Span::raw(" ")];
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

    let done_lines = done.map(done_lines).unwrap_or_default();
    let fuel_rows = if wide { 2 } else { 1 };
    let [gauge, fuel, done_area, _, cards] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(fuel_rows),
        Constraint::Length(done_lines.len() as u16),
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
    let b = |n: i64| Span::raw(n.to_string()).bold();
    let d_ = |s: &'static str| Span::styled(s, Style::new().fg(DIM));
    let fuel_text = if wide {
        vec![
            Line::from(vec![b(c.kc), d_(" kcal")]),
            Line::from(vec![b(c.p), d_("P "), b(c.c), d_("C "), b(c.f), d_("F")]),
        ]
    } else {
        vec![Line::from(vec![
            b(c.kc),
            d_(" kcal   "),
            b(c.p),
            d_(" protein g   "),
            b(c.c),
            d_(" carbs g   "),
            b(c.f),
            d_(" fat g"),
        ])]
    };
    f.render_widget(Paragraph::new(fuel_text), fuel);
    f.render_widget(
        Paragraph::new(done_lines).wrap(Wrap { trim: true }),
        done_area,
    );

    let plan = app.store.plan();
    let items: Vec<Item> = plan.sorted_day(d).into_iter().cloned().collect();
    if items.is_empty() {
        f.render_widget(
            Paragraph::new("Nothing planned. Press a to add.")
                .fg(DIM)
                .wrap(Wrap { trim: true }),
            cards,
        );
        return;
    }
    let sel = if focused { Some(app.card) } else { None };
    let paras: Vec<(Paragraph, u16, bool)> = items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let clash = clashes.contains(&it.id);
            let p = card(app, it, clash, sel == Some(i));
            let extra = if clash { 2 } else { 0 };
            let w = cards.width.saturating_sub(extra);
            let h = p.line_count(w) as u16 + extra;
            (p, h, clash)
        })
        .collect();
    // Scroll so the selected card is visible, one row of spacing between cards.
    let mut first = 0;
    if let Some(s) = sel {
        while first < s && paras[first..=s].iter().map(|x| x.1 + 1).sum::<u16>() > cards.height {
            first += 1;
        }
    }
    let mut y = cards.y;
    for (i, (p, h, clash)) in paras.into_iter().enumerate().skip(first) {
        if y >= cards.bottom() {
            break;
        }
        let h = h.min(cards.bottom() - y);
        let mut r = Rect::new(cards.x, y, cards.width, h);
        app.hits.push(Hit {
            area: r,
            day: d,
            card: Some(i),
        });
        if clash {
            let outer = Block::bordered().border_style(Style::new().fg(WARN));
            let inner = outer.inner(r);
            f.render_widget(outer, r);
            r = inner;
        }
        f.render_widget(p, r);
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
    let plan = app.store.plan().clone();
    let lib = &app.store.library;
    let checks = calc::checks(lib, &plan);
    let summary = calc::summary(lib, &plan);
    let lower_h = (checks.len().max(summary.disciplines.len()).max(1) as u16 + 2).clamp(4, 10);
    let [board, lower] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(lower_h)]).areas(area);

    let clashes = calc::clashes(&plan);
    let done = app
        .done
        .as_ref()
        .map(|(monday, w)| (*monday, sync::planned_vs_done(lib, &plan, *monday, w)));
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
    let week_note = done
        .map(|(m, _)| format!(" Checks · Done from Google Health, week of {m} "))
        .unwrap_or_else(|| " Checks ".into());
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

    let block = Block::bordered()
        .title(" Time by discipline ")
        .border_style(Style::new().fg(LINE));
    let inner = block.inner(disc_area);
    f.render_widget(block, disc_area);
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

/// The "add a session" type list.
pub fn draw_picker(f: &mut Frame, app: &App, state: &mut ListState) {
    let types = &app.store.library.types;
    let area = popup_area(f.area(), 40, types.len() as u16 + 4);
    let inner = frame_popup(f, area, &format!("Add to {}", DAYS[app.day]), Color::White);
    let items: Vec<ListItem> = types
        .iter()
        .map(|t| {
            ListItem::new(Line::from(vec![
                Span::styled("● ", Style::new().fg(hex(&t.color))),
                Span::raw(t.label.clone()),
                Span::styled(format!("  {}", t.category.name()), Style::new().fg(DIM)),
            ]))
        })
        .collect();
    let [list, hint] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
    f.render_stateful_widget(
        List::new(items)
            .highlight_style(Style::new().bg(SURFACE).bold())
            .highlight_symbol("› "),
        list,
        state,
    );
    f.render_widget(
        Paragraph::new("j/k move · Enter add · Esc cancel").fg(DIM),
        hint,
    );
}

/// Today's index in the week, for the initial selection.
pub fn today_index() -> usize {
    use chrono::Datelike;
    Local::now().weekday().num_days_from_monday() as usize
}
