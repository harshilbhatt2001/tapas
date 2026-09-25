//! Store persistence: JSON encoding, `storage::save` (run on every edit), and `storage::load`.
//!
//! Set `TAPAS_BENCH_DIR` to also bench `save` on another filesystem (ids end in `/bench_dir`).
use std::{hint::black_box, path::Path, time::Duration};

use criterion::{
    BenchmarkGroup, Criterion, Throughput, criterion_group, criterion_main, measurement::WallTime,
};
use tapas::{
    model::Store,
    storage::{self, Paths},
};
use tempfile::TempDir;

mod common;

fn stores() -> [(&'static str, Store); 2] {
    [
        ("starter", common::store_starter()),
        ("many", common::store_many()),
    ]
}

/// Settings for groups that touch the disk: fewer, longer samples.
fn io_group<'a>(c: &'a mut Criterion, name: &str) -> BenchmarkGroup<'a, WallTime> {
    let mut group = c.benchmark_group(name);
    group
        .sample_size(50)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3));
    group
}

fn tempdir_in(parent: &Path) -> TempDir {
    tempfile::Builder::new()
        .prefix("tapas-bench-")
        .tempdir_in(parent)
        .expect("create temp dir")
}

fn pretty(store: &Store) -> Vec<u8> {
    serde_json::to_vec_pretty(store).expect("serialize")
}

fn bytes(v: &[u8]) -> Throughput {
    Throughput::Bytes(u64::try_from(v.len()).expect("fits u64"))
}

fn serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("serialize");
    for (name, store) in stores() {
        let json = pretty(&store);
        eprintln!(
            "storage: {name} store is {} bytes of pretty JSON",
            json.len()
        );
        group.throughput(bytes(&json));
        group.bench_function(format!("pretty/{name}"), |b| {
            b.iter(|| serde_json::to_vec_pretty(black_box(&store)).expect("serialize"));
        });
        group.bench_function(format!("compact/{name}"), |b| {
            b.iter(|| serde_json::to_vec(black_box(&store)).expect("serialize"));
        });
    }
    group.finish();
}

fn save(c: &mut Criterion) {
    let local = tempfile::tempdir().expect("create temp dir");
    let extra = std::env::var_os("TAPAS_BENCH_DIR").map(|d| tempdir_in(Path::new(&d)));
    eprintln!("storage: save into {}", local.path().display());
    if let Some(dir) = &extra {
        eprintln!(
            "storage: save also into {} (TAPAS_BENCH_DIR)",
            dir.path().display()
        );
    }
    let mut group = io_group(c, "save");
    for (name, store) in stores() {
        group.throughput(bytes(&pretty(&store)));
        let dirs = std::iter::once((name.to_owned(), &local))
            .chain(extra.iter().map(|d| (format!("{name}/bench_dir"), d)));
        for (id, dir) in dirs {
            let paths = Paths::under(&dir.path().join(name));
            group.bench_function(id, |b| {
                let mut store = store.clone();
                b.iter(|| storage::save(black_box(&paths), black_box(&mut store)).expect("save"));
            });
        }
    }
    group.finish();
}

fn load(c: &mut Criterion) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let mut group = io_group(c, "load");
    for (name, store) in stores() {
        let paths = Paths::under(&dir.path().join(name));
        storage::save(&paths, &mut store.clone()).expect("save");
        group.throughput(bytes(&pretty(&store)));
        group.bench_function(name, |b| {
            b.iter(|| storage::load(black_box(&paths)).expect("load"));
        });
    }
    group.finish();
}

fn deserialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("deserialize");
    for (name, store) in stores() {
        let json = pretty(&store);
        group.throughput(bytes(&json));
        group.bench_function(name, |b| {
            b.iter(|| serde_json::from_slice::<Store>(black_box(&json)).expect("parse"));
        });
    }
    group.finish();
}

criterion_group!(benches, serialize, save, load, deserialize);
criterion_main!(benches);
