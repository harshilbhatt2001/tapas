//! Screen 2: plan templates, opened on the board or applied to dated weeks.

use chrono::{Local, NaiveDate};
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent},
    layout::Rect,
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState},
};
use uuid::Uuid;

use super::{
    app::{Action, App, Board, FormKind, Screen},
    week,
    widgets::{DIM, Field, Form, LINE, SURFACE},
};
use crate::{
    calc, export,
    model::{DAY_FMT, Plan, week_monday},
};

pub fn on_key(app: &mut App, k: KeyEvent) {
    let n = app.store.plans.len();
    match k.code {
        KeyCode::Char('j') | KeyCode::Down => app.plan_sel = (app.plan_sel + 1).min(n - 1),
        KeyCode::Char('k') | KeyCode::Up => app.plan_sel = app.plan_sel.saturating_sub(1),
        KeyCode::Enter => {
            let p = &app.store.plans[app.plan_sel];
            let msg = format!("Editing template {}; t goes back to this week", p.name);
            app.set_board(Board::Template(p.id.clone()));
            app.card = 0;
            app.screen = Screen::Week;
            app.info(msg);
        }
        KeyCode::Char('A') => {
            let p = &app.store.plans[app.plan_sel];
            let from = export::next_monday(Local::now().date_naive());
            let form = Form::new(
                format!("Apply {}", p.name),
                Color::White,
                vec![
                    Field::text("From Monday", from.to_string()),
                    Field::text("Weeks 1-52", "1"),
                ],
            );
            app.open_form(form, FormKind::ApplyTemplate(p.id.clone()));
        }
        KeyCode::Char('n') => {
            let name = format!("Week {}", n + 1);
            let form = Form::new(
                "New template",
                Color::White,
                vec![Field::text("Name", name)],
            );
            app.open_form(form, FormKind::NewPlan);
        }
        KeyCode::Char('c') => copy(app),
        KeyCode::Char('r') => {
            let name = app.store.plans[app.plan_sel].name.clone();
            let form = Form::new(
                "Rename template",
                Color::White,
                vec![Field::text("Name", name)],
            );
            app.open_form(form, FormKind::RenamePlan(app.plan_sel));
        }
        KeyCode::Char('d') => {
            if n <= 1 {
                app.error("Keep at least one template");
            } else {
                let name = &app.store.plans[app.plan_sel].name;
                app.confirm(
                    format!("Delete template {name:?}? Dated weeks keep their copies."),
                    Action::DeletePlan(app.plan_sel),
                );
            }
        }
        _ => {}
    }
}

fn name(form: &Form) -> Result<String, String> {
    let n = form.text(0);
    if n.is_empty() {
        Err("Name: can't be empty".into())
    } else {
        Ok(n.to_owned())
    }
}

/// Select the last template and put it on the board.
fn show_last(app: &mut App) {
    app.plan_sel = app.store.plans.len() - 1;
    let id = app.store.plans[app.plan_sel].id.clone();
    app.set_board(Board::Template(id));
}

/// A new empty template, put on the board.
pub fn apply_new(app: &mut App, form: &Form) -> Result<(), String> {
    app.store.plans.push(Plan::new(name(form)?));
    show_last(app);
    app.commit();
    Ok(())
}

pub fn apply_rename(app: &mut App, form: &Form, i: usize) -> Result<(), String> {
    let n = name(form)?;
    let p = app.store.plans.get_mut(i).ok_or("That template is gone")?;
    p.name = n;
    app.commit();
    Ok(())
}

/// Deep copy of the selected template with fresh plan and item ids, put on the board.
fn copy(app: &mut App) {
    let mut p = app.store.plans[app.plan_sel].clone();
    p.id = Uuid::new_v4().to_string();
    p.name = format!("{} copy", p.name);
    for it in p.days.iter_mut().flatten() {
        it.id = Uuid::new_v4().to_string();
    }
    app.store.plans.push(p);
    show_last(app);
    app.commit();
}

/// Deleting the template on the board sends the board back to this week.
pub fn delete(app: &mut App, i: usize) {
    if app.store.plans.len() <= 1 || i >= app.store.plans.len() {
        return;
    }
    if app.board_template() == Some(i) {
        app.set_board(Board::Week(week::this_monday()));
    }
    app.store.plans.remove(i);
    app.commit();
}

/// `Mon 21 Sep, Mon 28 Sep`
fn dates(mondays: &[NaiveDate]) -> String {
    mondays
        .iter()
        .map(|m| m.format(DAY_FMT).to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The "Apply" form: asks first when any target week already has sessions.
pub fn submit_apply(app: &mut App, form: &Form, template: &str) -> Result<(), String> {
    let from: NaiveDate = form
        .text(0)
        .parse()
        .map_err(|_| "From Monday: use YYYY-MM-DD".to_string())?;
    let weeks: u32 = form.parse(1)?;
    if !(1..=52).contains(&weeks) {
        return Err("Weeks 1-52: between 1 and 52".into());
    }
    let filled = app.store.filled_weeks(from, weeks);
    if filled.is_empty() {
        apply_template(app, template, from, weeks);
    } else {
        app.confirm(
            format!("Replace the sessions of {}?", dates(&filled)),
            Action::ApplyTemplate {
                template: template.to_owned(),
                from,
                weeks,
            },
        );
    }
    Ok(())
}

/// Replace the items of `weeks` weeks from the week of `from` with the template's.
pub fn apply_template(app: &mut App, template: &str, from: NaiveDate, weeks: u32) {
    if !app.store.apply_template(template, from, weeks) {
        app.error("That template is gone");
        return;
    }
    let name = app
        .store
        .plans
        .iter()
        .find(|p| p.id == template)
        .map(|p| p.name.clone())
        .unwrap_or_default();
    app.card = 0;
    app.commit();
    let count = if weeks == 1 {
        "1 week".to_owned()
    } else {
        format!("{weeks} weeks")
    };
    app.info(format!(
        "Applied {name} to {count} from {}",
        week_monday(from).format(DAY_FMT)
    ));
}

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let on_board = app.board_template();
    let items: Vec<ListItem> = app
        .store
        .plans
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let s = calc::summary(&app.store.library, p);
            let shown = on_board == Some(i);
            let mark = if shown { "● " } else { "  " };
            let n: usize = p.days.iter().map(Vec::len).sum();
            ListItem::new(Line::from(vec![
                Span::raw(mark),
                Span::styled(
                    format!("{:<32}", p.name),
                    if shown {
                        Style::new().bold()
                    } else {
                        Style::new()
                    },
                ),
                Span::raw(format!("{:>6} h", s.hours)),
                Span::styled(
                    format!(
                        "   {} sessions, {} hard, {} long, {n} items",
                        s.sessions, s.hard, s.long
                    ),
                    Style::new().fg(DIM),
                ),
            ]))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(app.plan_sel));
    f.render_stateful_widget(
        List::new(items)
            .block(
                Block::bordered()
                    .title(" Templates (● on the board) ")
                    .title_bottom(
                        Line::from(
                            " Enter edit · A apply to weeks · n new · c copy · r rename · d delete ",
                        )
                        .fg(DIM),
                    )
                    .border_style(Style::new().fg(LINE)),
            )
            .highlight_style(Style::new().bg(SURFACE))
            .highlight_symbol("› "),
        area,
        &mut state,
    );
}
