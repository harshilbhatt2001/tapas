//! B2: full-frame `tui::draw` on a `TestBackend`. The real loop redraws at least every 100 ms
//! even when idle, so the steady-state per-frame cost is what matters.
use std::hint::black_box;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers},
};
use tapas::{
    model::{Library, Plan, Store},
    storage::Paths,
    tui::{App, draw},
};
use tempfile::TempDir;

mod common;

/// Full board and the single-day view (below 140 columns).
const SIZES: [(u16, u16); 2] = [(200, 60), (120, 40)];

/// Screen names in tab order; tab `n` is selected with key `'1' + n`.
const SCREENS: [&str; 5] = ["week", "plans", "library", "profile", "export"];

/// A deterministic app: fixed Monday, first day selected, paths under a throwaway dir so a
/// stray save could never touch the real store. The `TempDir` must outlive the app.
fn app(store: Store) -> (TempDir, App) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = App::new(Paths::under(dir.path()), store);
    app.first_monday = common::monday();
    app.day = 0;
    app.card = 0;
    (dir, app)
}

fn store_of((library, plan): (Library, Plan)) -> Store {
    Store {
        library,
        plans: vec![plan],
        ..Store::default()
    }
}

/// `Screen` is not public, so navigate the way a user does.
fn press(app: &mut App, c: char) {
    app.handle_event(&Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    )));
}

fn terminal((w, h): (u16, u16)) -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(w, h)).expect("infallible")
}

fn frame(t: &mut Terminal<TestBackend>, app: &mut App) {
    let done = t.draw(|f| draw(f, app)).expect("infallible");
    black_box(done.buffer);
}

fn buffer_text(t: &Terminal<TestBackend>) -> String {
    t.backend()
        .buffer()
        .content
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect()
}

/// Redraw the same app on one terminal, like the idle event loop.
fn steady(b: &mut criterion::Bencher, size: (u16, u16), app: &mut App) {
    let mut t = terminal(size);
    b.iter(|| frame(&mut t, app));
}

fn screens(c: &mut Criterion) {
    let mut g = c.benchmark_group("screens");
    for (n, name) in SCREENS.iter().enumerate() {
        for size in SIZES {
            let (_dir, mut app) = app(common::store_starter());
            press(
                &mut app,
                char::from(b'1' + u8::try_from(n).expect("5 screens")),
            );
            let id = BenchmarkId::new(*name, format!("{}x{}", size.0, size.1));
            g.bench_function(id, |b| steady(b, size, &mut app));
        }
    }
    g.finish();
}

/// A `common` library-and-plan builder.
type Fixture = fn() -> (Library, Plan);

fn week_plan_size(c: &mut Criterion) {
    let mut g = c.benchmark_group("week_plan_size");
    let fixtures: [(&str, Fixture); 3] = [
        ("starter", common::starter),
        ("dense", common::dense),
        ("huge", common::huge),
    ];
    for (name, fixture) in fixtures {
        let (_dir, mut app) = app(store_of(fixture()));
        g.bench_function(name, |b| steady(b, SIZES[0], &mut app));
    }
    g.finish();
}

fn editor(c: &mut Criterion) {
    let mut g = c.benchmark_group("editor");
    let (_dir, mut app) = app(common::store_starter());
    // `e` edits the selected card: Monday's first session.
    press(&mut app, 'e');
    let mut t = terminal(SIZES[0]);
    frame(&mut t, &mut app);
    assert!(
        buffer_text(&t).contains("Duration min"),
        "item editor did not open"
    );
    g.bench_function("week/200x60", |b| steady(b, SIZES[0], &mut app));
    g.finish();
}

/// A fresh terminal diffs against an empty buffer and repaints every cell; a reused one only
/// flushes what changed (nothing, when idle).
fn first_frame(c: &mut Criterion) {
    let mut g = c.benchmark_group("first_frame");
    let (_dir, mut app) = app(common::store_starter());
    g.bench_function("fresh/200x60", |b| {
        b.iter_batched(
            || terminal(SIZES[0]),
            |mut t| {
                frame(&mut t, &mut app);
                t
            },
            BatchSize::SmallInput,
        );
    });
    g.bench_function("steady/200x60", |b| steady(b, SIZES[0], &mut app));
    g.finish();
}

criterion_group!(group, screens, week_plan_size, editor, first_frame);
criterion_main!(group);
