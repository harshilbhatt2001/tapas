//! Planning maths: per-day energy, weekly checks, clashes, summary, guidance and the
//! library lookups all of them lean on.
use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use tapas::{
    calc,
    library::{default_library, new_item},
    model::{Item, Kind, Library, Plan, Profile},
};

mod common;

/// The plan fixtures shared by several groups, by bench parameter name.
fn fixtures(names: &[&str]) -> Vec<(&'static str, Library, Plan)> {
    names
        .iter()
        .map(|&n| match n {
            "starter" => {
                let (l, p) = common::starter();
                ("starter", l, p)
            }
            "dense" => {
                let (l, p) = common::dense();
                ("dense", l, p)
            }
            "huge" => {
                let (l, p) = common::huge();
                ("huge", l, p)
            }
            _ => unreachable!("unknown fixture {n}"),
        })
        .collect()
}

fn day_calc(c: &mut Criterion) {
    let profile = Profile::default();
    let mut g = c.benchmark_group("day_calc");
    for (name, lib, plan) in fixtures(&["starter", "dense"]) {
        g.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.iter(|| {
                for items in &plan.days {
                    black_box(calc::day_calc(
                        black_box(&lib),
                        black_box(&profile),
                        black_box(items),
                    ));
                }
            });
        });
    }
    g.finish();
}

fn checks(c: &mut Criterion) {
    let mut g = c.benchmark_group("checks");
    for (name, lib, plan) in fixtures(&["starter", "dense", "huge"]) {
        g.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.iter(|| calc::checks(black_box(&lib), black_box(&plan)));
        });
    }
    g.finish();
}

fn clashes(c: &mut Criterion) {
    let mut g = c.benchmark_group("clashes");
    for (name, _, plan) in fixtures(&["dense", "huge"]) {
        g.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.iter(|| calc::clashes(black_box(&plan)));
        });
    }
    g.finish();
}

fn summary(c: &mut Criterion) {
    let mut cases: Vec<(&str, Library, Plan)> = fixtures(&["starter", "huge"]);
    // The huge week against a library with 40 extra types for the per-type scans.
    cases.push(("huge_lib+40", common::library_with(40), common::huge().1));
    let mut g = c.benchmark_group("summary");
    for (name, lib, plan) in &cases {
        g.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.iter(|| calc::summary(black_box(lib), black_box(plan)));
        });
    }
    g.finish();
}

fn guide(c: &mut Criterion) {
    let lib = default_library();
    let weight = Profile::default().weight;
    let items: Vec<(Kind, Item)> = Kind::ALL
        .into_iter()
        .filter_map(|k| {
            let t = lib.types.iter().find(|t| t.kind == k)?;
            Some((k, new_item(&lib, &t.key)?))
        })
        .collect();
    let mut g = c.benchmark_group("guide");
    for (kind, it) in &items {
        g.bench_with_input(BenchmarkId::from_parameter(kind.name()), it, |b, it| {
            b.iter(|| calc::guide(black_box(&lib), black_box(weight), black_box(it)));
        });
    }
    g.finish();
}

fn library(c: &mut Criterion) {
    let mut g = c.benchmark_group("library");
    for extra in [0, 40, 200] {
        let lib = common::library_with(extra);
        let last = lib.types.last().expect("library has types");
        // Last type and last effort: the longest scan for both lookups.
        let it = Item {
            type_key: last.key.clone(),
            effort: last.efforts.last().expect("type has efforts").key.clone(),
            ..new_item(&lib, &last.key).expect("type exists")
        };
        let n = lib.types.len();
        g.bench_with_input(BenchmarkId::new("get", n), &it.type_key, |b, key| {
            b.iter(|| lib.get(black_box(key)));
        });
        g.bench_with_input(BenchmarkId::new("resolve", n), &it, |b, it| {
            b.iter(|| lib.resolve(black_box(it)));
        });
    }
    g.finish();
}

/// Everything one week-board frame computes from the plan.
fn frame_calc(c: &mut Criterion) {
    let profile = Profile::default();
    let mut g = c.benchmark_group("frame_calc");
    for (name, lib, plan) in fixtures(&["starter", "dense"]) {
        g.bench_function(BenchmarkId::from_parameter(name), |b| {
            b.iter(|| {
                let (lib, plan) = (black_box(&lib), black_box(&plan));
                let days = plan
                    .days
                    .each_ref()
                    .map(|items| calc::day_calc(lib, &profile, items));
                (
                    days,
                    calc::checks(lib, plan),
                    calc::clashes(plan),
                    calc::summary(lib, plan),
                )
            });
        });
    }
    g.finish();
}

criterion_group!(
    group, day_calc, checks, clashes, summary, guide, library, frame_calc
);
criterion_main!(group);
