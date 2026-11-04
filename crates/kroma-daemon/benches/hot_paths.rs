//! Exercise the production modules without initializing a compositor, GPU, or audio device.

#[path = "../src/data.rs"]
#[allow(unused_imports)] // Harness-free benches remove #[test] functions, but keep their imports.
mod data;
#[path = "../src/render_demand.rs"]
#[allow(unused_imports)]
mod render_demand;
#[path = "../src/timing.rs"]
mod timing;
#[path = "../src/worker.rs"]
#[allow(unused_imports)]
mod worker;

use std::{hint::black_box, time::Duration};

use criterion::{Criterion, criterion_group, criterion_main};
use kroma_shared::traits::DataProvider;

fn hot_paths(c: &mut Criterion) {
    let provider = data::SystemDataProvider::new().unwrap();
    c.bench_function("data/system_stats", |b| {
        b.iter(|| black_box(provider.get_system_stats()));
    });

    let mut demand = render_demand::RenderDemand::default();
    demand.set(1, vec!["DP-1".into(), "eDP-1".into()]);
    demand.set(2, vec!["DP-1".into(), "HDMI-A-1".into()]);
    c.bench_function("render_demand/union", |b| {
        b.iter(|| black_box(demand.monitors()));
    });
    demand.remove(1);
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(50)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3));
    targets = hot_paths
}
criterion_main!(benches);
