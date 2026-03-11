mod common;

use anyhow::Result;
use common::{
    ensure_success, setup_compat_shim_repo, setup_hotfix_repo, setup_javascript_repo,
    setup_python_repo, setup_sparse_repo, setup_typescript_repo,
};
use serde_json::Value;

#[test]
fn hotfix_repo_json_output_has_phase_one_shape() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&["src/payment.rs:6", "--json", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;

    assert_eq!(parsed["target"]["path"], "src/payment.rs");
    assert_eq!(parsed["target"]["start_line"], 6);
    assert_eq!(parsed["target"]["end_line"], 6);
    assert_eq!(parsed["target"]["query_kind"], "line");
    assert_eq!(parsed["mode"], "heuristic");
    assert!(parsed["commits"].is_array());
    assert!(
        parsed["commits"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert_eq!(parsed["risk_level"], "HIGH");
    let commits = parsed["commits"]
        .as_array()
        .expect("commits should be an array");
    assert!(
        commits
            .iter()
            .all(|commit| commit["email"] == "test@example.com")
    );
    let hotfix_commit = commits
        .iter()
        .find(|commit| {
            commit["summary"]
                .as_str()
                .is_some_and(|summary| summary.contains("hotfix"))
        })
        .expect("expected hotfix commit metadata");
    assert!(
        hotfix_commit["message"]
            .as_str()
            .is_some_and(|msg| msg.contains("#4521"))
    );
    assert!(
        hotfix_commit["diff_excerpt"]
            .as_str()
            .is_some_and(|diff| diff.contains("diff --git a/src/payment.rs b/src/payment.rs"))
    );
    assert!(
        hotfix_commit["issue_refs"]
            .as_array()
            .is_some_and(|refs| refs.iter().any(|r| r == "#4521"))
    );
    assert!(
        hotfix_commit["coverage_score"]
            .as_f64()
            .is_some_and(|score| score > 0.9)
    );
    assert!(
        hotfix_commit["relevance_score"]
            .as_f64()
            .is_some_and(|score| score > 0.0)
    );
    assert_eq!(hotfix_commit["is_mechanical"], false);

    Ok(())
}

#[test]
fn hotfix_repo_terminal_output_lists_commits_and_risk() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&["src/payment.rs:6", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    assert!(stdout.contains("why: src/payment.rs (line 6)"));
    assert!(stdout.contains("Commits touching this line:"));
    assert!(stdout.contains("Heuristic risk: HIGH."));

    Ok(())
}

#[test]
fn range_query_works_for_compat_fixture() -> Result<()> {
    let repo = setup_compat_shim_repo()?;
    let output = repo.run_why(&["src/http.rs", "--lines", "1:6", "--json", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["target"]["query_kind"], "range");
    assert_eq!(parsed["target"]["start_line"], 1);
    assert_eq!(parsed["target"]["end_line"], 6);
    assert!(
        parsed["commits"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    let commits = parsed["commits"]
        .as_array()
        .expect("commits should be an array");
    let compat_commit = commits
        .iter()
        .find(|commit| {
            commit["summary"]
                .as_str()
                .is_some_and(|summary| summary.contains("legacy mobile clients"))
        })
        .expect("expected compat commit metadata");
    assert!(
        compat_commit["issue_refs"]
            .as_array()
            .is_some_and(|refs| refs.iter().any(|r| r == "#318"))
    );
    assert_eq!(compat_commit["is_mechanical"], false);

    Ok(())
}

#[test]
fn sparse_repo_yields_non_high_risk() -> Result<()> {
    let repo = setup_sparse_repo()?;
    let output = repo.run_why(&["src/util.rs:1", "--json", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_ne!(parsed["risk_level"], "HIGH");

    Ok(())
}

#[test]
fn rust_symbol_queries_resolve_and_render_commit_output() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&["src/payment.rs:process_payment", "--json", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["target"]["path"], "src/payment.rs");
    assert_eq!(parsed["target"]["query_kind"], "symbol");
    assert_eq!(parsed["target"]["start_line"], 4);
    assert_eq!(parsed["target"]["end_line"], 12);
    assert_eq!(parsed["risk_level"], "HIGH");
    assert!(
        parsed["commits"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    Ok(())
}

#[test]
fn rust_qualified_symbol_queries_resolve_impl_methods() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&[
        "src/payment.rs:PaymentService::process_payment",
        "--json",
        "--no-llm",
    ])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["target"]["path"], "src/payment.rs");
    assert_eq!(parsed["target"]["query_kind"], "qualified_symbol");
    assert_eq!(parsed["target"]["start_line"], 4);
    assert_eq!(parsed["target"]["end_line"], 12);
    assert_eq!(parsed["risk_level"], "HIGH");
    assert!(
        parsed["commits"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    Ok(())
}

#[test]
fn typescript_symbol_queries_resolve_and_render_commit_output() -> Result<()> {
    let repo = setup_typescript_repo()?;
    let output = repo.run_why(&["src/auth.ts:authenticate", "--json", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["target"]["path"], "src/auth.ts");
    assert_eq!(parsed["target"]["query_kind"], "symbol");
    assert_eq!(parsed["target"]["start_line"], 11);
    assert_eq!(parsed["target"]["end_line"], 17);
    assert_eq!(parsed["risk_level"], "HIGH");
    assert!(
        parsed["commits"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    Ok(())
}

#[test]
fn typescript_class_symbol_queries_resolve_and_render_commit_output() -> Result<()> {
    let repo = setup_typescript_repo()?;
    let output = repo.run_why(&["src/auth.ts:AuthService", "--json", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["target"]["path"], "src/auth.ts");
    assert_eq!(parsed["target"]["query_kind"], "symbol");
    assert_eq!(parsed["target"]["start_line"], 1);
    assert_eq!(parsed["target"]["end_line"], 9);
    assert_eq!(parsed["risk_level"], "HIGH");
    assert!(
        parsed["commits"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    Ok(())
}

#[test]
fn javascript_symbol_queries_resolve_and_render_commit_output() -> Result<()> {
    let repo = setup_javascript_repo()?;
    let output = repo.run_why(&["src/auth.js:login", "--json", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["target"]["path"], "src/auth.js");
    assert_eq!(parsed["target"]["query_kind"], "symbol");
    assert_eq!(parsed["target"]["start_line"], 11);
    assert_eq!(parsed["target"]["end_line"], 17);
    assert_eq!(parsed["risk_level"], "HIGH");
    assert!(
        parsed["commits"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    Ok(())
}

#[test]
fn javascript_class_symbol_queries_resolve_and_render_commit_output() -> Result<()> {
    let repo = setup_javascript_repo()?;
    let output = repo.run_why(&["src/auth.js:AuthService", "--json", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["target"]["path"], "src/auth.js");
    assert_eq!(parsed["target"]["query_kind"], "symbol");
    assert_eq!(parsed["target"]["start_line"], 1);
    assert_eq!(parsed["target"]["end_line"], 9);
    assert_eq!(parsed["risk_level"], "HIGH");
    assert!(
        parsed["commits"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    Ok(())
}

#[test]
fn python_symbol_queries_resolve_and_render_commit_output() -> Result<()> {
    let repo = setup_python_repo()?;
    let output = repo.run_why(&["src/auth.py:authenticate", "--json", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["target"]["path"], "src/auth.py");
    assert_eq!(parsed["target"]["query_kind"], "symbol");
    assert_eq!(parsed["target"]["start_line"], 8);
    assert_eq!(parsed["target"]["end_line"], 13);
    assert_eq!(parsed["risk_level"], "HIGH");
    assert!(
        parsed["commits"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    Ok(())
}
