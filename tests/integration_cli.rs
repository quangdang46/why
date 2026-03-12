mod common;

use anyhow::Result;
use common::{
    assert_json_golden, assert_terminal_golden, ensure_success, setup_compat_shim_repo,
    setup_coupling_repo, setup_hotfix_repo, setup_javascript_repo, setup_python_repo,
    setup_sparse_repo, setup_split_repo, setup_typescript_repo,
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
    assert!(
        parsed["local_context"]["comments"]
            .as_array()
            .is_some_and(|comments| comments.iter().any(|comment| comment
                .as_str()
                .is_some_and(|text| text.contains("security: validate amount range"))))
    );
    assert!(
        parsed["local_context"]["comments"]
            .as_array()
            .is_some_and(|comments| comments.iter().any(|comment| comment
                .as_str()
                .is_some_and(|text| text.contains("duplicate charge incident #4521"))))
    );
    assert!(
        parsed["local_context"]["risk_flags"]
            .as_array()
            .is_some_and(|flags| flags.iter().any(|flag| flag == "security"))
    );
    assert!(
        parsed["local_context"]["risk_flags"]
            .as_array()
            .is_some_and(|flags| flags
                .iter()
                .any(|flag| flag == "token" || flag == "incident" || flag == "hotfix"))
    );
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
fn hotfix_repo_since_filters_to_recent_commits() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&["src/payment.rs:6", "--json", "--no-llm", "--since", "1"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    let commits = parsed["commits"]
        .as_array()
        .expect("commits should be an array");
    assert_eq!(commits.len(), 1);
    assert!(
        commits[0]["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("hotfix"))
    );

    Ok(())
}

#[test]
fn hotfix_repo_team_report_shows_primary_owner_and_bus_factor() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&["src/payment.rs:process_payment", "--team", "--json"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["target"]["path"], "src/payment.rs");
    assert_eq!(parsed["target"]["query_kind"], "symbol");
    assert_eq!(parsed["bus_factor"], 1);
    assert_eq!(parsed["risk_level"], "HIGH");
    let owners = parsed["owners"].as_array().expect("owners should be array");
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0]["author"], "Fixture Bot");
    assert_eq!(owners[0]["commit_count"], 2);
    assert_eq!(owners[0]["ownership_percent"], 100);

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
    assert!(
        parsed["local_context"]["comments"]
            .as_array()
            .is_some_and(|comments| comments.is_empty())
    );
    assert!(
        parsed["local_context"]["markers"]
            .as_array()
            .is_some_and(|markers| markers.is_empty())
    );

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

#[test]
fn coupling_queries_return_ranked_json_for_fixture_repo() -> Result<()> {
    let repo = setup_coupling_repo()?;
    let output = repo.run_why(&["src/schema.rs:1", "--coupled", "--json"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["target_path"], "src/schema.rs");
    assert_eq!(parsed["target_commit_count"], 5);
    let results = parsed["results"]
        .as_array()
        .expect("coupling results should be an array");
    assert!(!results.is_empty());
    assert_eq!(results[0]["path"], "src/data.rs");
    assert_eq!(results[0]["shared_commits"], 5);
    assert_eq!(results[0]["target_commit_count"], 5);
    assert_eq!(results[0]["coupling_ratio"], 1.0);

    Ok(())
}

#[test]
fn blame_chain_queries_return_origin_and_skipped_commits_for_fixture_repo() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&[
        "src/payment.rs:PaymentService::process_payment",
        "--blame-chain",
        "--json",
    ])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["target"]["path"], "src/payment.rs");
    assert_eq!(parsed["target"]["query_kind"], "qualified_symbol");
    assert_eq!(parsed["mode"], "blame-chain");
    assert_eq!(parsed["chain_depth"], 1);
    assert_eq!(parsed["risk_level"], "HIGH");
    assert!(
        parsed["starting_commit"]["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("align payment indentation"))
    );
    let skipped = parsed["noise_commits_skipped"]
        .as_array()
        .expect("noise commits should be an array");
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0]["is_mechanical"].as_bool().unwrap_or(false));
    assert!(
        skipped[0]["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("align payment indentation"))
    );
    assert!(
        parsed["origin_commit"]["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("hotfix: fix duplicate charge vulnerability"))
    );
    assert!(
        parsed["origin_commit"]["issue_refs"]
            .as_array()
            .is_some_and(|refs| refs.iter().any(|r| r == "#4521"))
    );

    Ok(())
}

#[test]
fn blame_chain_queries_render_terminal_output_for_fixture_repo() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&[
        "src/payment.rs:PaymentService::process_payment",
        "--blame-chain",
    ])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    assert!(stdout.contains("Blame chain for src/payment.rs"));
    assert!(stdout.contains("Starting blame tip:"));
    assert!(stdout.contains("Skipped (mechanical):"));
    assert!(stdout.contains("fmt: align payment indentation"));
    assert!(stdout.contains("True origin:"));
    assert!(stdout.contains("hotfix: fix duplicate charge vulnerability"));
    assert!(stdout.contains("Risk signals:"));

    Ok(())
}

