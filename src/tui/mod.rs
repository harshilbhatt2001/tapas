//! The ratatui front end: `tapas` without a subcommand.

mod app;
mod editor;
mod export;
mod library;
mod plans;
mod profile;
mod sync;
mod week;
mod widgets;

use std::{
    io::stdout,
    sync::mpsc,
    time::{Duration, Instant},
};

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

pub use app::{App, Board};
use app::{Background, Modal, Screen};
use widgets::{DIM, LINE, WARN, message_popup};

use crate::{
    calc,
    model::{Device, Store},
    storage::Paths,
};

/// Run the TUI until the user quits. Mouse capture is released on exit and on panic. Drive
/// sync runs in the background; quitting waits up to 5 s for unsynced edits.
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
    app.set_board(app.board().clone());
    sync::start(&mut app, false);

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
            sync::tick(&mut app, Instant::now());
            app.tick = app.tick.wrapping_add(1);
        }
        if app.unsynced || app.busy.contains(&app::Task::Sync) {
            app.info("Syncing with Google Drive before quitting…");
            terminal.draw(|f| draw(f, &mut app))?;
        }
        Ok(())
    })();
    let left_behind = sync::on_quit(&mut app, &rx);
    let _ = execute!(stdout(), DisableMouseCapture);
    ratatui::restore();
    if let Some(msg) = left_behind {
        eprintln!("{msg}");
    }
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
                Span::raw(" "),
                Span::raw(week::board_title(app)).bold(),
            ]),
            {
                let mut l = export::google_state(app);
                l.spans.insert(0, Span::raw(" "));
                l
            },
            {
                let mut l = sync::status_line(app);
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
        Modal::Picker(picker, mut state) => {
            week::draw_picker(f, app, &picker, &mut state);
            app.modal = Modal::Picker(picker, state);
        }
        Modal::WorkoutDetails { title, workouts } => {
            let mut lines: Vec<Line> = Vec::new();
            for (i, metrics) in workouts.iter().enumerate() {
                if i > 0 {
                    lines.push(Line::raw(""));
                }
                lines.extend(metrics.iter().map(|m| Line::raw(m.clone())));
            }
            lines.push(Line::styled("Any key closes", Style::new().fg(DIM)));
            message_popup(f, &title, Text::from(lines), Color::White);
            app.modal = Modal::WorkoutDetails { title, workouts };
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
            ("[ ]", "previous / next week"),
            ("t", "this week"),
            ("g", "go to the week of a date"),
            ("p", "apply a template to this week"),
            ("a", "add a session to the day"),
            ("Enter", "edit session; metrics of a done block or workout"),
            ("e", "edit session"),
            ("m", "link the session to a Google Health workout"),
            ("y", "duplicate session"),
            ("d", "delete session"),
            ("< >", "move session to previous / next day"),
            ("S", "load the starter week"),
            ("X", "clear the week"),
            ("r", "fetch this week from Google Health again"),
            ("c", "show / hide commute rides from Google Health"),
            ("click", "select a session or day"),
        ],
        Screen::Plans => &[
            ("j/k", "select template"),
            ("Enter", "edit on the board"),
            ("A", "apply to a range of weeks"),
            ("n", "new empty template"),
            ("c", "copy template"),
            ("r", "rename template"),
            ("d", "delete template"),
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
            ("s", "sync with Google Drive now"),
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
    use crate::{
        google::health::Workout, health_cache::WeekActuals, library::starter_plan, model::Item,
    };
    use chrono::{Days, Local, NaiveDate, NaiveTime, TimeDelta, TimeZone, Utc};
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    fn app() -> App {
        let store = Store {
            plans: vec![starter_plan("Base")],
            ..Store::default()
        };
        let id = store.plans[0].id.clone();
        let mut a = App::new(
            Paths::under(std::path::Path::new("/nonexistent/tapas")),
            store,
            Device::default(),
        );
        a.set_board(Board::Template(id));
        a
    }

    fn press(a: &mut App, c: char) {
        a.handle_event(&ratatui::crossterm::event::Event::Key(
            ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Char(c)),
        ));
    }

    #[test]
    fn dated_weeks_are_stored_on_first_edit() {
        let mut a = app();
        let this = week::this_monday();
        press(&mut a, 't');
        assert_eq!(a.board(), &Board::Week(this));
        let s = render(&mut a, 180, 50);
        assert!(s.contains("empty · p to apply"), "{s}");
        assert!(s.contains(&format!("Week of {}", this.format("%a %-d %b %Y"))));
        assert!(a.store.weeks.is_empty());

        press(&mut a, 'S');
        assert_eq!(a.store.weeks.len(), 1);
        assert_eq!(a.store.weeks[0].monday, this);
        assert_eq!(a.store.weeks[0].plan.name, "");

        press(&mut a, ']');
        let next = this + chrono::Days::new(7);
        assert_eq!(a.board(), &Board::Week(next));
        assert!(a.plan().is_empty());
        press(&mut a, 'p');
        a.handle_event(&ratatui::crossterm::event::Event::Key(
            ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Enter),
        ));
        let mondays: Vec<_> = a.store.weeks.iter().map(|w| w.monday).collect();
        assert_eq!(mondays, [this, next]);
        assert_eq!(a.plan().name, "Base");
        assert!(render(&mut a, 180, 50).contains("· from \"Base\""));

        press(&mut a, '[');
        press(&mut a, '[');
        assert_eq!(a.board(), &Board::Week(this - chrono::Days::new(7)));
    }

    fn workout(
        id: &str,
        exercise_type: &str,
        date: NaiveDate,
        start: &str,
        minutes: u32,
    ) -> Workout {
        let start = Local
            .from_local_datetime(&date.and_time(NaiveTime::parse_from_str(start, "%H:%M").unwrap()))
            .unwrap();
        Workout {
            id: id.into(),
            exercise_type: exercise_type.into(),
            day: date,
            start,
            end: start + TimeDelta::minutes(minutes.into()),
            minutes,
            active_min: None,
            kcal: None,
            avg_hr: None,
            distance_m: None,
            elevation_m: None,
            pace_s_per_km: None,
            swim_lengths: None,
            azm: None,
            google_zones: None,
            notes: None,
            display_name: None,
        }
    }

    /// Last week's Monday: a run at 07:00 done at 07:05, a commute ride at 08:30 and a missed
    /// swim at 18:00. Returns the app and the run's and swim's ids.
    fn last_week_app(dir: &std::path::Path) -> (App, String, String) {
        let mut a = App::new(Paths::under(dir), Store::default(), Device::default());
        let monday = week::this_monday() - Days::new(7);
        a.set_board(Board::Week(monday));
        let lib = a.store.library.clone();
        let at = |ty: &str, hhmm: &str| Item {
            start: NaiveTime::parse_from_str(hhmm, "%H:%M").unwrap(),
            ..crate::library::new_item(&lib, ty).unwrap()
        };
        let (run, swim) = (at("run", "07:00"), at("swim", "18:00"));
        let ids = (run.id.clone(), swim.id.clone());
        a.plan_mut().days[0] = vec![run, swim];
        a.actuals = Some((
            monday,
            WeekActuals {
                fetched_at: Utc::now(),
                workouts: vec![
                    workout("ride", "CYCLING", monday, "08:30", 20),
                    workout("run", "RUNNING", monday, "07:05", 40),
                ],
                heart_rate: vec![],
            },
        ));
        a.day = 0;
        (a, ids.0, ids.1)
    }

    #[test]
    fn selection_indexes_the_visible_slots() {
        let dir = tempfile::tempdir().unwrap();
        let (mut a, run, swim) = last_week_app(dir.path());
        a.card = 1;
        assert_eq!(week::selected_id(&a), Some(swim.clone()));

        press(&mut a, 'c');
        assert_eq!(
            week::selected_id(&a),
            None,
            "the commute ride is card 1 now"
        );
        a.card = 0;
        assert_eq!(week::selected_id(&a), Some(run));
        a.card = 2;
        assert_eq!(week::selected_id(&a), Some(swim));

        let s = render(&mut a, 180, 50);
        assert!(s.contains("✓ done"), "{s}");
        assert!(s.contains("commute"), "{s}");
        assert!(s.contains("load 0 (TRIMP)"), "{s}");

        a.card = 1;
        press(&mut a, 'e');
        assert!(
            matches!(a.modal, Modal::None),
            "a commute has no session to edit"
        );
        a.card = 0;
        a.handle_event(&ratatui::crossterm::event::Event::Key(
            ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Enter),
        ));
        assert!(matches!(a.modal, Modal::WorkoutDetails { .. }));
    }

    #[test]
    fn m_links_the_session_to_the_chosen_workout() {
        let dir = tempfile::tempdir().unwrap();
        let (mut a, _, swim) = last_week_app(dir.path());
        a.card = 1;
        press(&mut a, 'm');
        // Choices: the run, the ride, "none", then "auto", selected as the swim has no link.
        press(&mut a, 'k');
        press(&mut a, 'k');
        a.handle_event(&ratatui::crossterm::event::Event::Key(
            ratatui::crossterm::event::KeyEvent::from(ratatui::crossterm::event::KeyCode::Enter),
        ));
        let (d, i) = a.plan().find(&swim).unwrap();
        assert_eq!(a.plan().days[d][i].workout.as_deref(), Some("ride"));
        assert_eq!(week::selected_id(&a), Some(swim));
        let (saved, _) = crate::storage::load(&a.paths).unwrap();
        let monday = a.board_monday().unwrap();
        let saved = &saved.week(monday).unwrap().plan.days[d][i];
        assert_eq!(saved.workout.as_deref(), Some("ride"));
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
