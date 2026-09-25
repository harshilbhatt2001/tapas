//! Screen 3: the editable session library.

use chrono::NaiveTime;
use ratatui::{
    Frame,
    crossterm::event::{KeyCode, KeyEvent},
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState, Row, Table, TableState},
};

use super::{
    app::{Action, App, FormKind, LibFocus},
    widgets::{DIM, Field, Form, LINE, SURFACE, hex},
};
use crate::{
    library::default_library,
    model::{Category, Effort, Kind, SessionType, TIME_FMT},
};

pub fn on_key(app: &mut App, k: KeyEvent) {
    let types = app.store.library.types.len();
    let efforts = app
        .store
        .library
        .types
        .get(app.lib_type)
        .map_or(0, |t| t.efforts.len());
    match (k.code, app.lib_focus) {
        (KeyCode::Char('h') | KeyCode::Left, _) => app.lib_focus = LibFocus::Types,
        (KeyCode::Char('l') | KeyCode::Right, _) if types > 0 => app.lib_focus = LibFocus::Efforts,
        (KeyCode::Tab, f) => {
            app.lib_focus = match f {
                LibFocus::Types => LibFocus::Efforts,
                LibFocus::Efforts => LibFocus::Types,
            }
        }
        (KeyCode::Char('j') | KeyCode::Down, LibFocus::Types) => {
            app.lib_type = (app.lib_type + 1).min(types.saturating_sub(1));
            app.lib_effort = 0;
        }
        (KeyCode::Char('k') | KeyCode::Up, LibFocus::Types) => {
            app.lib_type = app.lib_type.saturating_sub(1);
            app.lib_effort = 0;
        }
        (KeyCode::Char('j') | KeyCode::Down, LibFocus::Efforts) => {
            app.lib_effort = (app.lib_effort + 1).min(efforts.saturating_sub(1));
        }
        (KeyCode::Char('k') | KeyCode::Up, LibFocus::Efforts) => {
            app.lib_effort = app.lib_effort.saturating_sub(1);
        }
        (KeyCode::Enter | KeyCode::Char('e'), LibFocus::Types) => {
            if let Some(t) = app.store.library.types.get(app.lib_type) {
                let form = type_form(t, "Edit type");
                app.open_form(form, FormKind::Type(Some(app.lib_type)));
            }
        }
        (KeyCode::Enter | KeyCode::Char('e'), LibFocus::Efforts) => {
            if let Some(e) = app
                .store
                .library
                .types
                .get(app.lib_type)
                .and_then(|t| t.efforts.get(app.lib_effort))
            {
                let form = effort_form(e, "Edit effort", Color::White);
                app.open_form(form, FormKind::Effort(app.lib_type, Some(app.lib_effort)));
            }
        }
        (KeyCode::Char('n'), LibFocus::Types) => {
            let form = type_form(&blank_type(), "New type");
            app.open_form(form, FormKind::Type(None));
        }
        (KeyCode::Char('n'), LibFocus::Efforts) => {
            if types > 0 {
                let form = effort_form(&blank_effort(), "New effort", Color::White);
                app.open_form(form, FormKind::Effort(app.lib_type, None));
            }
        }
        (KeyCode::Char('d'), LibFocus::Types) => {
            let Some(t) = app.store.library.types.get(app.lib_type) else {
                return;
            };
            let users: Vec<&str> = app
                .store
                .plans
                .iter()
                .filter(|p| p.uses_type(&t.key))
                .map(|p| p.name.as_str())
                .collect();
            if users.is_empty() {
                let msg = format!("Delete type {:?}?", t.label);
                app.confirm(msg, Action::DeleteType(app.lib_type));
            } else {
                let msg = format!("{} is used by: {}", t.label, users.join(", "));
                app.error(msg);
            }
        }
        (KeyCode::Char('d'), LibFocus::Efforts) => {
            let Some(t) = app.store.library.types.get(app.lib_type) else {
                return;
            };
            if t.efforts.len() <= 1 {
                app.error("A type needs at least one effort");
            } else if let Some(e) = t.efforts.get(app.lib_effort) {
                let msg = format!("Delete effort {:?} of {}?", e.label, t.label);
                app.confirm(msg, Action::DeleteEffort(app.lib_type, app.lib_effort));
            }
        }
        (KeyCode::Char('R'), _) => app.confirm(
            "Reset the library to the defaults? Custom types and efforts are lost.",
            Action::ResetLibrary,
        ),
        _ => {}
    }
}

