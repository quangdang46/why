use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const FRONTMATTER_DELIMITER: &str = "---";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InvestigationWorkflow {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub target_hint: Option<String>,
    pub body: String,
    pub path: PathBuf,
}

pub fn builtin_workflows_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../workflows")
        .canonicalize()
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workflows"))
}

pub fn load_builtin_workflows() -> Result<Vec<InvestigationWorkflow>> {
    load_workflows_from(&builtin_workflows_dir())
}

pub fn load_builtin_workflow(id: &str) -> Result<Option<InvestigationWorkflow>> {
    load_workflow_from(&builtin_workflows_dir(), id)
}

pub fn load_workflows_from(dir: &Path) -> Result<Vec<InvestigationWorkflow>> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("failed to read workflows directory {}", dir.display()))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());

    let mut workflows = Vec::new();
    for entry in entries {
        let path = entry.path();
        if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        workflows.push(parse_workflow_file(&path)?);
    }

    Ok(workflows)
}

pub fn load_workflow_from(dir: &Path, id: &str) -> Result<Option<InvestigationWorkflow>> {
    let requested = normalize_id(id);
    for workflow in load_workflows_from(dir)? {
        if workflow.id == requested {
            return Ok(Some(workflow));
        }
    }
    Ok(None)
}

fn parse_workflow_file(path: &Path) -> Result<InvestigationWorkflow> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read workflow file {}", path.display()))?;
    let (frontmatter, body) = split_frontmatter(&source)
        .with_context(|| format!("failed to parse workflow frontmatter in {}", path.display()))?;
    let id = frontmatter
        .get("id")
        .cloned()
        .unwrap_or_else(|| workflow_id_from_path(path));
    let title = frontmatter
        .get("title")
        .cloned()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("workflow is missing a title"))?;
    let summary = frontmatter
        .get("summary")
        .cloned()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("workflow is missing a summary"))?;
    let target_hint = frontmatter.get("target_hint").cloned();
    if body.trim().is_empty() {
        bail!("workflow body must not be empty");
    }

    Ok(InvestigationWorkflow {
        id: normalize_id(&id),
        title,
        summary,
        target_hint,
        body: body.trim().to_string(),
        path: path.to_path_buf(),
    })
}

fn split_frontmatter(source: &str) -> Result<(std::collections::BTreeMap<String, String>, String)> {
    let mut lines = source.lines();
    if lines.next().map(str::trim) != Some(FRONTMATTER_DELIMITER) {
        bail!("workflow file must start with YAML-style frontmatter");
    }

    let mut frontmatter = std::collections::BTreeMap::new();
    let mut saw_end = false;
    for line in lines.by_ref() {
        if line.trim() == FRONTMATTER_DELIMITER {
            saw_end = true;
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("invalid frontmatter line: {line}"))?;
        frontmatter.insert(key.trim().to_string(), value.trim().to_string());
    }

    if !saw_end {
        bail!("workflow frontmatter must end with ---");
    }

    Ok((frontmatter, lines.collect::<Vec<_>>().join("\n")))
}

fn workflow_id_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(normalize_id)
        .unwrap_or_else(|| "workflow".to_string())
}

fn normalize_id(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace('_', "-")
}

#[cfg(test)]
mod tests {
    use super::{load_workflow_from, load_workflows_from, parse_workflow_file};
    use anyhow::Result;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn parses_markdown_workflow_frontmatter_and_body() -> Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("root-cause.md");
        fs::write(
            &path,
            r#"---
title: Root Cause Archaeology
summary: Build a commit-backed narrative for a regression.
target_hint: symbol-or-range
---
1. Resolve the target.
2. Walk its history.
"#,
        )?;

        let workflow = parse_workflow_file(&path)?;
        assert_eq!(workflow.id, "root-cause");
        assert_eq!(workflow.title, "Root Cause Archaeology");
        assert_eq!(
            workflow.summary,
            "Build a commit-backed narrative for a regression."
        );
        assert_eq!(workflow.target_hint.as_deref(), Some("symbol-or-range"));
        assert!(workflow.body.contains("Walk its history"));
        Ok(())
    }

    #[test]
    fn loads_workflows_sorted_and_reloads_from_disk() -> Result<()> {
        let dir = TempDir::new()?;
        let workflows_dir = dir.path().join("workflows");
        fs::create_dir_all(&workflows_dir)?;
        let alpha = workflows_dir.join("alpha.md");
        let beta = workflows_dir.join("beta.md");
        fs::write(
            &beta,
            "---\ntitle: Beta\nsummary: Second workflow.\n---\nBeta body.\n",
        )?;
        fs::write(
            &alpha,
            "---\ntitle: Alpha\nsummary: First workflow.\n---\nAlpha body.\n",
        )?;

        let workflows = load_workflows_from(&workflows_dir)?;
        assert_eq!(workflows.len(), 2);
        assert_eq!(workflows[0].id, "alpha");
        assert_eq!(workflows[1].id, "beta");

        fs::write(
            &alpha,
            "---\ntitle: Alpha\nsummary: Reloaded workflow summary.\n---\nUpdated body.\n",
        )?;
        let reloaded = load_workflow_from(&workflows_dir, "alpha")?
            .expect("workflow should exist after reload");
        assert_eq!(reloaded.summary, "Reloaded workflow summary.");
        assert!(reloaded.body.contains("Updated body"));
        Ok(())
    }
}
