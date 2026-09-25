//! Calendar export: building the week's events and serialising them to iCalendar, Google
//! CSV and Calendar API events.
use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use tapas::{
    export::{self, ExportEvent, ExportOpts},
    google::calendar,
    model::{Library, Plan, Store},
    services,
};

mod common;

/// One week from the bench Monday, life items included.
fn opts() -> ExportOpts {
    ExportOpts {
        first_monday: common::monday(),
        weeks: 1,
        include_life: true,
    }
}

fn store((library, plan): (Library, Plan)) -> Store {
    Store {
        library,
        plans: vec![plan],
        ..Store::default()
    }
}

/// The starter store and its exported first week.
fn starter_events() -> (Store, Vec<ExportEvent>) {
    let s = store(common::starter());
    let evs = export::events(&s, s.plan_or_first(None), &opts());
    (s, evs)
}

fn events(c: &mut Criterion) {
    let opts = opts();
    let mut g = c.benchmark_group("events");
    for (name, s) in [
        ("starter", store(common::starter())),
        ("huge", store(common::huge())),
    ] {
        g.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.iter(|| {
                export::events(
                    black_box(&s),
                    black_box(s.plan_or_first(None)),
                    black_box(&opts),
                )
            });
        });
    }
    g.finish();
}

fn to_ics(c: &mut Criterion) {
    let (s, evs) = starter_events();
    let mut g = c.benchmark_group("to_ics");
    for weeks in [1, 52] {
        g.bench_with_input(BenchmarkId::from_parameter(weeks), &weeks, |b, &w| {
            b.iter(|| {
                export::to_ics(
                    black_box(s.plan_or_first(None)),
                    black_box(&evs),
                    black_box(w),
                )
            });
        });
    }
    g.finish();
}

fn to_google_csv(c: &mut Criterion) {
    let (_, evs) = starter_events();
    let mut g = c.benchmark_group("to_google_csv");
    for weeks in [1u32, 12, 52] {
        g.throughput(Throughput::Elements(evs.len() as u64 * u64::from(weeks)));
        g.bench_with_input(BenchmarkId::from_parameter(weeks), &weeks, |b, &w| {
            b.iter(|| export::to_google_csv(black_box(&evs), black_box(w)).expect("csv"));
        });
    }
    g.finish();
}

fn to_event(c: &mut Criterion) {
    let (s, evs) = starter_events();
    let cal = services::cal_events(&evs);
    let plan_id = &s.plan_or_first(None).id;
    let mut g = c.benchmark_group("to_event");
    g.throughput(Throughput::Elements(cal.len() as u64));
    g.bench_function(BenchmarkId::from_parameter("starter"), |b| {
        b.iter(|| {
            black_box(&cal)
                .iter()
                .map(|ev| calendar::to_event(ev, plan_id, 1, black_box("Europe/Amsterdam")))
                .collect::<anyhow::Result<Vec<_>>>()
                .expect("valid events")
        });
    });
    g.finish();
}

criterion_group!(group, events, to_ics, to_google_csv, to_event);
criterion_main!(group);
