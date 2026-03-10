//! Planned unit-test matrix for crates/archaeologist.

#[allow(dead_code)]
pub const TEST_CASES: &[(&str, &str)] = &[
    ("test_issue_ref_extraction", "extract #123, fixes #456, closes #789 style references"),
    ("test_mechanical_commit_detection", "whitespace-only diff is classified mechanical"),
    ("test_bulk_refactor_detection", "large multi-file refactor receives mechanical classification"),
    ("test_scoring_high_signal", "hotfix/security wording drives high relevance score"),
    ("test_scoring_mechanical_penalty", "mechanical commits are heavily penalized in scoring"),
    ("test_scoring_recency_bonus", "recent commits receive bounded recency bonus"),
    ("test_top_n_selection", "highest ranked N commits are retained when commit list exceeds budget"),
];

#[test]
fn archaeologist_test_matrix_covers_plan_inventory() {
    let expected = [
        "test_issue_ref_extraction",
        "test_mechanical_commit_detection",
        "test_bulk_refactor_detection",
        "test_scoring_high_signal",
        "test_scoring_mechanical_penalty",
        "test_scoring_recency_bonus",
        "test_top_n_selection",
    ];

    let actual: Vec<_> = TEST_CASES.iter().map(|(name, _)| *name).collect();
    assert_eq!(actual, expected);
}
