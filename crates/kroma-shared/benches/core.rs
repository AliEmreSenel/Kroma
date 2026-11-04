//! CPU-only workloads; fixture construction and packaging are outside timed loops.

use std::{hint::black_box, time::Duration};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use kroma_shared::{
    ipc::DaemonCommand,
    shade::{CompressionPolicy, LiveShadePackage},
    translator,
    types::{ShadeConfig, ShadeMeta},
};

fn config() -> ShadeConfig {
    ShadeConfig {
        meta: ShadeMeta {
            name: "Benchmark".into(),
            author: "Kroma".into(),
            description: String::new(),
            version: "1.0".into(),
            tags: Vec::new(),
        },
        states: Default::default(),
        rendering: Default::default(),
        transitions: Default::default(),
        transitions_usage: Default::default(),
    }
}

fn translate(c: &mut Criterion) {
    let simple = include_str!("../../../examples/shadertoy/plasma.glsl");
    let channels = "void mainImage(out vec4 c, in vec2 p) { c = texture(iChannel0, p / iResolution.xy) + texture(iChannel3, p / iResolution.xy) * sin(iTime); }";
    let helpers: String = (0..512)
        .map(|i| format!("float helper{i}(float x) {{ return sin(x); }}\n"))
        .collect();
    let large = format!("{helpers}\n{channels}");
    let mut group = c.benchmark_group("translate");
    for (name, source) in [
        ("plasma", simple),
        ("channels", channels),
        ("large", &large),
    ] {
        group.throughput(Throughput::Bytes(source.len() as u64));
        group.bench_function(name, |b| {
            b.iter(|| translator::translate(black_box(source), "Bench", "Kroma"));
        });
    }
    group.finish();
}

fn packages(c: &mut Criterion) {
    let temp = tempfile::tempdir().unwrap();
    // Deterministic, moderately compressible bytes; no entropy from the host.
    let payload: Vec<u8> = (0..8 * 1024 * 1024)
        .map(|i| ((i * 31 + i / 4096) % 251) as u8)
        .collect();
    for (codec, policy) in [
        ("none", CompressionPolicy::None),
        ("lz4", CompressionPolicy::Lz4),
        ("zstd", CompressionPolicy::Zstd { level: 3 }),
    ] {
        let path = temp.path().join(format!("{codec}.shade"));
        let mut package = LiveShadePackage::new_empty(config());
        package.add_asset("assets/video.bin".into(), payload.clone());
        package.set_entry_compression("assets/video.bin", policy);
        package.save(&path).unwrap();
        drop(package);

        c.bench_with_input(BenchmarkId::new("package_load", codec), &path, |b, path| {
            b.iter(|| LiveShadePackage::load(black_box(path)).unwrap());
        });

        let package = LiveShadePackage::load(&path).unwrap();
        let stream = package.open_asset_stream("assets/video.bin").unwrap();
        for size in [4096, 65536] {
            let mut out = vec![0u8; size];
            let mut offset = 0;
            let mut group = c.benchmark_group(format!("stream_read_{size}"));
            group.throughput(Throughput::Bytes(size as u64));
            group.bench_function(codec, |b| {
                b.iter(|| {
                    // Sequential AVIO-sized reads, including chunk boundaries.
                    let read = stream
                        .read_at(black_box(offset), black_box(&mut out))
                        .unwrap();
                    black_box(&out[..read]);
                    offset = (offset + size as u64) % payload.len() as u64;
                });
            });
            group.finish();
        }
    }
}

fn ipc(c: &mut Criterion) {
    let command = DaemonCommand::LoadShade {
        path: "/tmp/benchmark.shade".into(),
        force: false,
        transition: None,
    };
    let bytes = serde_json::to_vec(&command).unwrap();
    c.bench_function("ipc/load_decode", |b| {
        b.iter(|| serde_json::from_slice::<DaemonCommand>(black_box(&bytes)).unwrap());
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(50)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3));
    targets = translate, packages, ipc
}
criterion_main!(benches);
