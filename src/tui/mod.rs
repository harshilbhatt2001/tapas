//! The ratatui front end: `tapas` without a subcommand.

mod app;
mod editor;
mod export;
mod library;
mod plans;
mod profile;
mod week;
mod widgets;

use std::{io::stdout, sync::mpsc, time::Duration};

use anyhow::Result;
use ratatui::{
    Frame,
    crossterm::{
        event::{self, DisableMouseCapture, EnableMouseCapture},
        execute,
    },
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Paragraph, Tabs},
};

pub use app::App;
use app::{Background, Modal, Screen};
use widgets::{DIM, LINE, WARN, message_popup};

use crate::{
    calc,
    model::{Device, Store},
    storage::Paths,
};

/// Run the TUI until the user quits. Mouse capture is released on exit and on panic.
pub fn run(paths: Paths, store: Store, device: Device) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let (tx, rx) = mpsc::channel();
    let mut app = App::new(paths, store, device);
    app.day = week::today_index();
    app.clamp();
    app.bg = Some(Background {
        handle: rt.handle().clone(),
        tx,
    });

    // Chained under ratatui's own hook, which restores the terminal first.
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(stdout(), DisableMouseCapture);
        prev(info);
    }));
    let mut terminal = ratatui::init();
    execute!(stdout(), EnableMouseCapture)?;
    let res = (|| -> Result<()> {
        while !app.quit {
            terminal.draw(|f| draw(f, &mut app))?;
            if event::poll(Duration::from_millis(100))? {
                app.handle_event(&event::read()?);
            }
            while let Ok(r) = rx.try_recv() {
                app.on_bg(r);
            }
            app.tick = app.tick.wrapping_add(1);
        }
        Ok(())
    })();
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    // Don't wait for in-flight Google calls.
    rt.shutdown_background();
    res
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let [header, tabs, body, footer] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(f.area());
    draw_header(f, app, header);
    f.render_widget(
        Tabs::new(
            Screen::ALL
                .iter()
                .enumerate()
                .map(|(i, s)| format!("{} {}", i + 1, s.title())),
        )
        .select(app.screen.index())
        .style(Style::new().fg(DIM))
        .highlight_style(Style::new().fg(Color::White).bold().underlined())
        .divider("·"),
        tabs,
    );
    match app.screen {
        Screen::Week => week::draw(f, app, body),
        Screen::Plans => plans::draw(f, app, body),
        Screen::Library => library::draw(f, app, body),
        Screen::Profile => profile::draw(f, app, body),
        Screen::Export => export::draw(f, app, body),
    }
    draw_footer(f, app, footer);
    draw_modal(f, app);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let [left, bib] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(34)]).areas(area);
    let plan = app.plan();
    let s = calc::summary(&app.store.library, plan);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::raw(" tapas").bold(),
                Span::styled("  training week planner", Style::new().fg(DIM)),
            ]),
            Line::from(vec![
                Span::styled(" plan ", Style::new().fg(DIM)),
                Span::raw(plan.name.clone()).bold(),
                Span::styled(
                    format!("  ({} of {})", app.active() + 1, app.store.plans.len()),
                    Style::new().fg(DIM),
                ),
            ]),
            {
                let mut l = export::google_state(app);
                l.spans.insert(0, Span::raw(" "));
                l
            },
        ]),
        left,
    );
    let b = |n: usize| Span::raw(n.to_string()).bold();
    let d = |t: &'static str| Span::styled(t, Style::new().fg(DIM));
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::raw(format!("{}", s.hours)).bold(),
                d(" h this week"),
            ]),
            Line::from(vec![
                b(s.sessions),
                d(" sessions  "),
                b(s.hard),
                d(" hard  "),
                b(s.long),
                d(" long"),
            ]),
        ])
        .centered()
        .block(Block::bordered().border_style(Style::new().fg(Color::White))),
        bib,
    );
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let [left, right] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(18)]).areas(area);
    let status = match app.busy_text() {
        Some(b) => Line::styled(format!(" {b}"), Style::new().fg(Color::White)),
        None if app.status.error => {
            Line::styled(format!(" {}", app.status.text), Style::new().fg(WARN))
        }
        None => Line::styled(format!(" {}", app.status.text), Style::new().fg(DIM)),
    };
    f.render_widget(Paragraph::new(status), left);
    f.render_widget(
        Paragraph::new("? help · q quit ").fg(DIM).right_aligned(),
        right,
    );
}

fn draw_modal(f: &mut Frame, app: &mut App) {
    match std::mem::take(&mut app.modal) {
        Modal::None => {}
        Modal::Help => {
            message_popup(f, "Keys", help(app.screen), Color::White);
            app.modal = Modal::Help;
        }
        Modal::Confirm { msg, action } => {
            let body = Text::from(vec![
                Line::raw(msg.clone()),
                Line::raw(""),
                Line::styled("y / Enter yes · n / Esc no", Style::new().fg(DIM)),
            ]);
            message_popup(f, "Confirm", body, WARN);
            app.modal = Modal::Confirm { msg, action };
        }
        Modal::AddPicker(mut state) => {
            week::draw_picker(f, app, &mut state);
            app.modal = Modal::AddPicker(state);
        }
        Modal::Form(form, kind) => {
            match &kind {
                app::FormKind::Item(id) => editor::draw(f, app, &form, id),
                _ => form.render_popup(f),
            }
            app.modal = Modal::Form(form, kind);
        }
    }
}

