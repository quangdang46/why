use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use tempfile::tempdir;
use why_cache::Cache;

fn bench_cache_set(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_set");

    for entry_count in [1_usize, 10, 100] {
        group.bench_with_input(
            BenchmarkId::from_parameter(entry_count),
            &entry_count,
            |b, &entry_count| {
                b.iter(|| {
                    let Ok(dir) = tempdir() else {
                        return;
                    };
                    let Ok(mut cache) = Cache::open(dir.path(), entry_count + 1) else {
                        return;
                    };
                    for index in 0..entry_count {
                        let head_hash = format!("{index:016x}");
                        let key =
                            Cache::make_key("src/auth.rs", &format!("symbol_{index}"), &head_hash);
                        if cache
                            .set(key, format!("report-{index}"), black_box(&head_hash))
                            .is_err()
                        {
                            return;
                        }
                    }
                })
            },
        );
    }

    group.finish();
}

fn bench_cache_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_get");

    for entry_count in [1_usize, 10, 100] {
        let Ok(dir) = tempdir() else {
            continue;
        };
        let Ok(mut cache) = Cache::open(dir.path(), entry_count + 1) else {
            continue;
        };
        let target_head_hash = format!("{:016x}", entry_count / 2);
        let target_key = Cache::make_key(
            "src/auth.rs",
            &format!("symbol_{}", entry_count / 2),
            &target_head_hash,
        );

        for index in 0..entry_count {
            let head_hash = format!("{index:016x}");
            let key = Cache::make_key("src/auth.rs", &format!("symbol_{index}"), &head_hash);
            if cache
                .set(key, format!("report-{index}"), &head_hash)
                .is_err()
            {
                continue;
            }
        }

        group.bench_with_input(
            BenchmarkId::from_parameter(entry_count),
            &entry_count,
            |b, _| {
                b.iter(|| {
                    if let Some(value) = cache.get::<String>(black_box(&target_key)) {
                        black_box(value);
                    }
                })
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_cache_set, bench_cache_get);
criterion_main!(benches);
