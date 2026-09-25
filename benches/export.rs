//! Placeholder; filled in by the export bench task.
use criterion::{Criterion, criterion_group, criterion_main};

mod common;

fn benches(_c: &mut Criterion) {}

criterion_group!(group, benches);
criterion_main!(group);
