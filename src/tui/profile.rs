//! Screen 4: weight and base intake, with the weight pull from Google Health.

use std::ops::RangeInclusive;

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
use crate::{
    google::auth::Api,
    services::{self, Prompt},
    zones::{HrZones, default_upper},
};

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
                    Field::text("Max HR bpm", p.hr.max.to_string()),
                    Field::text("Resting HR bpm", p.hr.resting.to_string()),
                    Field::text(
                        "LTHR (optional)",
                        p.hr.lthr.map(|lthr| lthr.to_string()).unwrap_or_default(),
                    ),
                    Field::text("Z1 upper bpm", p.hr.upper[0].to_string()),
                    Field::text("Z2 upper bpm", p.hr.upper[1].to_string()),
                    Field::text("Z3 upper bpm", p.hr.upper[2].to_string()),
                    Field::text("Z4 upper bpm", p.hr.upper[3].to_string()),
                ],
            );
            app.open_form(form, FormKind::Profile);
        }
        KeyCode::Char('w') => {
            if let Err(e) = services::require_login(&app.paths, Api::Health) {
                app.error(format!("{e:#}"));
                return;
            }
            let paths = app.paths.clone();
            app.spawn(Task::Weight, async move {
                BgResult::Weight(services::latest_weight(&paths, Prompt::Never).await)
            });
        }
        _ => {}
    }
}

const MAX_HR: usize = 5;
const RESTING_HR: usize = 6;
const LTHR: usize = 7;
const FIRST_UPPER: usize = 8;
const SANE_BPM: RangeInclusive<u16> = 40..=230;

fn parse_bpm(form: &Form, i: usize) -> Result<Option<u16>, String> {
    if form.text(i).is_empty() {
        return Ok(None);
    }
    let bpm: u16 = form.parse(i)?;
    if !SANE_BPM.contains(&bpm) {
        return Err(format!(
            "{}: must be {}-{} bpm",
            form.fields[i].label,
            SANE_BPM.start(),
            SANE_BPM.end()
        ));
    }
    Ok(Some(bpm))
}

fn required_bpm(form: &Form, i: usize) -> Result<u16, String> {
    parse_bpm(form, i)?.ok_or_else(|| format!("{}: required", form.fields[i].label))
}

/// Zone bounds after editing: blank bounds take the new defaults, and bounds still at the
/// defaults of the old max HR and LTHR follow the new ones.
fn edited_upper(old: &HrZones, max: u16, lthr: Option<u16>, entered: [Option<u16>; 4]) -> [u16; 4] {
    let new_default = default_upper(max, lthr);
    let upper = std::array::from_fn(|i| entered[i].unwrap_or(new_default[i]));
    if upper == default_upper(old.max, old.lthr) {
        new_default
    } else {
        upper
    }
}

fn parse_hr(form: &Form, old: &HrZones) -> Result<HrZones, String> {
    let max = required_bpm(form, MAX_HR)?;
    let resting = required_bpm(form, RESTING_HR)?;
    if resting >= max {
        return Err("Resting HR: must be below max HR".into());
    }
    let lthr = parse_bpm(form, LTHR)?;
    let mut entered = [None; 4];
    for (i, bound) in entered.iter_mut().enumerate() {
        *bound = parse_bpm(form, FIRST_UPPER + i)?;
    }
    let upper = edited_upper(old, max, lthr, entered);
    if let Some(i) = upper.windows(2).position(|pair| pair[0] >= pair[1]) {
        return Err(format!(
            "Z{} upper: must be above Z{} upper ({} bpm)",
            i + 2,
            i + 1,
            upper[i]
        ));
    }
    Ok(HrZones {
        max,
        resting,
        lthr,
        upper,
    })
}

pub fn apply(app: &mut App, form: &Form) -> Result<(), String> {
    let v: Vec<f64> = (0..5).map(|i| form.parse(i)).collect::<Result<_, _>>()?;
    if v.iter().any(|x| *x < 0.0) {
        return Err("Values can't be negative".into());
    }
    if v[0] <= 0.0 {
        return Err("Weight kg: must be above 0".into());
    }
    let hr = parse_hr(form, &app.store.profile.hr)?;
    let p = &mut app.store.profile;
    p.hr = hr;
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

fn zones_text(hr: &HrZones) -> String {
    let [z1, z2, z3, z4] = hr.upper;
    let source = if hr.upper != default_upper(hr.max, hr.lthr) {
        "custom"
    } else if hr.lthr.is_some() {
        "from LTHR"
    } else {
        "from max HR"
    };
    format!("Z1 ≤{z1} · Z2 ≤{z2} · Z3 ≤{z3} · Z4 ≤{z4} · Z5 >{z4} bpm ({source})")
}

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let p = &app.store.profile;
    let [table, text] = Layout::vertical([Constraint::Length(11), Constraint::Fill(1)]).areas(area);
    let rows = [
        ("Weight", format!("{} kg", p.weight)),
        ("Base calories", format!("{} kcal", p.base.kcal)),
        ("Base protein", format!("{} g", p.base.p)),
        ("Base carbs", format!("{} g", p.base.c)),
        ("Base fat", format!("{} g", p.base.f)),
        ("Max HR", format!("{} bpm", p.hr.max)),
        ("Resting HR", format!("{} bpm", p.hr.resting)),
        (
            "LTHR",
            p.hr.lthr
                .map_or_else(|| "none".to_owned(), |lthr| format!("{lthr} bpm")),
        ),
        ("HR zones", zones_text(&p.hr)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bounds_follow_a_new_max_hr_but_typed_bounds_stay() {
        let old = HrZones::default();
        let unchanged = old.upper.map(Some);
        assert_eq!(
            edited_upper(&old, 200, None, unchanged),
            default_upper(200, None)
        );
        assert_eq!(
            edited_upper(&old, 185, Some(170), unchanged),
            default_upper(185, Some(170))
        );
        let typed = [Some(115), Some(130), Some(148), Some(167)];
        assert_eq!(edited_upper(&old, 200, None, typed), [115, 130, 148, 167]);
        let blank_z1 = [None, Some(130), Some(148), Some(167)];
        assert_eq!(
            edited_upper(&old, 200, None, blank_z1),
            [120, 130, 148, 167]
        );
    }
}