fn blank_type() -> SessionType {
    SessionType {
        key: String::new(),
        label: "New type".into(),
        color: "#8E9AA6".into(),
        kind: Kind::Other,
        category: Category::Train,
        start: NaiveTime::from_hms_opt(7, 0, 0).expect("valid time"),
        counted: true,
        efforts: vec![blank_effort()],
    }
}

fn blank_effort() -> Effort {
    Effort {
        key: String::new(),
        label: "Easy".into(),
        rpe: 4.0,
        met: 6.0,
        dur: 45,
        legs: false,
    }
}

fn type_form(t: &SessionType, title: &str) -> Form {
    let kind = Kind::ALL.iter().position(|k| *k == t.kind).unwrap_or(0);
    let cat = Category::ALL
        .iter()
        .position(|c| *c == t.category)
        .unwrap_or(0);
    Form::new(
        title,
        hex(&t.color),
        vec![
            Field::text("Label", t.label.clone()),
            Field::text("Colour #RRGGBB", t.color.clone()),
            Field::choice(
                "Kind",
                Kind::ALL.map(|k| k.name().to_owned()).to_vec(),
                kind,
            ),
            Field::choice(
                "Category",
                Category::ALL.map(|c| c.name().to_owned()).to_vec(),
                cat,
            ),
            Field::text("Default start", t.start.format(TIME_FMT).to_string()),
            Field::toggle("Weekly hours", t.counted),
        ],
    )
}

fn effort_form(e: &Effort, title: &str, accent: Color) -> Form {
    Form::new(
        title,
        accent,
        vec![
            Field::text("Label", e.label.clone()),
            Field::text("RPE 0-10", e.rpe.to_string()),
            Field::text("MET", e.met.to_string()),
            Field::text("Default min", e.dur.to_string()),
            Field::toggle("Heavy legs", e.legs),
        ],
    )
}

/// Slug key from a label, unique among `taken`.
fn new_key<'a>(label: &str, taken: &(impl Iterator<Item = &'a str> + Clone)) -> String {
    let base = slug::slugify(label);
    let base = if base.is_empty() {
        "custom".into()
    } else {
        base
    };
    // Among count + 1 candidates at least one is free.
    (1..=taken.clone().count() + 1)
        .map(|n| {
            if n == 1 {
                base.clone()
            } else {
                format!("{base}{n}")
            }
        })
        .find(|k| !taken.clone().any(|t| t == k))
        .expect("a free candidate")
}

fn label(form: &Form) -> Result<String, String> {
    let l = form.text(0);
    if l.is_empty() {
        Err("Label: can't be empty".into())
    } else {
        Ok(l.to_owned())
    }
}

pub fn apply_type(app: &mut App, form: &Form, i: Option<usize>) -> Result<(), String> {
    let label = label(form)?;
    let color = form.text(1).to_uppercase();
    let valid = color.len() == 7
        && color.starts_with('#')
        && color[1..].chars().all(|c| c.is_ascii_hexdigit());
    if !valid {
        return Err("Colour: use #RRGGBB, e.g. #5B9BE0".into());
    }
    let start = NaiveTime::parse_from_str(form.text(4), TIME_FMT)
        .map_err(|_| "Default start: use HH:MM".to_string())?;
    let apply = |t: &mut SessionType| {
        t.label.clone_from(&label);
        t.color.clone_from(&color);
        t.kind = Kind::ALL[form.choice(2)];
        t.category = Category::ALL[form.choice(3)];
        t.start = start;
        t.counted = form.toggle(5);
    };
    let types = &mut app.store.library.types;
    if let Some(i) = i {
        apply(types.get_mut(i).ok_or("That type is gone")?);
    } else {
        let mut t = blank_type();
        t.key = new_key(&label, &types.iter().map(|t| t.key.as_str()));
        t.efforts[0].key = "easy".into();
        apply(&mut t);
        types.push(t);
        app.lib_type = types.len() - 1;
        app.lib_effort = 0;
    }
    app.commit();
    Ok(())
}

