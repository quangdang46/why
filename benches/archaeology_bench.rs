use criterion::{Criterion, criterion_group, criterion_main};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use why_archaeologist::analyze_target;
use why_locator::{QueryKind, QueryTarget};

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

fn bench_analyze_target_symbol(c: &mut Criterion) {
    let fixture = setup_fixture("timebomb_repo");
    let target = QueryTarget {
        path: "src/legacy.rs".into(),
        start_line: None,
        end_line: None,
        symbol: Some("process_legacy_format".into()),
        query_kind: QueryKind::Symbol,
    };

    c.bench_function("archaeology/analyze_target_symbol_timebomb_repo", |b| {
        b.iter(|| {
            analyze_target(
                std::hint::black_box(&target),
                std::hint::black_box(fixture.path()),
            )
        })
    });
}

fn bench_analyze_target_range(c: &mut Criterion) {
    let fixture = setup_fixture("timebomb_repo");
    let target = QueryTarget {
        path: "src/legacy.rs".into(),
        start_line: Some(14),
        end_line: Some(22),
        symbol: None,
        query_kind: QueryKind::Range,
    };

    c.bench_function("archaeology/analyze_target_range_timebomb_repo", |b| {
        b.iter(|| {
            analyze_target(
                std::hint::black_box(&target),
                std::hint::black_box(fixture.path()),
            )
        })
    });
}

criterion_group!(
    benches,
    bench_analyze_target_symbol,
    bench_analyze_target_range
);
criterion_main!(benches);