fn help(screen: Screen) -> Text<'static> {
    let global = [("1-5", "switch screen"), ("?", "this help"), ("q", "quit")];
    let keys: &[(&str, &str)] = match screen {
        Screen::Week => &[
            ("h/l ←/→", "previous / next day"),
            ("j/k ↓/↑", "next / previous session"),
            ("a", "add a session to the day"),
            ("Enter e", "edit session"),
            ("y", "duplicate session"),
            ("d", "delete session"),
            ("< >", "move session to previous / next day"),
            ("S", "load the starter week"),
            ("X", "clear the week"),
            ("c", "show / hide commute rides from Google Health"),
            ("click", "select a session or day"),
        ],
        Screen::Plans => &[
            ("j/k", "select plan"),
            ("Enter", "make active"),
            ("n", "new empty plan"),
            ("c", "copy plan"),
            ("r", "rename plan"),
            ("d", "delete plan"),
        ],
        Screen::Library => &[
            ("h/l Tab", "types / efforts pane"),
            ("j/k", "select"),
            ("Enter e", "edit type or effort"),
            ("n", "new type or effort"),
            ("d", "delete type or effort"),
            ("R", "reset library to defaults"),
        ],
        Screen::Profile => &[
            ("Enter e", "edit weight and base intake"),
            ("w", "latest weight from Google Health"),
        ],
        Screen::Export => &[
            ("Enter e", "edit export settings"),
            ("i", "write .ics to Downloads"),
            ("c", "write Google CSV to Downloads"),
            ("g", "push plan to Google Calendar"),
            ("f", "fetch this week's workouts"),
        ],
    };
    let row = |(k, v): &(&str, &str)| {
        Line::from(vec![
            Span::styled(format!(" {k:<10}"), Style::new().bold()),
            Span::raw(format!("{v} ")),
        ])
    };
    let mut lines: Vec<Line> = keys.iter().map(row).collect();
    lines.push(Line::styled(" ─────", Style::new().fg(LINE)));
    lines.extend(global.iter().map(row));
    lines.push(Line::styled(
        " Forms: Tab/Shift-Tab field, ←/→ choose,",
        Style::new().fg(DIM),
    ));
    lines.push(Line::styled(
        " Ctrl-s save, Esc cancel. Any key closes.",
        Style::new().fg(DIM),
    ));
    Text::from(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::starter_plan;
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    fn app() -> App {
        let store = Store {
            plans: vec![starter_plan("Base")],
            ..Store::default()
        };
        App::new(
            Paths::under(std::path::Path::new("/nonexistent/tapas")),
            store,
            Device::default(),
        )
    }

    fn text(buf: &Buffer) -> String {
        let w = buf.area.width as usize;
        buf.content
            .chunks(w)
            .map(|row| {
                row.iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render(app: &mut App, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(f, app)).unwrap();
        text(t.backend().buffer())
    }

    #[test]
    fn week_board_shows_all_days() {
        let mut a = app();
        let s = render(&mut a, 180, 50);
        for d in crate::model::DAYS {
            assert!(s.contains(d), "missing {d}");
        }
        assert!(s.contains("10.7 h this week"));
        assert!(s.contains("Run: Run/walk"));
        assert!(s.contains("Big"));
        assert!(s.contains("No conflicts or load problems found."));
        assert!(s.contains("Padel 1h30 (not in weekly hours)"));
        // Clicking Saturday's first card selects it.
        let hit = a
            .hits
            .iter()
            .find(|h| h.day == 5 && h.card == Some(0))
            .copied()
            .unwrap();
        a.handle_event(&ratatui::crossterm::event::Event::Mouse(
            ratatui::crossterm::event::MouseEvent {
                kind: ratatui::crossterm::event::MouseEventKind::Down(
                    ratatui::crossterm::event::MouseButton::Left,
                ),
                column: hit.area.x + 1,
                row: hit.area.y,
                modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
            },
        ));
        assert_eq!((a.day, a.card), (5, 0));
    }

    #[test]
    fn narrow_terminal_shows_one_day() {
        let mut a = app();
        a.day = 5;
        let s = render(&mut a, 100, 40);
        assert!(s.contains("Sat"));
        assert!(!s.contains(" Mon "));
        assert!(s.contains("Bike: Long ride"));
        assert!(s.contains("fuel on the go"));
        assert!(s.contains("4498 kcal"));
    }

    #[test]
    fn clash_is_marked_and_editor_renders() {
        let mut a = app();
        // Tuesday: move the swim onto the gym slot.
        let swim = a.store.plans[0].days[1][2].id.clone();
        a.store.plans[0].days[1][2].start = chrono::NaiveTime::from_hms_opt(7, 15, 0).unwrap();
        a.day = 1;
        let s = render(&mut a, 180, 50);
        assert!(s.contains("clash"));
        assert!(s.contains("Tue: Gym (heavy strength) overlaps Swim (technique)."));
        editor::open(&mut a, &swim);
        let s = render(&mut a, 180, 50);
        assert!(s.contains("Effort"));
        assert!(s.contains("‹ Technique ›"));
        assert!(s.contains("About"));
    }
}
