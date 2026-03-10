//! Planned unit-test matrix for crates/context.

#[allow(dead_code)]
pub const TEST_CASES: &[(&str, &str)] = &[
    ("test_comment_extraction", "collect //, #, and /* style comments from context window"),
    ("test_marker_extraction", "extract TODO/FIXME/HACK markers with correct marker kind"),
    ("test_builtin_high_risk", "auth/security vocabulary sets high-risk flags"),
    ("test_builtin_medium_risk", "migration vocabulary sets medium-risk flags"),
    ("test_custom_keywords", "custom risk vocabulary from config influences risk level"),
    ("test_heuristic_risk_computation", "flag combinations fold into expected heuristic risk"),
];

#[test]
fn context_test_matrix_covers_plan_inventory() {
    let expected = [
        "test_comment_extraction",
        "test_marker_extraction",
        "test_builtin_high_risk",
        "test_builtin_medium_risk",
        "test_custom_keywords",
        "test_heuristic_risk_computation",
    ];

    let actual: Vec<_> = TEST_CASES.iter().map(|(name, _)| *name).collect();
    assert_eq!(actual, expected);
}
