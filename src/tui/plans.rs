//! Screen 2: named plans.

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
    app::{Action, App, FormKind},
    widgets::{DIM, Field, Form, LINE, SURFACE},
};
use crate::{calc, model::Plan};

pub fn on_key(app: &mut App, k: KeyEvent) {
    let n = app.store.plans.len();
    match k.code {
        KeyCode::Char('j') | KeyCode::Down => app.plan_sel = (app.plan_sel + 1).min(n - 1),
        KeyCode::Char('k') | KeyCode::Up => app.plan_sel = app.plan_sel.saturating_sub(1),
        KeyCode::Enter => {
            app.store.active = app.plan_sel;
            app.card = 0;
            app.commit();
            app.info(format!("Active plan: {}", app.store.plan().name));
        }
        KeyCode::Char('n') => {
            let name = format!("Week {}", n + 1);
            let form = Form::new("New plan", Color::White, vec![Field::text("Name", name)]);
            app.open_form(form, FormKind::NewPlan);
        }
        KeyCode::Char('c') => copy(app),
        KeyCode::Char('r') => {
            let name = app.store.plans[app.plan_sel].name.clone();
            let form = Form::new("Rename plan", Color::White, vec![Field::text("Name", name)]);
            app.open_form(form, FormKind::RenamePlan(app.plan_sel));
        }
        KeyCode::Char('d') => {
            if n <= 1 {
                app.error("Keep at least one plan");
            } else {
                let name = &app.store.plans[app.plan_sel].name;
                app.confirm(
                    format!("Delete plan {name:?}?"),
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

/// A new empty plan, made active.
pub fn apply_new(app: &mut App, form: &Form) -> Result<(), String> {
    app.store.plans.push(Plan::new(name(form)?));
    app.store.active = app.store.plans.len() - 1;
    app.plan_sel = app.store.active;
    app.commit();
    Ok(())
}

pub fn apply_rename(app: &mut App, form: &Form, i: usize) -> Result<(), String> {
    let n = name(form)?;
    let p = app.store.plans.get_mut(i).ok_or("That plan is gone")?;
    p.name = n;
    app.commit();
    Ok(())
}

/// Deep copy of the selected plan with fresh plan and item ids, made active.
fn copy(app: &mut App) {
    let mut p = app.store.plans[app.plan_sel].clone();
    p.id = Uuid::new_v4().to_string();
    p.name = format!("{} copy", p.name);
    for it in p.days.iter_mut().flatten() {
        it.id = Uuid::new_v4().to_string();
    }
    app.store.plans.push(p);
    app.store.active = app.store.plans.len() - 1;
    app.plan_sel = app.store.active;
    app.commit();
}

pub fn delete(app: &mut App, i: usize) {
    if app.store.plans.len() <= 1 || i >= app.store.plans.len() {
        return;
    }
    app.store.plans.remove(i);
    if app.store.active > i {
        app.store.active -= 1;
    }
    app.commit();
}

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .store
        .plans
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let s = calc::summary(&app.store.library, p);
            let active = i == app.store.active;
            let mark = if active { "● " } else { "  " };
            let n: usize = p.days.iter().map(Vec::len).sum();
            ListItem::new(Line::from(vec![
                Span::raw(mark),
                Span::styled(
                    format!("{:<32}", p.name),
                    if active {
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
                    .title(" Plans (● active) ")
                    .title_bottom(
                        Line::from(" Enter activate · n new · c copy · r rename · d delete ")
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
