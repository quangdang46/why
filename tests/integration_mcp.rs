mod common;

use anyhow::Result;
use common::{ensure_success, setup_hotfix_repo, setup_split_repo, setup_timebomb_repo};
use serde_json::Value;

fn response_lines(output: &std::process::Output) -> Vec<Value> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("response line should be JSON"))
        .collect()
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

    let responses = response_lines(&output);
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(responses[0]["result"]["protocolVersion"], "2.0");
    let tools = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools/list should return tool array");
    assert_eq!(tools.len(), 3);
    assert_eq!(tools[0]["name"], "why_symbol");
    assert_eq!(tools[1]["name"], "why_split");
    assert_eq!(tools[2]["name"], "why_time_bombs");

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

    let responses = response_lines(&output);
    let payload = &responses[0]["result"]["content"][0]["json"];
    assert_eq!(payload["target"]["path"], "src/payment.rs");
    assert_eq!(payload["target"]["query_kind"], "symbol");
    assert_eq!(payload["risk_level"], "HIGH");
    assert!(payload["commits"].as_array().is_some_and(|items| !items.is_empty()));

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

    let responses = response_lines(&output);
    let payload = &responses[0]["result"]["content"][0]["json"];
    assert_eq!(payload["path"], "src/auth.rs");
    assert_eq!(payload["symbol"], "authenticate");
    assert_eq!(payload["blocks"].as_array().map(|blocks| blocks.len()), Some(2));

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

    let responses = response_lines(&output);
    let payload = responses[0]["result"]["content"][0]["json"]
        .as_array()
        .expect("time bomb result should be array");
    assert!(!payload.is_empty());
    assert!(payload.iter().any(|finding| finding["kind"] == "PastDueTodo"));

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

    let responses = response_lines(&output);
    assert_eq!(responses[0]["id"], 7);
    assert_eq!(responses[0]["error"]["code"], -32602);
    assert!(responses[0]["error"]["message"]
        .as_str()
        .is_some_and(|msg| msg.contains("unknown tool")));

    Ok(())
}
