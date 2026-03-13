mod common;

use anyhow::Result;
use common::{
    assert_json_golden, assert_terminal_golden, ensure_success, setup_compat_shim_repo,
    setup_coupling_repo, setup_ghost_repo, setup_hotfix_repo, setup_javascript_repo,
    setup_python_repo, setup_sparse_repo, setup_split_repo, setup_timebomb_repo,
    setup_typescript_repo,
};
use serde_json::Value;

#[test]
fn hotfix_repo_json_output_has_phase_one_shape() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&["src/payment.rs:6", "--json", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;

    assert_eq!(parsed["mode"], "heuristic");
    assert_eq!(parsed["risk_level"], "HIGH");
    assert_eq!(parsed["confidence"], "low");
    assert!(
        parsed["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("Heuristic analysis of src/payment.rs:6"))
    );
    assert!(
        parsed["evidence"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert!(
        parsed["inference"]
            .as_array()
            .is_some_and(|items| items.is_empty())
    );
    assert!(
        parsed["unknowns"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert!(parsed["risk_summary"].as_str().is_some_and(
        |text| text.contains("security sensitivity")
            || text.contains("migration")
            || text.contains("available history")
    ));
    assert!(
        parsed["change_guidance"]
            .as_str()
            .is_some_and(|text| !text.is_empty())
    );
    assert!(
        parsed["notes"]
            .as_array()
            .is_some_and(|items| items.iter().any(|note| note
                .as_str()
                .is_some_and(|text| text.contains("No LLM synthesis"))))
    );
    assert!(parsed["cost_usd"].is_null());

    Ok(())
}

#[test]
fn hotfix_repo_since_filters_to_recent_evidence() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&["src/payment.rs:6", "--json", "--no-llm", "--since", "1"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert!(
        parsed["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("Heuristic analysis"))
    );
    let evidence = parsed["evidence"]
        .as_array()
        .expect("evidence should be an array");
    assert_eq!(evidence.len(), 1);
    assert!(
        evidence[0]
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
fn hotfix_repo_terminal_output_lists_summary_evidence_and_risk() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&["src/payment.rs:6", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    assert!(stdout.contains("why: src/payment.rs:6"));
    assert!(stdout.contains("Summary"));
    assert!(stdout.contains("Risk: HIGH (low)"));
    assert!(stdout.contains("Evidence"));
    assert!(stdout.contains("Unknowns"));
    assert!(!stdout.contains("[cached]"));
    assert_terminal_golden("cli_why_hotfix_repo", &stdout)?;

    Ok(())
}

#[test]
fn hotfix_repo_json_output_matches_golden_snapshot() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&["src/payment.rs:6", "--json", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_json_golden("cli_why_hotfix_repo", &parsed)?;

    Ok(())
}

#[test]
fn repeated_query_uses_cache_and_writes_cache_file() -> Result<()> {
    let repo = setup_hotfix_repo()?;

    let first = repo.run_why(&["src/payment.rs:6", "--no-llm"])?;
    ensure_success(&first)?;
    assert!(!repo.stdout(&first).contains("[cached]"));

    let cache_path = repo.path.join(".why").join("cache.json");
    assert!(cache_path.is_file());

    let second = repo.run_why(&["src/payment.rs:6", "--no-llm"])?;
    ensure_success(&second)?;
    assert!(repo.stdout(&second).contains("[cached]"));

    Ok(())
}

#[test]
fn no_cache_flag_bypasses_cached_hit_indicator() -> Result<()> {
    let repo = setup_hotfix_repo()?;

    let first = repo.run_why(&["src/payment.rs:6", "--no-llm"])?;
    ensure_success(&first)?;

    let second = repo.run_why(&["src/payment.rs:6", "--no-llm", "--no-cache"])?;
    ensure_success(&second)?;
    assert!(!repo.stdout(&second).contains("[cached]"));

    Ok(())
}

#[test]
fn range_query_works_for_compat_fixture() -> Result<()> {
    let repo = setup_compat_shim_repo()?;
    let output = repo.run_why(&["src/http.rs", "--lines", "1:6", "--json", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["mode"], "heuristic");
    assert_eq!(parsed["risk_level"], "MEDIUM");
    assert!(
        parsed["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("src/http.rs:1-6"))
    );
    let evidence = parsed["evidence"]
        .as_array()
        .expect("evidence should be an array");
    assert!(!evidence.is_empty());
    assert!(evidence.iter().any(|item| {
        item.as_str()
            .is_some_and(|summary| summary.contains("legacy mobile clients"))
    }));

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
        parsed["evidence"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert!(
        parsed["unknowns"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item
                .as_str()
                .is_some_and(|text| text.contains("No model synthesis"))))
    );

    Ok(())
}

#[test]
fn rust_symbol_queries_emit_why_report_json() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&["src/payment.rs:process_payment", "--json", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["mode"], "heuristic");
    assert_eq!(parsed["risk_level"], "HIGH");
    assert_eq!(parsed["confidence"], "low");
    assert!(
        parsed["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("src/payment.rs:process_payment"))
    );
    assert!(
        parsed["evidence"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    Ok(())
}

#[test]
fn rust_qualified_symbol_queries_emit_why_report_json() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&[
        "src/payment.rs:PaymentService::process_payment",
        "--json",
        "--no-llm",
    ])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["mode"], "heuristic");
    assert_eq!(parsed["risk_level"], "HIGH");
    assert!(parsed["summary"].as_str().is_some_and(|summary| {
        summary.contains("src/payment.rs:PaymentService::process_payment")
    }));
    assert!(
        parsed["evidence"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    Ok(())
}

#[test]
fn typescript_symbol_queries_emit_why_report_json() -> Result<()> {
    let repo = setup_typescript_repo()?;
    let output = repo.run_why(&["src/auth.ts:authenticate", "--json", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["mode"], "heuristic");
    assert_eq!(parsed["risk_level"], "HIGH");
    assert!(
        parsed["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("src/auth.ts:authenticate"))
    );
    assert!(
        parsed["evidence"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    Ok(())
}

#[test]
fn typescript_class_symbol_queries_emit_why_report_json() -> Result<()> {
    let repo = setup_typescript_repo()?;
    let output = repo.run_why(&["src/auth.ts:AuthService", "--json", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["mode"], "heuristic");
    assert_eq!(parsed["risk_level"], "HIGH");
    assert!(
        parsed["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("src/auth.ts:AuthService"))
    );
    assert!(
        parsed["evidence"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    Ok(())
}

#[test]
fn javascript_symbol_queries_emit_why_report_json() -> Result<()> {
    let repo = setup_javascript_repo()?;
    let output = repo.run_why(&["src/auth.js:login", "--json", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["mode"], "heuristic");
    assert_eq!(parsed["risk_level"], "HIGH");
    assert!(
        parsed["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("src/auth.js:login"))
    );
    assert!(
        parsed["evidence"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    Ok(())
}

#[test]
fn javascript_class_symbol_queries_emit_why_report_json() -> Result<()> {
    let repo = setup_javascript_repo()?;
    let output = repo.run_why(&["src/auth.js:AuthService", "--json", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["mode"], "heuristic");
    assert_eq!(parsed["risk_level"], "HIGH");
    assert!(
        parsed["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("src/auth.js:AuthService"))
    );
    assert!(
        parsed["evidence"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    Ok(())
}

#[test]
fn python_symbol_queries_emit_why_report_json() -> Result<()> {
    let repo = setup_python_repo()?;
    let output = repo.run_why(&["src/auth.py:authenticate", "--json", "--no-llm"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert_eq!(parsed["mode"], "heuristic");
    assert_eq!(parsed["risk_level"], "HIGH");
    assert!(
        parsed["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("src/auth.py:authenticate"))
    );
    assert!(
        parsed["evidence"]
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
fn hotspots_subcommand_returns_ranked_json_for_fixture_repo() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&["hotspots", "--limit", "5", "--json"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    let findings = parsed
        .as_array()
        .expect("hotspots output should be an array");
    assert!(!findings.is_empty());
    assert_eq!(findings[0]["path"], "src/payment.rs");
    assert!(findings[0]["churn_commits"].as_u64().unwrap_or_default() >= 2);
    assert_eq!(findings[0]["risk_level"], "HIGH");
    assert!(findings[0]["hotspot_score"].as_f64().unwrap_or_default() >= 6.0);
    assert_json_golden("cli_hotspots_hotfix_repo", &parsed)?;

    Ok(())
}

#[test]
fn hotspots_subcommand_renders_terminal_summary() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why(&["hotspots", "--limit", "3"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    assert_terminal_golden("cli_hotspots_hotfix_repo", &stdout)?;

    Ok(())
}

#[test]
fn health_subcommand_returns_json_report_and_persists_snapshot() -> Result<()> {
    let repo = setup_timebomb_repo()?;
    let output = repo.run_why(&["health", "--json"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    assert!(parsed["debt_score"].as_u64().unwrap_or_default() > 0);
    assert_eq!(parsed["signals"]["time_bombs"], 1);
    assert_eq!(parsed["signals"]["stale_hacks"], 0);
    assert!(parsed["delta"].is_null());
    assert!(
        parsed["notes"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    let cache_path = repo.path.join(".why").join("cache.json");
    let cache_value: Value = serde_json::from_str(&std::fs::read_to_string(cache_path)?)?;
    assert_eq!(
        cache_value["health_snapshots"].as_array().map(|v| v.len()),
        Some(1)
    );

    Ok(())
}

#[test]
fn repeated_health_subcommand_reports_trend_from_previous_snapshot() -> Result<()> {
    let repo = setup_timebomb_repo()?;

    let first = repo.run_why(&["health", "--json"])?;
    ensure_success(&first)?;

    let second = repo.run_why(&["health", "--json"])?;
    ensure_success(&second)?;
    let parsed: Value = serde_json::from_str(&repo.stdout(&second))?;
    assert_eq!(parsed["delta"]["direction"], "→");
    assert_eq!(parsed["delta"]["amount"], 0);
    assert!(
        parsed["delta"]["previous_score"]
            .as_u64()
            .unwrap_or_default()
            > 0
    );

    Ok(())
}

#[test]
fn health_subcommand_renders_terminal_summary() -> Result<()> {
    let repo = setup_timebomb_repo()?;
    let output = repo.run_why(&["health"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    assert!(stdout.contains("Repository health"));
    assert!(stdout.contains("Debt score:"));
    assert!(stdout.contains("Signals"));
    assert!(stdout.contains("time_bombs: 1"));
    assert!(stdout.contains("stale_hacks: 0"));

    Ok(())
}

#[test]
fn ghost_subcommand_returns_ranked_json_for_fixture_repo() -> Result<()> {
    let repo = setup_ghost_repo()?;
    let output = repo.run_why(&["ghost", "--limit", "5", "--json"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    let parsed: Value = serde_json::from_str(&stdout)?;
    let findings = parsed
        .as_array()
        .expect("ghost output should be an array");
    assert!(!findings.is_empty());
    assert_eq!(findings[0]["path"], "src/auth.rs");
    assert_eq!(findings[0]["symbol"], "validate_auth_token_legacy");
    assert_eq!(findings[0]["risk_level"], "HIGH");
    assert_eq!(findings[0]["call_site_count"], 1);
    assert!(findings[0]["notes"].as_array().is_some_and(|items| items
        .iter()
        .any(|note| note.as_str().is_some_and(|text| text.contains("static analysis")))));

    Ok(())
}

#[test]
fn ghost_subcommand_renders_terminal_summary() -> Result<()> {
    let repo = setup_ghost_repo()?;
    let output = repo.run_why(&["ghost", "--limit", "5"])?;
    ensure_success(&output)?;

    let stdout = repo.stdout(&output);
    assert!(stdout.contains("Top 5 ghost functions by risk-aware archaeology"));
    assert!(stdout.contains("validate_auth_token_legacy"));
    assert!(stdout.contains("risk: HIGH"));
    assert!(stdout.contains("call-sites  1") || stdout.contains("call-sites 1"));
    assert!(stdout.contains("WARNING: ghost detection uses static analysis"));

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
