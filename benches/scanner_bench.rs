use criterion::{Criterion, criterion_group, criterion_main};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use why_scanner::{scan_ghosts, scan_health, scan_hotspots};

const BENCH_HISTORY_COMMITS: &str = "250";
const BENCH_EXTRA_FILES: &str = "40";

fn setup_fixture(name: &str) -> TempDir {
    let dir = TempDir::new().expect("failed to create tempdir for benchmark fixture");
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
        .join("setup.sh");
    let output = Command::new("bash")
        .arg(&script)
        .arg(dir.path())
        .env("WHY_BENCH_HISTORY_COMMITS", BENCH_HISTORY_COMMITS)
        .env("WHY_BENCH_EXTRA_FILES", BENCH_EXTRA_FILES)
        .output()
        .expect("failed to run benchmark fixture script");
    assert!(
        output.status.success(),
        "fixture setup failed for {name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    dir
}

fn bench_scan_hotspots(c: &mut Criterion) {
    let fixture = setup_fixture("hotfix_repo");

    c.bench_function("scanner/hotspots_hotfix_repo", |b| {
        b.iter(|| {
            scan_hotspots(
                std::hint::black_box(fixture.path()),
                std::hint::black_box(10),
                std::hint::black_box(None),
            )
        })
    });
}

fn bench_scan_health(c: &mut Criterion) {
    let fixture = setup_fixture("timebomb_repo");

    c.bench_function("scanner/health_timebomb_repo", |b| {
        b.iter(|| scan_health(std::hint::black_box(fixture.path())))
    });
}

fn bench_scan_ghosts(c: &mut Criterion) {
    let fixture = setup_fixture("ghost_repo");

    c.bench_function("scanner/ghosts_ghost_repo", |b| {
        b.iter(|| {
            scan_ghosts(
                std::hint::black_box(fixture.path()),
                std::hint::black_box(10),
            )
        })
    });
}

criterion_group!(
    benches,
    bench_scan_hotspots,
    bench_scan_health,
    bench_scan_ghosts
);
criterion_main!(benches);