#[test]
fn split_queries_return_json_null_when_no_split_is_suggested() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&[
        "src/payment.rs:PaymentService::process_payment",
        "--split",
        "--json",
    ])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert!(parsed.is_null());
    assert_json_golden("cli_split_no_split_hotfix_repo", &parsed)?;

    Ok(())
}

#[test]
fn split_queries_render_no_split_message_when_target_is_cohesive() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&["src/payment.rs:PaymentService::process_payment", "--split"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    assert!(stdout.contains("No split suggested for PaymentService::process_payment"));
    assert!(stdout.contains("archaeologically cohesive"));
    assert_terminal_golden("cli_split_no_split_hotfix_repo", &stdout)?;

    Ok(())
}

#[test]
fn split_queries_return_positive_json_suggestion_for_mixed_era_fixture() -> Result<()> {
    let repo = setup_split_repo()?;
    let output = repo.run_why(&["src/auth.rs:authenticate", "--split", "--json"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    let blocks = parsed["blocks"]
        .as_array()
        .expect("split suggestion should include blocks");

    assert_eq!(parsed["path"], "src/auth.rs");
    assert_eq!(parsed["symbol"], "authenticate");
    assert_eq!(parsed["start_line"], 1);
    assert_eq!(parsed["end_line"], 14);
    assert_eq!(parsed["total_lines"], 14);
    assert_eq!(blocks.len(), 2);

    assert_eq!(blocks[0]["start_line"], 1);
    assert_eq!(blocks[0]["end_line"], 6);
    assert_eq!(blocks[0]["line_count"], 6);
    assert_eq!(blocks[0]["percentage_of_function"], 43);
    assert_eq!(blocks[0]["era_label"], "Security hardening era");
    assert_eq!(blocks[0]["suggested_name"], "authenticate_with_guard");
    assert_eq!(blocks[0]["risk_level"], "HIGH");
    assert!(
        blocks[0]["dominant_commit_summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("hotfix: harden authenticate"))
    );

    assert_eq!(blocks[1]["start_line"], 7);
    assert_eq!(blocks[1]["end_line"], 14);
    assert_eq!(blocks[1]["line_count"], 8);
    assert_eq!(blocks[1]["percentage_of_function"], 57);
    assert_eq!(blocks[1]["era_label"], "Backward compat era");
    assert_eq!(blocks[1]["suggested_name"], "authenticate_legacy");
    assert_eq!(blocks[1]["risk_level"], "MEDIUM");
    assert!(
        blocks[1]["dominant_commit_summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("legacy v1 token support"))
    );
    assert_json_golden("cli_split_positive_split_repo", &parsed)?;

    Ok(())
}

#[test]
fn split_queries_render_positive_terminal_suggestion_for_mixed_era_fixture() -> Result<()> {
    let repo = setup_split_repo()?;
    let output = repo.run_why(&["src/auth.rs:authenticate", "--split"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    assert!(stdout.contains("Suggested split for authenticate() (14 lines, lines 1-14)"));
    assert!(stdout.contains("Block A  lines 1-6  Security hardening era"));
    assert!(stdout.contains("Suggested extraction: authenticate_with_guard()"));
    assert!(stdout.contains("Risk: HIGH"));
    assert!(stdout.contains("Block B  lines 7-14  Backward compat era"));
    assert!(stdout.contains("Suggested extraction: authenticate_legacy()"));
    assert!(stdout.contains("Risk: MEDIUM"));
    assert!(stdout.contains("different reasons to change"));
    assert!(stdout.contains("historically distinct paths"));
    assert_terminal_golden("cli_split_positive_split_repo", &stdout)?;

    Ok(())
}
