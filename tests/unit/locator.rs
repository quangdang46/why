//! Planned unit-test matrix for crates/locator.
//!
//! These tests codify the expected behaviors from PLAN.md so the future
//! locator crate can adopt them with minimal reinterpretation.

#[allow(dead_code)]
pub const TEST_CASES: &[(&str, &str)] = &[
    ("test_parse_file_colon_symbol", "\"src/lib.rs:authenticate\" -> file=src/lib.rs, spec=authenticate"),
    ("test_parse_file_colon_line", "\"src/lib.rs:42\" -> file=src/lib.rs, line=41"),
    ("test_parse_lines_override", "--lines 80:120 -> start=79, end=119"),
    ("test_parse_qualified", "\"src/lib.rs:AuthService::login\" -> qualified symbol preserved"),
    ("test_rust_symbol_resolution", "tree-sitter resolves Rust symbol to correct line range"),
    ("test_typescript_symbol_resolution", "tree-sitter resolves TypeScript symbol to correct line range"),
    ("test_ambiguous_resolution_warns", "duplicate symbol names warn and choose deterministic result"),
    ("test_language_detection", "extensions .rs .ts .py map to supported languages"),
];

#[test]
fn locator_test_matrix_covers_plan_inventory() {
    let expected = [
        "test_parse_file_colon_symbol",
        "test_parse_file_colon_line",
        "test_parse_lines_override",
        "test_parse_qualified",
        "test_rust_symbol_resolution",
        "test_typescript_symbol_resolution",
        "test_ambiguous_resolution_warns",
        "test_language_detection",
    ];

    let actual: Vec<_> = TEST_CASES.iter().map(|(name, _)| *name).collect();
    assert_eq!(actual, expected);
}
