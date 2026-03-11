//! Archaeology-guided split suggestions.

use anyhow::{Context, Result, anyhow, bail};
use git2::{BlameOptions, Oid, Repository};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use why_archaeologist::{
    CommitEvidence, LocalContext, RiskLevel, discover_repository, infer_risk_level,
    relative_repo_path,
};
use why_context::WhyConfig;
use why_locator::{QueryKind, QueryTarget, resolve_target};

const MIN_BLOCK_LINES: u32 = 5;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SplitSuggestion {
    pub path: PathBuf,
    pub symbol: String,
    pub start_line: u32,
    pub end_line: u32,
    pub total_lines: u32,
    pub blocks: Vec<SplitBlock>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SplitBlock {
    pub start_line: u32,
    pub end_line: u32,
    pub line_count: u32,
    pub percentage_of_function: u32,
    pub dominant_commit_oid: String,
    pub dominant_commit_short_oid: String,
    pub dominant_commit_summary: String,
    pub era_label: String,
    pub suggested_name: String,
    pub risk_level: RiskLevel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RawBlock {
    start_line: u32,
    end_line: u32,
    dominant_commit: Oid,
}

#[derive(Debug, Clone)]
struct CommitSummary {
    oid_text: String,
    short_oid: String,
    summary: String,
    message: String,
}

pub fn suggest_split(target: &QueryTarget, cwd: &Path) -> Result<Option<SplitSuggestion>> {
    if !matches!(
        target.query_kind,
        QueryKind::Symbol | QueryKind::QualifiedSymbol
    ) {
        bail!("split suggestions require a symbol or qualified symbol target");
    }

    let resolved = resolve_target(target, cwd)?;
    let symbol = resolved
        .symbol
        .clone()
        .ok_or_else(|| anyhow!("resolved target is missing symbol name"))?;
    let absolute_path = cwd.join(&resolved.path);
    let repo = discover_repository(&absolute_path)?;
    let relative_path = relative_repo_path(&repo, &absolute_path)?;
    let total_lines = resolved.end_line - resolved.start_line + 1;

    let raw_blocks = blame_blocks(
        &repo,
        &relative_path,
        resolved.start_line,
        resolved.end_line,
    )?;
    let merged = merge_small_blocks(raw_blocks, MIN_BLOCK_LINES);
    if merged.len() < 2 {
        return Ok(None);
    }

    let commit_summaries = load_commit_summaries(&repo, &merged)?;
    let config = WhyConfig::default();
    let blocks = merged
        .into_iter()
        .map(|block| build_split_block(block, &commit_summaries, total_lines, &symbol, &config))
        .collect::<Result<Vec<_>>>()?;

    Ok(Some(SplitSuggestion {
        path: relative_path,
        symbol,
        start_line: resolved.start_line,
        end_line: resolved.end_line,
        total_lines,
        blocks,
    }))
}

fn blame_blocks(
    repo: &Repository,
    relative_path: &Path,
    start_line: u32,
    end_line: u32,
) -> Result<Vec<RawBlock>> {
    let mut options = BlameOptions::new();
    options
        .min_line(start_line as usize)
        .max_line(end_line as usize);

    let blame = repo
        .blame_file(relative_path, Some(&mut options))
        .with_context(|| format!("failed to blame {}", relative_path.display()))?;

    let mut blocks = Vec::new();
    for hunk in blame.iter() {
        let oid = hunk.final_commit_id();
        if oid.is_zero() {
            continue;
        }

        let hunk_start = hunk.final_start_line() as u32;
        let hunk_end = hunk_start + hunk.lines_in_hunk() as u32 - 1;
        let overlap_start = hunk_start.max(start_line);
        let overlap_end = hunk_end.min(end_line);
        if overlap_end < overlap_start {
            continue;
        }

        blocks.push(RawBlock {
            start_line: overlap_start,
            end_line: overlap_end,
            dominant_commit: oid,
        });
    }

    if blocks.is_empty() {
        bail!(
            "no blame blocks found for {}:{}-{}",
            relative_path.display(),
            start_line,
            end_line
        );
    }

    Ok(blocks)
}

fn load_commit_summaries(
    repo: &Repository,
    blocks: &[RawBlock],
) -> Result<HashMap<Oid, CommitSummary>> {
    let mut summaries = HashMap::new();
    for block in blocks {
        if summaries.contains_key(&block.dominant_commit) {
            continue;
        }

        let commit = repo
            .find_commit(block.dominant_commit)
            .with_context(|| format!("failed to load blamed commit {}", block.dominant_commit))?;
        let oid_text = block.dominant_commit.to_string();
        summaries.insert(
            block.dominant_commit,
            CommitSummary {
                short_oid: oid_text.chars().take(8).collect(),
                oid_text,
                summary: commit.summary().unwrap_or("(no summary)").to_string(),
                message: commit
                    .message()
                    .unwrap_or("(no message)")
                    .trim()
                    .to_string(),
            },
        );
    }
    Ok(summaries)
}

fn build_split_block(
    block: RawBlock,
    commit_summaries: &HashMap<Oid, CommitSummary>,
    total_lines: u32,
    symbol: &str,
    config: &WhyConfig,
) -> Result<SplitBlock> {
    let commit = commit_summaries
        .get(&block.dominant_commit)
        .ok_or_else(|| anyhow!("missing commit summary for block"))?;
    let line_count = block.end_line - block.start_line + 1;
    let percentage_of_function = ((line_count * 100) + (total_lines / 2)) / total_lines;
    let risk_level = infer_risk_level(
        &[synthetic_commit_evidence(commit, line_count, total_lines)],
        &LocalContext {
            comments: Vec::new(),
            markers: Vec::new(),
            risk_flags: Vec::new(),
        },
        config,
    );

    Ok(SplitBlock {
        start_line: block.start_line,
        end_line: block.end_line,
        line_count,
        percentage_of_function,
        dominant_commit_oid: commit.oid_text.clone(),
        dominant_commit_short_oid: commit.short_oid.clone(),
        dominant_commit_summary: commit.summary.clone(),
        era_label: era_label(&commit.summary).to_string(),
        suggested_name: suggested_extraction_name(symbol, &commit.summary),
        risk_level,
    })
}

fn synthetic_commit_evidence(
    commit: &CommitSummary,
    line_count: u32,
    total_lines: u32,
) -> CommitEvidence {
    CommitEvidence {
        oid: commit.oid_text.clone(),
        short_oid: commit.short_oid.clone(),
        author: "unknown".to_string(),
        email: "unknown".to_string(),
        time: 0,
        date: "unknown".to_string(),
        summary: commit.summary.clone(),
        message: commit.message.clone(),
        diff_excerpt: String::new(),
        coverage_score: line_count as f32 / total_lines as f32,
        relevance_score: 0.0,
        issue_refs: Vec::new(),
        is_mechanical: false,
    }
}

fn merge_small_blocks(mut blocks: Vec<RawBlock>, min_block_lines: u32) -> Vec<RawBlock> {
    if blocks.len() < 2 {
        return blocks;
    }

    loop {
        let Some(index) = blocks
            .iter()
            .position(|block| block_len(block) < min_block_lines)
        else {
            break;
        };

        if blocks.len() == 1 {
            break;
        }

        if index == 0 {
            let next = blocks.remove(1);
            blocks[0].end_line = next.end_line;
            blocks[0].dominant_commit = next.dominant_commit;
            continue;
        }

        if index == blocks.len() - 1 {
            let tail = blocks.remove(index);
            blocks[index - 1].end_line = tail.end_line;
            continue;
        }

        let left_len = block_len(&blocks[index - 1]);
        let right_len = block_len(&blocks[index + 1]);
        let small = blocks.remove(index);
        if left_len >= right_len {
            blocks[index - 1].end_line = small.end_line;
        } else {
            blocks[index].start_line = small.start_line;
        }
    }

    coalesce_adjacent_same_commit(blocks)
}

fn coalesce_adjacent_same_commit(blocks: Vec<RawBlock>) -> Vec<RawBlock> {
    let mut merged: Vec<RawBlock> = Vec::new();
    for block in blocks {
        if let Some(last) = merged.last_mut()
            && last.dominant_commit == block.dominant_commit
            && last.end_line + 1 >= block.start_line
        {
            last.end_line = block.end_line;
        } else {
            merged.push(block);
        }
    }
    merged
}

fn block_len(block: &RawBlock) -> u32 {
    block.end_line - block.start_line + 1
}

fn era_label(summary: &str) -> &'static str {
    let summary = summary.to_ascii_lowercase();
    if contains_any(&summary, &["hotfix", "security", "patch"]) {
        "Security hardening era"
    } else if contains_any(&summary, &["compat", "legacy", "shim"]) {
        "Backward compat era"
    } else if contains_any(&summary, &["migration", "upgrade"]) {
        "Migration era"
    } else if contains_any(&summary, &["fix", "bug"]) {
        "Bug fix era"
    } else if contains_any(&summary, &["feat", "add", "implement"]) {
        "Feature era"
    } else {
        "Unknown era"
    }
}

fn suggested_extraction_name(symbol: &str, summary: &str) -> String {
    let base = base_symbol_name(symbol);
    let summary = summary.to_ascii_lowercase();
    let suffix = if contains_any(&summary, &["guard", "security", "auth"]) {
        "with_guard"
    } else if contains_any(&summary, &["legacy", "compat", "v1"]) {
        "legacy"
    } else if contains_any(&summary, &["migration", "upgrade"]) {
        "migration_path"
    } else {
        "inner"
    };
    format!("{base}_{suffix}")
}

fn base_symbol_name(symbol: &str) -> &str {
    symbol.rsplit("::").next().unwrap_or(symbol)
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::{
        RawBlock, base_symbol_name, coalesce_adjacent_same_commit, era_label, merge_small_blocks,
        suggested_extraction_name,
    };
    use git2::{Repository, Signature};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn classifies_era_labels() {
        assert_eq!(
            era_label("hotfix: patch auth bypass"),
            "Security hardening era"
        );
        assert_eq!(
            era_label("feat: add legacy compat shim"),
            "Backward compat era"
        );
        assert_eq!(era_label("chore: rearrange code"), "Unknown era");
    }

    #[test]
    fn suggests_extraction_names() {
        assert_eq!(
            suggested_extraction_name("authenticate", "security auth hardening"),
            "authenticate_with_guard"
        );
        assert_eq!(
            suggested_extraction_name("AuthService::login", "legacy compat path"),
            "login_legacy"
        );
        assert_eq!(
            suggested_extraction_name("migrate", "upgrade storage engine"),
            "migrate_migration_path"
        );
        assert_eq!(base_symbol_name("Foo::bar"), "bar");
    }

    #[test]
    fn merges_small_middle_block_into_larger_neighbor() {
        let a = oid(1);
        let b = oid(2);
        let c = oid(3);
        let blocks = vec![
            RawBlock {
                start_line: 10,
                end_line: 19,
                dominant_commit: a,
            },
            RawBlock {
                start_line: 20,
                end_line: 22,
                dominant_commit: b,
            },
            RawBlock {
                start_line: 23,
                end_line: 34,
                dominant_commit: c,
            },
        ];

        let merged = merge_small_blocks(blocks, 5);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[1].start_line, 20);
        assert_eq!(merged[1].end_line, 34);
        assert_eq!(merged[1].dominant_commit, c);
    }

    #[test]
    fn coalesces_adjacent_blocks_with_same_commit() {
        let a = oid(1);
        let merged = coalesce_adjacent_same_commit(vec![
            RawBlock {
                start_line: 1,
                end_line: 4,
                dominant_commit: a,
            },
            RawBlock {
                start_line: 5,
                end_line: 8,
                dominant_commit: a,
            },
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].start_line, 1);
        assert_eq!(merged[0].end_line, 8);
    }

    #[test]
    fn can_blame_fixture_like_history_end_to_end() {
        let dir = tempdir().expect("tempdir");
        let repo = Repository::init(dir.path()).expect("init repo");
        let sig = Signature::now("Test User", "test@example.com").expect("signature");
        let file_path = dir.path().join("sample.rs");

        fs::write(&file_path, "fn authenticate() {\n    guard();\n}\n")
            .expect("write first version");
        commit_all(&repo, &sig, "hotfix: auth guard");

        fs::write(
            &file_path,
            "fn authenticate() {\n    guard();\n    legacy_v1();\n    legacy_v1_fallback();\n    legacy_v1_cleanup();\n    legacy_v1_metrics();\n    legacy_v1_finish();\n}\n",
        )
        .expect("write second version");
        commit_all(&repo, &sig, "feat: add legacy compat path");

        let blamed = repo
            .blame_file(std::path::Path::new("sample.rs"), None)
            .expect("blame file");
        assert!(blamed.iter().count() >= 2);
    }

    fn commit_all(repo: &Repository, sig: &Signature<'_>, message: &str) {
        let mut index = repo.index().expect("index");
        index
            .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .expect("add all");
        index.write().expect("write index");
        let tree_id = index.write_tree().expect("write tree");
        let tree = repo.find_tree(tree_id).expect("find tree");
        let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
        let parents: Vec<_> = parent.iter().collect();
        repo.commit(Some("HEAD"), sig, sig, message, &tree, &parents)
            .expect("commit");
    }

    fn oid(byte: u8) -> git2::Oid {
        git2::Oid::from_bytes(&[byte; 20]).expect("oid")
    }
}
