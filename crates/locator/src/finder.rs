use crate::{QueryKind, QueryTarget, SupportedLanguage};
use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use tree_sitter::{Parser, QueryCursor};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub path: PathBuf,
    pub start_line: u32,
    pub end_line: u32,
    pub query_kind: QueryKind,
    pub symbol: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SymbolMatch {
    name: String,
    start_line: u32,
    end_line: u32,
}

pub fn resolve_target(target: &QueryTarget, cwd: &Path) -> Result<ResolvedTarget> {
    match target.query_kind {
        QueryKind::Line | QueryKind::Range => {
            let (start_line, end_line) = target.line_range().ok_or_else(|| {
                anyhow!(
                    "target {:?} is missing a concrete line range",
                    target.query_kind
                )
            })?;

            Ok(ResolvedTarget {
                path: target.path.clone(),
                start_line,
                end_line,
                query_kind: target.query_kind,
                symbol: None,
            })
        }
        QueryKind::Symbol => resolve_symbol_target(target, cwd),
        QueryKind::QualifiedSymbol => bail!(
            "qualified symbol resolution is not implemented yet for {}",
            target.path.display()
        ),
    }
}

fn resolve_symbol_target(target: &QueryTarget, cwd: &Path) -> Result<ResolvedTarget> {
    let symbol = target
        .symbol
        .as_deref()
        .ok_or_else(|| anyhow!("symbol queries require a symbol specifier"))?;
    let absolute_path = cwd.join(&target.path);
    let language = SupportedLanguage::detect(&absolute_path)?;

    if language != SupportedLanguage::Rust {
        bail!(
            "symbol resolution for {} is not implemented yet; currently supported: Rust (.rs)",
            language.grammar_name()
        );
    }

    let source = fs::read_to_string(&absolute_path)
        .with_context(|| format!("failed to read {}", absolute_path.display()))?;
    let matches = collect_symbol_matches(language, &source)?;

    let matched: Vec<_> = matches
        .into_iter()
        .filter(|candidate| candidate.name == symbol)
        .collect();

    match matched.as_slice() {
        [] => bail!(
            "symbol '{symbol}' was not found in {}",
            target.path.display()
        ),
        [candidate] => Ok(ResolvedTarget {
            path: target.path.clone(),
            start_line: candidate.start_line,
            end_line: candidate.end_line,
            query_kind: target.query_kind,
            symbol: Some(symbol.to_string()),
        }),
        candidates => {
            let spans = candidates
                .iter()
                .map(|candidate| {
                    format!(
                        "{}:{}-{}",
                        target.path.display(),
                        candidate.start_line,
                        candidate.end_line
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "symbol '{symbol}' is ambiguous in {}. Candidates: {spans}",
                target.path.display()
            )
        }
    }
}

fn collect_symbol_matches(language: SupportedLanguage, source: &str) -> Result<Vec<SymbolMatch>> {
    let mut parser = Parser::new();
    parser
        .set_language(&language.tree_sitter_language())
        .map_err(|error| anyhow!(error.to_string()))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow!("tree-sitter failed to parse source text"))?;
    let query = language.load_symbol_query()?;
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = Vec::new();

    for query_match in cursor.matches(&query, tree.root_node(), source.as_bytes()) {
        let mut name = None;
        let mut start_line = None;
        let mut end_line = None;

        for capture in query_match.captures {
            match capture_names[capture.index as usize] {
                "symbol.name" => {
                    name = Some(
                        capture
                            .node
                            .utf8_text(source.as_bytes())
                            .map_err(|error| anyhow!(error.to_string()))?
                            .to_string(),
                    );
                }
                "symbol.definition" => {
                    start_line = Some(capture.node.start_position().row as u32 + 1);
                    end_line = Some(capture.node.end_position().row as u32 + 1);
                }
                _ => {}
            }
        }

        if let (Some(name), Some(start_line), Some(end_line)) = (name, start_line, end_line) {
            matches.push(SymbolMatch {
                name,
                start_line,
                end_line,
            });
        }
    }

    Ok(matches)
}

#[cfg(test)]
mod tests {
    use super::{ResolvedTarget, collect_symbol_matches, resolve_target};
    use crate::{QueryKind, QueryTarget, SupportedLanguage};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempSourceDir {
        path: PathBuf,
    }

    impl TempSourceDir {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be valid")
                .as_nanos();
            let path = std::env::temp_dir().join(format!("why-locator-test-{unique}"));
            fs::create_dir_all(&path).expect("temp dir should be created");
            Self { path }
        }

        fn write_file(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.path.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("parent dir should exist");
            }
            fs::write(&path, contents).expect("test source file should be written");
            path
        }
    }

    impl Drop for TempSourceDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn resolves_rust_symbol_to_exact_line_range() {
        let temp = TempSourceDir::new();
        temp.write_file(
            "src/lib.rs",
            "pub struct AuthService;\n\nimpl AuthService {\n    pub fn login(&self) -> bool {\n        true\n    }\n}\n\npub fn authenticate() -> bool {\n    AuthService.login()\n}\n",
        );

        let target = QueryTarget {
            path: PathBuf::from("src/lib.rs"),
            start_line: None,
            end_line: None,
            symbol: Some("authenticate".into()),
            query_kind: QueryKind::Symbol,
        };

        let resolved = resolve_target(&target, &temp.path).expect("symbol should resolve");
        assert_eq!(
            resolved,
            ResolvedTarget {
                path: PathBuf::from("src/lib.rs"),
                start_line: 8,
                end_line: 10,
                query_kind: QueryKind::Symbol,
                symbol: Some("authenticate".into()),
            }
        );
    }

    #[test]
    fn rejects_ambiguous_rust_symbol_matches() {
        let temp = TempSourceDir::new();
        temp.write_file(
            "src/lib.rs",
            "pub fn duplicate() -> bool {\n    true\n}\n\nmod nested {\n    pub fn duplicate() -> bool {\n        false\n    }\n}\n",
        );

        let target = QueryTarget {
            path: PathBuf::from("src/lib.rs"),
            start_line: None,
            end_line: None,
            symbol: Some("duplicate".into()),
            query_kind: QueryKind::Symbol,
        };

        let error = resolve_target(&target, &temp.path).expect_err("duplicate symbols should fail");
        let message = error.to_string();
        assert!(message.contains("symbol 'duplicate' is ambiguous"));
        assert!(message.contains("src/lib.rs:1-3"));
        assert!(message.contains("src/lib.rs:5-7"));
    }

    #[test]
    fn rejects_unsupported_symbol_language_for_now() {
        let temp = TempSourceDir::new();
        temp.write_file("src/app.ts", "function authenticate() { return true }\n");

        let target = QueryTarget {
            path: PathBuf::from("src/app.ts"),
            start_line: None,
            end_line: None,
            symbol: Some("authenticate".into()),
            query_kind: QueryKind::Symbol,
        };

        let error = resolve_target(&target, &temp.path)
            .expect_err("non-Rust symbol resolution should be deferred");
        assert!(
            error
                .to_string()
                .contains("symbol resolution for typescript is not implemented yet")
        );
    }

    #[test]
    fn collects_rust_symbol_matches_from_source() {
        let source = "pub struct AuthService;\n\npub fn authenticate() -> bool {\n    true\n}\n";
        let matches = collect_symbol_matches(SupportedLanguage::Rust, source)
            .expect("rust symbols should be collected");

        assert!(matches.iter().any(|candidate| {
            candidate.name == "AuthService" && candidate.start_line == 1 && candidate.end_line == 1
        }));
        assert!(matches.iter().any(|candidate| {
            candidate.name == "authenticate" && candidate.start_line == 3 && candidate.end_line == 5
        }));
    }
}
