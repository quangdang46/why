//! Planned unit-test matrix for crates/synthesizer.

#[allow(dead_code)]
pub const TEST_CASES: &[(&str, &str)] = &[
    ("test_parse_valid_response", "well-formed JSON response parses into WhyReport"),
    ("test_parse_strips_markdown_fences", "```json fenced responses are cleaned before parsing"),
    ("test_parse_risk_level_variants", "HIGH/high/High all normalize to RiskLevel::High"),
    ("test_heuristic_report_no_key", "missing API key produces heuristic-only report with low confidence"),
    ("test_cost_calculation", "known token counts map to expected dollar estimate"),
    ("test_prompt_contract_rules", "prompt contract codifies required fields and anti-hallucination rules"),
    ("test_anthropic_config_defaults", "Anthropic client config defaults match the planned model, token, and timeout surface"),
    ("test_anthropic_request_shape", "Anthropic requests include required headers and payload fields"),
    ("test_anthropic_usage_costs", "usage tokens are converted into approximate dollar cost"),
    ("test_retry_policy", "429 and 5xx errors retry with bounded backoff while 4xx errors do not"),
];

#[test]
fn synthesizer_test_matrix_covers_plan_inventory() {
    let expected = [
        "test_parse_valid_response",
        "test_parse_strips_markdown_fences",
        "test_parse_risk_level_variants",
        "test_heuristic_report_no_key",
        "test_cost_calculation",
        "test_prompt_contract_rules",
        "test_anthropic_config_defaults",
        "test_anthropic_request_shape",
        "test_anthropic_usage_costs",
        "test_retry_policy",
    ];

    let actual: Vec<_> = TEST_CASES.iter().map(|(name, _)| *name).collect();
    assert_eq!(actual, expected);
}
