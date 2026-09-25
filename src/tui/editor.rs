//! Item editor popup with a live fuelling guide.

use chrono::NaiveTime;
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use super::{
    app::{App, FormKind},
    widgets::{DIM, Field, Form, frame_popup, hex, popup_area},
};
use crate::{
    calc,
    model::{DAYS, Item, TIME_FMT},
};

const EFFORT: usize = 0;
const DAY: usize = 1;
const START: usize = 2;
const DUR: usize = 3;
const NOTES: usize = 4;

/// Longest session the editor accepts, as in the artifact.
const MAX_DUR: u32 = 720;

pub fn open(app: &mut App, id: &str) {
    let Some((d, i)) = app.store.plan().find(id) else {
        return;
    };
    let it = &app.store.plan().days[d][i];
    let Some(t) = app.store.library.get(&it.type_key) else {
        app.error(format!(
            "Type {:?} is no longer in the library",
            it.type_key
        ));
        return;
    };
    let effort = t
        .efforts
        .iter()
        .position(|e| e.key == it.effort)
        .unwrap_or(0);
    let form = Form::new(
        t.label.clone(),
        hex(&t.color),
        vec![
            Field::choice(
                "Effort",
                t.efforts.iter().map(|e| e.label.clone()).collect(),
                effort,
            ),
            Field::choice("Day", DAYS.map(String::from).to_vec(), d),
            Field::text("Start", it.start.format(TIME_FMT).to_string()),
            Field::text("Duration min", it.dur.to_string()),
            Field::text("Notes", it.notes.clone()),
        ],
    );
    app.open_form(form, FormKind::Item(id.to_owned()));
}

/// Changing the effort resets the duration to the effort's default, like the artifact.
pub fn on_change(app: &App, form: &mut Form, id: &str, field: usize) {
    if field != EFFORT {
        return;
    }
    let Some(it) = item(app, id) else { return };
    if let Some(e) = app
        .store
        .library
        .get(&it.type_key)
        .and_then(|t| t.efforts.get(form.choice(EFFORT)))
    {
        form.set_text(DUR, e.dur.to_string());
    }
}

fn item<'a>(app: &'a App, id: &str) -> Option<&'a Item> {
    let (d, i) = app.store.plan().find(id)?;
    Some(&app.store.plan().days[d][i])
}

/// The item as the form currently describes it, and the target day.
fn edited(app: &App, form: &Form, id: &str) -> Result<(Item, usize), String> {
    let it = item(app, id).ok_or("The session no longer exists")?;
    let t = app
        .store
        .library
        .get(&it.type_key)
        .ok_or("The session's type is no longer in the library")?;
    let effort = t
        .efforts
        .get(form.choice(EFFORT))
        .map_or_else(|| it.effort.clone(), |e| e.key.clone());
    let start = NaiveTime::parse_from_str(form.text(START), TIME_FMT)
        .map_err(|_| "Start: use HH:MM, e.g. 07:30".to_string())?;
    let dur: u32 = form
        .text(DUR)
        .parse()
        .map_err(|_| "Duration min: whole minutes, 0 for all day".to_string())?;
    if dur > MAX_DUR {
        return Err(format!("Duration min: at most {MAX_DUR}"));
    }
    let new = Item {
        effort,
        start,
        dur,
        notes: form.text(NOTES).to_owned(),
        ..it.clone()
    };
    Ok((new, form.choice(DAY)))
}

pub fn apply(app: &mut App, form: &Form, id: &str) -> Result<(), String> {
    let (new, to) = edited(app, form, id)?;
    let (d, i) = app
        .store
        .plan()
        .find(id)
        .ok_or("The session no longer exists")?;
    let plan = app.store.plan_mut();
    if d == to {
        plan.days[d][i] = new;
    } else {
        plan.days[d].remove(i);
        plan.days[to].push(new);
    }
    app.commit();
    app.day = to;
    app.card = app
        .store
        .plan()
        .sorted_day(to)
        .iter()
        .position(|x| x.id == id)
        .unwrap_or(0);
    Ok(())
}

/// Form on top, guide below; the guide follows the effort and duration being edited.
pub fn draw(f: &mut Frame, app: &App, form: &Form, id: &str) {
    let lib = &app.store.library;
    // Preview with whatever parses so far.
    let preview = edited(app, form, id).map(|x| x.0).ok().or_else(|| {
        let it = item(app, id)?.clone();
        let effort = lib
            .get(&it.type_key)
            .and_then(|t| t.efforts.get(form.choice(EFFORT)))
            .map_or_else(|| it.effort.clone(), |e| e.key.clone());
        Some(Item {
            effort,
            dur: form.text(DUR).parse().unwrap_or(it.dur),
            ..it
        })
    });
    let mut guide = Vec::new();
    if let Some(it) = &preview {
        let g = calc::guide(lib, app.store.profile.weight, it);
        let row = |k: &'static str, v: String| {
            Line::from(vec![
                Span::styled(format!("{k:<8}"), Style::new().fg(form.accent).bold()),
                Span::raw(v),
            ])
        };
        for (k, v) in [
            ("Before", g.before),
            ("During", g.during),
            ("After", g.after),
            ("Note", g.note),
        ] {
            if let Some(v) = v {
                guide.push(row(k, v));
            }
        }
        if calc::is_train(lib, it) {
            let kc = calc::kcal(lib, app.store.profile.weight, it).round();
            guide.push(row(
                "Energy",
                format!("About {kc} kcal on top of your day."),
            ));
        }
    }
    if guide.is_empty() {
        guide.push(Line::styled(
            "No fuelling notes for this one.",
            Style::new().fg(DIM),
        ));
    }
    let guide = Paragraph::new(guide).wrap(Wrap { trim: true });
    let width = 76.min(f.area().width);
    let guide_h = guide.line_count(width.saturating_sub(2)) as u16;
    let area = popup_area(f.area(), width, form.height() + guide_h + 4);
    let inner = frame_popup(f, area, &form.title, form.accent);
    let [fields, _, head, body] = Layout::vertical([
        Constraint::Length(form.height()),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(inner);
    form.render_fields(f, fields);
    f.render_widget(Paragraph::new("Guide").fg(DIM).bold(), head);
    f.render_widget(guide, body);
}