pub fn apply_effort(app: &mut App, form: &Form, t: usize, e: Option<usize>) -> Result<(), String> {
    let label = label(form)?;
    let rpe: f64 = form.parse(1)?;
    if !(0.0..=10.0).contains(&rpe) {
        return Err("RPE 0-10: between 0 and 10".into());
    }
    let met: f64 = form.parse(2)?;
    if met < 0.0 {
        return Err("MET: can't be negative".into());
    }
    let dur: u32 = form.parse(3)?;
    let apply = |x: &mut Effort| {
        x.label.clone_from(&label);
        x.rpe = rpe;
        x.met = met;
        x.dur = dur;
        x.legs = form.toggle(4);
    };
    let ty = app
        .store
        .library
        .types
        .get_mut(t)
        .ok_or("That type is gone")?;
    if let Some(e) = e {
        apply(ty.efforts.get_mut(e).ok_or("That effort is gone")?);
    } else {
        let mut x = blank_effort();
        x.key = new_key(&label, &ty.efforts.iter().map(|e| e.key.as_str()));
        apply(&mut x);
        ty.efforts.push(x);
        app.lib_effort = ty.efforts.len() - 1;
    }
    app.commit();
    Ok(())
}

pub fn delete_type(app: &mut App, i: usize) {
    let lib = &mut app.store.library;
    let Some(t) = lib.types.get(i) else { return };
    if app.store.plans.iter().any(|p| p.uses_type(&t.key)) {
        return;
    }
    lib.types.remove(i);
    app.commit();
}

pub fn delete_effort(app: &mut App, t: usize, e: usize) {
    let Some(ty) = app.store.library.types.get_mut(t) else {
        return;
    };
    if ty.efforts.len() > 1 && e < ty.efforts.len() {
        ty.efforts.remove(e);
        app.commit();
    }
}

pub fn reset(app: &mut App) {
    app.store.library = default_library();
    app.lib_type = 0;
    app.lib_effort = 0;
    app.commit();
}

pub fn draw(f: &mut Frame, app: &App, area: Rect) {
    let [left, right] =
        Layout::horizontal([Constraint::Length(54), Constraint::Fill(1)]).areas(area);
    let focus_style = |on: bool| Style::new().fg(if on { Color::White } else { LINE });
    let types = &app.store.library.types;
    let items: Vec<ListItem> = types
        .iter()
        .map(|t| {
            let counted = if t.counted { "" } else { " ·not counted" };
            ListItem::new(Line::from(vec![
                Span::styled("▌", Style::new().fg(hex(&t.color))),
                Span::raw(format!("{:<12}", t.label)),
                Span::styled(
                    format!(
                        "{} {} {}{counted}",
                        t.kind.name(),
                        t.category.name(),
                        t.start.format(TIME_FMT)
                    ),
                    Style::new().fg(DIM),
                ),
            ]))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(app.lib_type));
    f.render_stateful_widget(
        List::new(items)
            .block(
                Block::bordered()
                    .title(" Session types ")
                    .title_bottom(Line::from(" n new · e edit · d delete · R reset ").fg(DIM))
                    .border_style(focus_style(app.lib_focus == LibFocus::Types)),
            )
            .highlight_style(Style::new().bg(SURFACE))
            .highlight_symbol("› "),
        left,
        &mut state,
    );

    let Some(t) = types.get(app.lib_type) else {
        return;
    };
    let rows = t.efforts.iter().map(|e| {
        Row::new(vec![
            e.label.clone(),
            format!("{}", e.rpe),
            format!("{}", e.met),
            format!("{} min", e.dur),
            if e.legs {
                "heavy legs".into()
            } else {
                String::new()
            },
            e.key.clone(),
        ])
    });
    let mut state = TableState::default().with_selected(Some(app.lib_effort));
    f.render_stateful_widget(
        Table::new(
            rows,
            [
                Constraint::Fill(2),
                Constraint::Length(5),
                Constraint::Length(5),
                Constraint::Length(8),
                Constraint::Length(11),
                Constraint::Fill(1),
            ],
        )
        .header(Row::new(["Effort", "RPE", "MET", "Default", "", "key"]).fg(DIM))
        .block(
            Block::bordered()
                .title(Line::from(vec![
                    Span::raw(" Efforts of "),
                    Span::styled(t.label.clone(), Style::new().fg(hex(&t.color)).bold()),
                    Span::raw(" "),
                ]))
                .title_bottom(Line::from(" h/l switch pane · n new · e edit · d delete ").fg(DIM))
                .border_style(focus_style(app.lib_focus == LibFocus::Efforts)),
        )
        .row_highlight_style(Style::new().bg(SURFACE))
        .highlight_symbol("› "),
        right,
        &mut state,
    );
}
