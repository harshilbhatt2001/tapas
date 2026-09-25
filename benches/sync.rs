//! Google Health sync: planned vs done, exercise type mapping and response page parsing.
use std::hint::black_box;

use chrono::{DateTime, Duration, Utc};
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use serde_json::json;
use tapas::{
    google::health::{map_exercise_type, parse_weights, parse_workouts},
    services,
};

mod common;

/// Points per synthetic response page: the `List` page size.
const POINTS: i64 = 1000;

/// Midnight UTC on `common::monday()`, the first point's time.
fn base() -> DateTime<Utc> {
    common::monday()
        .and_hms_opt(0, 0, 0)
        .expect("valid time")
        .and_utc()
}

/// An exercise page of `POINTS` hourly workouts, shaped like the unit tests' bodies.
fn exercise_body() -> String {
    let points: Vec<_> = (0..POINTS)
        .map(|i| {
            let start = base() + Duration::hours(i);
            json!({"exercise": {
                "interval": {"startTime": start, "endTime": start + Duration::minutes(45)},
                "exerciseType": "RUNNING",
                "metricsSummary": {"caloriesKcal": 512.5},
            }})
        })
        .collect();
    json!({"dataPoints": points, "nextPageToken": "next"}).to_string()
}

/// A weight page of `POINTS` hourly samples.
fn weight_body() -> String {
    let points: Vec<_> = (0..POINTS)
        .map(|i| {
            json!({"weight": {
                "sampleTime": {"physicalTime": base() + Duration::hours(i)},
                "weightGrams": 71_000 + i,
            }})
        })
        .collect();
    json!({"dataPoints": points, "nextPageToken": "next"}).to_string()
}

fn benches(c: &mut Criterion) {
    let (lib, plan) = common::starter();
    let workouts = common::workouts_year();
    c.bench_function("planned_vs_done", |b| {
        b.iter(|| {
            services::planned_vs_done(
                black_box(&lib),
                black_box(&plan),
                common::monday(),
                black_box(&workouts),
                71.1,
            )
        });
    });

    let types = [
        "SWIMMING_OPEN_WATER",
        "MOUNTAIN_BIKING",
        "TRAIL_RUNNING",
        "CALISTHENICS",
        "PADEL",
        "YOGA",
        "SKYDIVING",
        "",
    ];
    c.bench_function("map_exercise_type", |b| {
        b.iter(|| {
            for t in black_box(types) {
                black_box(map_exercise_type(t));
            }
        });
    });

    let mut group = c.benchmark_group("parse");
    // Every point is complete, so parsing measures all of them.
    let body = exercise_body();
    assert_eq!(parse_workouts(&body).expect("valid page").0.len(), 1000);
    group.throughput(Throughput::Bytes(body.len() as u64));
    group.bench_function("exercise_1000", |b| {
        b.iter(|| parse_workouts(black_box(&body)).expect("valid page"));
    });
    let body = weight_body();
    assert_eq!(parse_weights(&body).expect("valid page").0.len(), 1000);
    group.throughput(Throughput::Bytes(body.len() as u64));
    group.bench_function("weight_1000", |b| {
        b.iter(|| parse_weights(black_box(&body)).expect("valid page"));
    });
    group.finish();
}

criterion_group!(group, benches);
criterion_main!(group);
