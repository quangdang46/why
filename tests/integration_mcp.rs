mod common;

use anyhow::Result;
use common::{
    assert_json_golden, ensure_success, setup_coupling_repo, setup_coupling_rich_repo,
    setup_hotfix_repo, setup_split_repo, setup_timebomb_repo, setup_timebomb_rich_repo,
};
use serde_json::Value;

fn response_lines(output: &std::process::Output) -> Result<Vec<Value>> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(Into::into))
        .collect()
}

fn archaeology_golden_view(payload: &Value) -> Value {
    serde_json::json!({
        "target": payload["target"],
        "risk_level": payload["risk_level"],
        "risk_summary": payload["risk_summary"],
        "change_guidance": payload["change_guidance"],
        "mode": payload["mode"],
        "notes": payload["notes"],
        "local_context": payload["local_context"],
        "commits": payload["commits"]
            .as_array()
            .map(|commits| {
                commits
                    .iter()
                    .map(|commit| serde_json::json!({
                        "author": commit["author"],
                        "email": commit["email"],
                        "summary": commit["summary"],
                        "message": commit["message"],
                        "issue_refs": commit["issue_refs"],
                        "is_mechanical": commit["is_mechanical"]
                    }))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    })
}

fn hotspots_golden_view(payload: &[Value]) -> Value {
    Value::Array(
        payload
            .iter()
            .map(|finding| {
                serde_json::json!({
                    "path": finding["path"],
                    "churn_commits": finding["churn_commits"],
                    "risk_level": finding["risk_level"],
                    "hotspot_score": finding["hotspot_score"],
                    "top_commit_summaries": finding["top_commit_summaries"]
                })
            })
            .collect(),
    )
}

fn time_bombs_golden_view(payload: &[Value]) -> Value {
    Value::Array(
        payload
            .iter()
            .map(|finding| {
                serde_json::json!({
                    "path": finding["path"],
                    "line": finding["line"],
                    "kind": finding["kind"],
                    "marker": finding["marker"],
                    "severity": finding["severity"]
                })
            })
            .collect(),
    )
}

fn coupling_golden_view(payload: &Value) -> Value {
    serde_json::json!({
        "target_path": payload["target_path"],
        "target_commit_count": payload["target_commit_count"],
        "scan_commits": payload["scan_commits"],
        "results": payload["results"]
            .as_array()
            .map(|results| {
                results
                    .iter()
                    .map(|finding| serde_json::json!({
                        "path": finding["path"],
                        "shared_commits": finding["shared_commits"],
                        "target_commit_count": finding["target_commit_count"],
                        "coupling_ratio": finding["coupling_ratio"],
                        "top_commit_summaries": finding["top_commit_summaries"]
                    }))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    })
}

#[test]
fn mcp_initialize_and_tools_list_work_over_stdio() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why_with_stdin(
        &["mcp"],
        concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n",
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n"
        ),
    )?;
    ensure_success(&output)?;

    let responses = response_lines(&output)?;
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(responses[0]["result"]["protocolVersion"], "2.0");
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("tools/list should return tool array"))?;
    assert_eq!(tools.len(), 5);
    assert_eq!(tools[0]["name"], "why_symbol");
    assert_eq!(tools[1]["name"], "why_split");
    assert_eq!(tools[2]["name"], "why_time_bombs");
    assert_eq!(tools[3]["name"], "why_hotspots");
    assert_eq!(tools[4]["name"], "why_coupling");

    Ok(())
}

#[test]
fn mcp_why_symbol_returns_archaeology_result() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why_with_stdin(
        &["mcp"],
        concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{",
            "\"name\":\"why_symbol\",",
            "\"arguments\":{\"target\":\"src/payment.rs:process_payment\",\"no_llm\":true}",
            "}}\n"
        ),
    )?;
    ensure_success(&output)?;

    let responses = response_lines(&output)?;
    let payload = &responses[0]["result"]["content"][0]["json"];
    assert_eq!(payload["target"]["path"], "src/payment.rs");
    assert_eq!(payload["target"]["query_kind"], "symbol");
    assert_eq!(payload["risk_level"], "HIGH");
    assert!(
        payload["commits"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert_json_golden(
        "mcp_why_symbol_hotfix_repo",
        &archaeology_golden_view(payload),
    )?;

    Ok(())
}

#[test]
fn mcp_why_split_returns_split_suggestion_json() -> Result<()> {
    let repo = setup_split_repo()?;
    let output = repo.run_why_with_stdin(
        &["mcp"],
        concat!(
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{",
            "\"name\":\"why_split\",",
            "\"arguments\":{\"target\":\"src/auth.rs:authenticate\"}",
            "}}\n"
        ),
    )?;
    ensure_success(&output)?;

    let responses = response_lines(&output)?;
    let payload = &responses[0]["result"]["content"][0]["json"];
    assert_eq!(payload["path"], "src/auth.rs");
    assert_eq!(payload["symbol"], "authenticate");
    assert_eq!(
        payload["blocks"].as_array().map(|blocks| blocks.len()),
        Some(2)
    );
    assert_json_golden("mcp_why_split_split_repo", payload)?;

    Ok(())
}

#[test]
fn mcp_why_time_bombs_returns_findings() -> Result<()> {
    let repo = setup_timebomb_repo()?;
    let output = repo.run_why_with_stdin(
        &["mcp"],
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"why_time_bombs\",\"arguments\":{\"max_age_days\":30}}}\n",
    )?;
    ensure_success(&output)?;

    let responses = response_lines(&output)?;
    let payload = responses[0]["result"]["content"][0]["json"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("time bomb result should be array"))?;
    assert!(!payload.is_empty());
    assert!(
        payload
            .iter()
            .any(|finding| finding["kind"] == "PastDueTodo")
    );
    assert_json_golden(
        "mcp_why_time_bombs_timebomb_repo",
        &time_bombs_golden_view(payload),
    )?;

    Ok(())
}

#[test]
fn mcp_why_time_bombs_returns_multiple_marker_kinds_for_rich_fixture() -> Result<()> {
    let repo = setup_timebomb_rich_repo()?;
    let output = repo.run_why_with_stdin(
        &["mcp"],
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"why_time_bombs\",\"arguments\":{\"max_age_days\":30}}}\n",
    )?;
    ensure_success(&output)?;

    let responses = response_lines(&output)?;
    let payload = responses[0]["result"]["content"][0]["json"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("time bomb result should be array"))?;
    assert_eq!(payload.len(), 2);
    assert!(
        payload
            .iter()
            .any(|finding| finding["kind"] == "PastDueTodo")
    );
    assert!(
        payload
            .iter()
            .any(|finding| finding["kind"] == "ExpiredRemoveAfter")
    );
    assert_json_golden(
        "mcp_why_time_bombs_timebomb_rich_repo",
        &time_bombs_golden_view(payload),
    )?;

    Ok(())
}

#[test]
fn mcp_why_hotspots_returns_ranked_findings() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why_with_stdin(
        &["mcp"],
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"why_hotspots\",\"arguments\":{\"limit\":5}}}\n",
    )?;
    ensure_success(&output)?;

    let responses = response_lines(&output)?;
    let payload = responses[0]["result"]["content"][0]["json"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("hotspots result should be array"))?;
    assert!(!payload.is_empty());
    assert!(payload[0]["path"].is_string());
    assert!(payload[0]["churn_commits"].as_u64().unwrap_or_default() >= 1);
    assert!(payload[0]["hotspot_score"].as_f64().unwrap_or_default() >= 1.0);
    assert_json_golden(
        "mcp_why_hotspots_hotfix_repo",
        &hotspots_golden_view(payload),
    )?;

    Ok(())
}

#[test]
fn mcp_why_coupling_returns_ranked_findings() -> Result<()> {
    let repo = setup_coupling_repo()?;
    let output = repo.run_why_with_stdin(
        &["mcp"],
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"why_coupling\",\"arguments\":{\"target\":\"src/schema.rs:1\",\"limit\":5}}}\n",
    )?;
    ensure_success(&output)?;

    let responses = response_lines(&output)?;
    let payload = &responses[0]["result"]["content"][0]["json"];
    assert_eq!(payload["target_path"], "src/schema.rs");
    assert_eq!(payload["target_commit_count"], 5);
    let results = payload["results"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("coupling result should include array"))?;
    assert!(!results.is_empty());
    assert_eq!(results[0]["path"], "src/data.rs");
    assert_eq!(results[0]["shared_commits"], 5);
    assert_eq!(results[0]["coupling_ratio"], 1.0);
    assert_json_golden(
        "mcp_why_coupling_coupling_repo",
        &coupling_golden_view(payload),
    )?;

    Ok(())
}

#[test]
fn mcp_why_coupling_returns_ranked_findings_for_rich_fixture() -> Result<()> {
    let repo = setup_coupling_rich_repo()?;
    let output = repo.run_why_with_stdin(
        &["mcp"],
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"why_coupling\",\"arguments\":{\"target\":\"src/schema.rs:1\",\"limit\":5}}}\n",
    )?;
    ensure_success(&output)?;

    let responses = response_lines(&output)?;
    let payload = &responses[0]["result"]["content"][0]["json"];
    assert_eq!(payload["target_path"], "src/schema.rs");
    assert_eq!(payload["target_commit_count"], 5);
    let results = payload["results"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("coupling result should include array"))?;
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["path"], "src/data.rs");
    assert_eq!(results[0]["shared_commits"], 5);
    assert_eq!(results[0]["coupling_ratio"], 1.0);
    assert_eq!(results[1]["path"], "src/metrics.rs");
    assert_eq!(results[1]["shared_commits"], 2);
    assert_eq!(results[1]["coupling_ratio"], 0.4);
    assert_json_golden(
        "mcp_why_coupling_coupling_rich_repo",
        &coupling_golden_view(payload),
    )?;

    Ok(())
}

#[test]
fn mcp_returns_structured_error_for_unknown_tool() -> Result<()> {
    let repo = setup_hotfix_repo()?;
    let output = repo.run_why_with_stdin(
        &["mcp"],
        "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/call\",\"params\":{\"name\":\"missing\",\"arguments\":{}}}\n",
    )?;
    ensure_success(&output)?;

    let responses = response_lines(&output)?;
    assert_eq!(responses[0]["id"], 7);
    assert_eq!(responses[0]["error"]["code"], -32602);
    assert!(
        responses[0]["error"]["message"]
            .as_str()
            .is_some_and(|msg| msg.contains("unknown tool"))
    );

    Ok(())
}
