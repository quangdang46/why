//! Planned unit-test matrix for crates/evidence.

#[allow(dead_code)]
pub const TEST_CASES: &[(&str, &str)] = &[
    ("test_payload_within_budget", "large evidence payload truncates to configured character/token budget"),
    ("test_diff_excerpt_truncated", "long diff excerpt is truncated to compact display size"),
    ("test_issue_refs_deduplicated", "duplicate issue refs across commits collapse into unique signal set"),
    ("test_total_commit_count_preserved_when_payload_is_reduced", "reported total commit count stays accurate even when the payload drops commit detail to fit budget"),
    ("test_signal_lists_are_bounded", "pack-level and per-commit signal lists stay capped for bounded synthesis and fallback payloads"),
];

#[test]
fn evidence_test_matrix_covers_plan_inventory() {
    let expected = [
        "test_payload_within_budget",
        "test_diff_excerpt_truncated",
        "test_issue_refs_deduplicated",
        "test_total_commit_count_preserved_when_payload_is_reduced",
        "test_signal_lists_are_bounded",
    ];

    let actual: Vec<_> = TEST_CASES.iter().map(|(name, _)| *name).collect();
    assert_eq!(actual, expected);
}
