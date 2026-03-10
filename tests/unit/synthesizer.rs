//! Planned unit-test matrix for crates/synthesizer.

#[allow(dead_code)]
pub const TEST_CASES: &[(&str, &str)] = &[
    ("test_parse_valid_response", "well-formed JSON response parses into WhyReport"),
    ("test_parse_strips_markdown_fences", "```json fenced responses are cleaned before parsing"),
    ("test_parse_risk_level_variants", "HIGH/high/High all normalize to RiskLevel::High"),
    ("test_heuristic_report_no_key", "missing API key produces heuristic-only report with low confidence"),
    ("test_cost_calculation", "known token counts map to expected dollar estimate"),
];

#[test]
fn synthesizer_test_matrix_covers_plan_inventory() {
    let expected = [
        "test_parse_valid_response",
        "test_parse_strips_markdown_fences",
        "test_parse_risk_level_variants",
        "test_heuristic_report_no_key",
        "test_cost_calculation",
    ];

    let actual: Vec<_> = TEST_CASES.iter().map(|(name, _)| *name).collect();
    assert_eq!(actual, expected);
}
