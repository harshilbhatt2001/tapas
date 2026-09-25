//! Screen 4: weight and base intake, with the weight pull from Google Health.

use anyhow::Result;
use chrono::{DateTime, Local, Utc};
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent},
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::Line,
    widgets::{Block, Paragraph, Row, Table, Wrap},
};

use super::{
    app::{Action, App, BgResult, FormKind, Task},
    export::google_state,
    widgets::{DIM, Field, Form, LINE},
};
use crate::{google::auth::Api, sync};

pub fn on_key(app: &mut App, k: KeyEvent) {
    match k.code {
        KeyCode::Enter | KeyCode::Char('e') => {
            let p = &app.store.profile;
            let form = Form::new(
                "Profile",
                Color::White,
                vec![
                    Field::text("Weight kg", p.weight.to_string()),
                    Field::text("Base kcal", p.base.kcal.to_string()),
                    Field::text("Base protein g", p.base.p.to_string()),
                    Field::text("Base carbs g", p.base.c.to_string()),
                    Field::text("Base fat g", p.base.f.to_string()),
                ],
            );
            app.open_form(form, FormKind::Profile);
        }
        KeyCode::Char('w') => {
            if let Err(e) = sync::require_login(&app.paths, Api::Health) {
                app.error(format!("{e:#}"));
                return;
            }
            let paths = app.paths.clone();
            app.spawn(Task::Weight, async move {
                BgResult::Weight(sync::latest_weight(&paths).await)
            });
        }
        _ => {}
    }
}

pub fn apply(app: &mut App, form: &Form) -> Result<(), String> {
    let v: Vec<f64> = (0..5).map(|i| form.parse(i)).collect::<Result<_, _>>()?;
    if v.iter().any(|x| *x < 0.0) {
        return Err("Values can't be negative".into());
    }
    if v[0] <= 0.0 {
        return Err("Weight kg: must be above 0".into());
    }
    let p = &mut app.store.profile;
    p.weight = v[0];
    p.base.kcal = v[1];
    p.base.p = v[2];
    p.base.c = v[3];
    p.base.f = v[4];
    app.commit();
    Ok(())
}

pub fn on_weight(app: &mut App, res: Result<Option<(f64, DateTime<Utc>)>>) {
    match res {
        Err(e) => app.error(format!("Weight: {e:#}")),
        Ok(None) => app.info("No weight in Google Health for the last 90 days"),
        Ok(Some((kg, at))) => {
            let kg = (kg * 10.0).round() / 10.0;
            let at = at.with_timezone(&Local).format("%Y-%m-%d %H:%M");
            app.info(format!("Google Health: {kg} kg on {at}"));
            app.confirm(
                format!("Google Health has {kg} kg (measured {at}). Use it as your weight?"),
                Action::ApplyWeight(kg),
            );
        }
    }
}

pub fn apply_weight(app: &mut App, kg: f64) {
    app.store.profile.weight = kg;
    app.commit();
}

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let p = &app.store.profile;
    let [table, text] = Layout::vertical([Constraint::Length(7), Constraint::Fill(1)]).areas(area);
    let rows = [
        ("Weight", format!("{} kg", p.weight)),
        ("Base calories", format!("{} kcal", p.base.kcal)),
        ("Base protein", format!("{} g", p.base.p)),
        ("Base carbs", format!("{} g", p.base.c)),
        ("Base fat", format!("{} g", p.base.f)),
    ]
    .map(|(k, v)| Row::new(vec![k.to_owned(), v]));
    f.render_widget(
        Table::new(rows, [Constraint::Length(16), Constraint::Fill(1)]).block(
            Block::bordered()
                .title(" Profile ")
                .title_bottom(Line::from(" e edit · w weight from Google Health ").fg(DIM))
                .border_style(Style::new().fg(LINE)),
        ),
        table,
    );
    let body = vec![
        Line::raw(
            "Base is a rest day. Each session adds its estimated energy on top (from MET values and your weight), 85% as carbs and 15% as fat; protein stays at base. Watch your weekly average weight and adjust the base.",
        ),
        Line::raw(""),
        google_state(app),
    ];
    f.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: true })
            .block(Block::new().fg(DIM)),
        text.inner(ratatui::layout::Margin::new(1, 1)),
    );
}
