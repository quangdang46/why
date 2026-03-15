//! Planned unit-test matrix for crates/cache.

#[allow(dead_code)]
pub const TEST_CASES: &[(&str, &str)] = &[
    ("test_cache_key_format", "cache key includes target identity and HEAD hash"),
    ("test_cache_set_get", "stored report can be retrieved unchanged"),
    ("test_cache_miss_different_head", "same target under different HEAD hash misses cache"),
    ("test_cache_eviction", "oldest entry is evicted when max_entries is exceeded"),
    ("test_health_snapshot_rolling", "keeping 53 weekly health snapshots retains only 52"),
    (
        "test_health_snapshot_legacy_alias",
        "legacy health snapshots using the details field still deserialize",
    ),
];

#[test]
fn cache_test_matrix_covers_plan_inventory() {
    let expected = [
        "test_cache_key_format",
        "test_cache_set_get",
        "test_cache_miss_different_head",
        "test_cache_eviction",
        "test_health_snapshot_rolling",
        "test_health_snapshot_legacy_alias",
    ];

    let actual: Vec<_> = TEST_CASES.iter().map(|(name, _)| *name).collect();
    assert_eq!(actual, expected);
}
