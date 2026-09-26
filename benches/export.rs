//! Calendar export: building dated weeks' events and serialising them to iCalendar, Google
//! CSV and Calendar API events.
use std::hint::black_box;

use chrono::{Days, NaiveDate};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tapas::{
    export::{self, ExportEvent},
    google::calendar,
    model::{Library, Plan, Store},
};

mod common;

/// A day before the bench Monday, so every item is as planned.
fn today() -> NaiveDate {
    common::monday() - Days::new(1)
}

/// A store whose template `plan` is applied to `weeks` weeks from the bench Monday.
fn store((library, plan): (Library, Plan), weeks: u32) -> Store {
    let id = plan.id.clone();
    let mut s = Store {
        library,
        plans: vec![plan],
        ..Store::default()
    };
    assert!(s.apply_template(&id, common::monday(), weeks));
    s
}

/// The starter template applied to `weeks` weeks, life items included.
fn starter_events(weeks: u32) -> Vec<ExportEvent> {
    let s = store(common::starter(), weeks);
    export::range_events(&s, common::monday(), weeks, |_| None, today(), true)
}

fn events(c: &mut Criterion) {
    let mut g = c.benchmark_group("week_events");
    for (name, s) in [
        ("starter", store(common::starter(), 1)),
        ("huge", store(common::huge(), 1)),
    ] {
        g.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.iter(|| export::week_events(black_box(&s), common::monday(), None, today(), true));
        });
    }
    g.finish();
}

fn to_ics(c: &mut Criterion) {
    let mut g = c.benchmark_group("to_ics");
    for weeks in [1, 52] {
        let evs = starter_events(weeks);
        g.bench_with_input(BenchmarkId::from_parameter(weeks), &evs, |b, evs| {
            b.iter(|| export::to_ics("Training", black_box(evs)));
        });
    }
    g.finish();
}

fn to_google_csv(c: &mut Criterion) {
    let mut g = c.benchmark_group("to_google_csv");
    for weeks in [1u32, 12, 52] {
        let evs = starter_events(weeks);
        g.throughput(Throughput::Elements(evs.len() as u64));
        g.bench_with_input(BenchmarkId::from_parameter(weeks), &evs, |b, evs| {
            b.iter(|| export::to_google_csv(black_box(evs)).expect("csv"));
        });
    }
    g.finish();
}

fn to_event(c: &mut Criterion) {
    let evs = starter_events(1);
    let mut g = c.benchmark_group("to_event");
    g.throughput(Throughput::Elements(evs.len() as u64));
    g.bench_function(BenchmarkId::from_parameter("starter"), |b| {
        b.iter(|| {
            black_box(&evs)
                .iter()
                .map(|ev| calendar::to_event(ev, black_box("Europe/Amsterdam")))
                .collect::<anyhow::Result<Vec<_>>>()
                .expect("valid events")
        });
    });
    g.finish();
}

criterion_group!(group, events, to_ics, to_google_csv, to_event);
criterion_main!(group);
