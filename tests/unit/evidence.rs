//! Planned unit-test matrix for crates/evidence.

#[allow(dead_code)]
pub const TEST_CASES: &[(&str, &str)] = &[
    ("test_payload_within_budget", "large evidence payload truncates to configured character/token budget"),
    ("test_diff_excerpt_truncated", "long diff excerpt is truncated to compact display size"),
    ("test_issue_refs_deduplicated", "duplicate issue refs across commits collapse into unique signal set"),
];

#[test]
fn evidence_test_matrix_covers_plan_inventory() {
    let expected = [
        "test_payload_within_budget",
        "test_diff_excerpt_truncated",
        "test_issue_refs_deduplicated",
    ];

    let actual: Vec<_> = TEST_CASES.iter().map(|(name, _)| *name).collect();
    assert_eq!(actual, expected);
}
