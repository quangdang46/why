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
    qualified_name: Option<String>,
    start_line: u32,
    end_line: u32,
    nesting_depth: usize,
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
        QueryKind::Symbol | QueryKind::QualifiedSymbol => resolve_symbol_target(target, cwd),
    }
}

pub fn list_all_symbols(
    language: SupportedLanguage,
    source: &str,
) -> Result<Vec<(String, u32, u32)>> {
    let mut symbols: Vec<_> = collect_symbol_matches(language, source)?
        .into_iter()
        .map(|candidate| (candidate.name, candidate.start_line, candidate.end_line))
        .collect();
    symbols.sort_by(|left, right| {
        left.1
            .cmp(&right.1)
            .then(left.2.cmp(&right.2))
            .then(left.0.cmp(&right.0))
    });
    Ok(symbols)
}

fn resolve_symbol_target(target: &QueryTarget, cwd: &Path) -> Result<ResolvedTarget> {
    let symbol = target
        .symbol
        .as_deref()
        .ok_or_else(|| anyhow!("symbol queries require a symbol specifier"))?;
    let absolute_path = cwd.join(&target.path);
    let language = SupportedLanguage::detect(&absolute_path)?;

    let source = fs::read_to_string(&absolute_path)
        .with_context(|| format!("failed to read {}", absolute_path.display()))?;
    let matches = collect_symbol_matches(language, &source)?;

    let matched: Vec<_> = matches
        .into_iter()
        .filter(|candidate| match target.query_kind {
            QueryKind::Symbol => candidate.name == symbol,
            QueryKind::QualifiedSymbol => candidate.qualified_name.as_deref() == Some(symbol),
            _ => false,
        })
        .collect();

    let candidate = match matched.as_slice() {
        [] => {
            bail!(
                "symbol '{symbol}' was not found in {}",
                target.path.display()
            )
        }
        [candidate] => candidate,
        candidates => resolve_ambiguous_symbol(language, &target.path, symbol, candidates)?,
    };

    Ok(ResolvedTarget {
        path: target.path.clone(),
        start_line: candidate.start_line,
        end_line: candidate.end_line,
        query_kind: target.query_kind,
        symbol: Some(symbol.to_string()),
    })
}

fn resolve_ambiguous_symbol<'a>(
    language: SupportedLanguage,
    path: &Path,
    symbol: &str,
    candidates: &'a [SymbolMatch],
) -> Result<&'a SymbolMatch> {
    if matches!(
        language,
        SupportedLanguage::JavaScript | SupportedLanguage::TypeScript | SupportedLanguage::Python
    ) {
        let min_depth = candidates
            .iter()
            .map(|candidate| candidate.nesting_depth)
            .min()
            .ok_or_else(|| anyhow!("ambiguous symbol set cannot be empty"))?;
        let shallowest: Vec<_> = candidates
            .iter()
            .filter(|candidate| candidate.nesting_depth == min_depth)
            .collect();
        if let [candidate] = shallowest.as_slice() {
            return Ok(candidate);
        }
    }

    let spans = candidates
        .iter()
        .map(|candidate| {
            format!(
                "{}:{}-{}",
                path.display(),
                candidate.start_line,
                candidate.end_line
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    bail!(
        "symbol '{symbol}' is ambiguous in {}. Candidates: {spans}",
        path.display()
    )
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
    let mut matches: Vec<SymbolMatch> = Vec::new();

    for query_match in cursor.matches(&query, tree.root_node(), source.as_bytes()) {
        let mut name = None;
        let mut start_line = None;
        let mut end_line = None;
        let mut definition_node = None;

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
                    definition_node = Some(capture.node);
                }
                _ => {}
            }
        }

        if let (Some(name), Some(start_line), Some(end_line), Some(definition_node)) =
            (name, start_line, end_line, definition_node)
        {
            let symbol_match = SymbolMatch {
                qualified_name: qualified_rust_name(definition_node, &name, source)?,
                name,
                start_line,
                end_line,
                nesting_depth: named_ancestor_depth(definition_node),
            };

            if let Some(existing) = matches.iter_mut().find(|existing| {
                existing.name == symbol_match.name
                    && existing.qualified_name == symbol_match.qualified_name
                    && existing.start_line <= symbol_match.start_line
                    && existing.end_line >= symbol_match.end_line
            }) {
                if symbol_match.start_line < existing.start_line
                    || symbol_match.end_line > existing.end_line
                {
                    *existing = symbol_match;
                }
            } else if !matches.iter().any(|existing| {
                existing.name == symbol_match.name
                    && existing.qualified_name == symbol_match.qualified_name
                    && existing.start_line == symbol_match.start_line
                    && existing.end_line == symbol_match.end_line
            }) {
                matches.push(symbol_match);
            }
        }
    }

    Ok(matches)
}

fn named_ancestor_depth(node: tree_sitter::Node<'_>) -> usize {
    let mut depth = 0;
    let mut current = node.parent();

    while let Some(parent) = current {
        if parent.is_named() {
            depth += 1;
        }
        current = parent.parent();
    }

    depth
}

fn qualified_rust_name(
    definition_node: tree_sitter::Node<'_>,
    symbol_name: &str,
    source: &str,
) -> Result<Option<String>> {
    if definition_node.kind() != "function_item" {
        return Ok(None);
    }

    let mut current = definition_node.parent();
    while let Some(parent) = current {
        if parent.kind() == "impl_item" {
            if let Some(type_name) = extract_impl_type_name(parent, source)? {
                return Ok(Some(format!("{type_name}::{symbol_name}")));
            }
            break;
        }
        current = parent.parent();
    }

    Ok(None)
}

fn extract_impl_type_name(
    impl_node: tree_sitter::Node<'_>,
    source: &str,
) -> Result<Option<String>> {
    if let Some(type_node) = impl_node.child_by_field_name("type") {
        return first_named_identifier(type_node, source);
    }

    first_named_identifier(impl_node, source)
}

fn first_named_identifier(node: tree_sitter::Node<'_>, source: &str) -> Result<Option<String>> {
    if matches!(node.kind(), "type_identifier" | "identifier") {
        return Ok(Some(
            node.utf8_text(source.as_bytes())
                .map_err(|error| anyhow!(error.to_string()))?
                .to_string(),
        ));
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = first_named_identifier(child, source)? {
            return Ok(Some(found));
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::{ResolvedTarget, collect_symbol_matches, list_all_symbols, resolve_target};
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
                start_line: 9,
                end_line: 11,
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
        assert!(message.contains("src/lib.rs:6-8"));
    }

    #[test]
    fn resolves_typescript_function_symbol() {
        let temp = TempSourceDir::new();
        temp.write_file(
            "src/app.ts",
            "export function authenticate(): boolean {\n    return true;\n}\n",
        );

        let target = QueryTarget {
            path: PathBuf::from("src/app.ts"),
            start_line: None,
            end_line: None,
            symbol: Some("authenticate".into()),
            query_kind: QueryKind::Symbol,
        };

        let resolved =
            resolve_target(&target, &temp.path).expect("TypeScript symbol should resolve");
        assert_eq!(
            resolved,
            ResolvedTarget {
                path: PathBuf::from("src/app.ts"),
                start_line: 1,
                end_line: 3,
                query_kind: QueryKind::Symbol,
                symbol: Some("authenticate".into()),
            }
        );
    }

    #[test]
    fn resolves_javascript_class_method_symbol() {
        let temp = TempSourceDir::new();
        temp.write_file(
            "src/app.js",
            "class AuthService {\n  login() {\n    return true;\n  }\n}\n",
        );

        let target = QueryTarget {
            path: PathBuf::from("src/app.js"),
            start_line: None,
            end_line: None,
            symbol: Some("login".into()),
            query_kind: QueryKind::Symbol,
        };

        let resolved =
            resolve_target(&target, &temp.path).expect("JavaScript symbol should resolve");
        assert_eq!(
            resolved,
            ResolvedTarget {
                path: PathBuf::from("src/app.js"),
                start_line: 2,
                end_line: 4,
                query_kind: QueryKind::Symbol,
                symbol: Some("login".into()),
            }
        );
    }

    #[test]
    fn resolves_exported_typescript_function_symbol() {
        let temp = TempSourceDir::new();
        temp.write_file(
            "src/app.ts",
            "export function authenticate(token: string): boolean {\n    return token.length > 0;\n}\n",
        );

        let target = QueryTarget {
            path: PathBuf::from("src/app.ts"),
            start_line: None,
            end_line: None,
            symbol: Some("authenticate".into()),
            query_kind: QueryKind::Symbol,
        };

        let resolved = resolve_target(&target, &temp.path)
            .expect("exported TypeScript function should resolve");
        assert_eq!(
            resolved,
            ResolvedTarget {
                path: PathBuf::from("src/app.ts"),
                start_line: 1,
                end_line: 3,
                query_kind: QueryKind::Symbol,
                symbol: Some("authenticate".into()),
            }
        );
    }

    #[test]
    fn resolves_typescript_arrow_function_symbol() {
        let temp = TempSourceDir::new();
        temp.write_file(
            "src/app.ts",
            "export const authenticate = (token: string): boolean => {\n    return token.length > 0;\n};\n",
        );

        let target = QueryTarget {
            path: PathBuf::from("src/app.ts"),
            start_line: None,
            end_line: None,
            symbol: Some("authenticate".into()),
            query_kind: QueryKind::Symbol,
        };

        let resolved =
            resolve_target(&target, &temp.path).expect("TypeScript arrow function should resolve");
        assert_eq!(
            resolved,
            ResolvedTarget {
                path: PathBuf::from("src/app.ts"),
                start_line: 1,
                end_line: 3,
                query_kind: QueryKind::Symbol,
                symbol: Some("authenticate".into()),
            }
        );
    }

    #[test]
    fn resolves_javascript_arrow_function_symbol() {
        let temp = TempSourceDir::new();
        temp.write_file(
            "src/app.js",
            "const login = (token) => {\n  return Boolean(token);\n};\n",
        );

        let target = QueryTarget {
            path: PathBuf::from("src/app.js"),
            start_line: None,
            end_line: None,
            symbol: Some("login".into()),
            query_kind: QueryKind::Symbol,
        };

        let resolved =
            resolve_target(&target, &temp.path).expect("JavaScript arrow function should resolve");
        assert_eq!(
            resolved,
            ResolvedTarget {
                path: PathBuf::from("src/app.js"),
                start_line: 1,
                end_line: 3,
                query_kind: QueryKind::Symbol,
                symbol: Some("login".into()),
            }
        );
    }

    #[test]
    fn resolves_go_function_symbol() {
        let temp = TempSourceDir::new();
        temp.write_file(
            "src/main.go",
            "package main\n\nfunc Authenticate(token string) bool {\n    return token != \"\"\n}\n",
        );

        let target = QueryTarget {
            path: PathBuf::from("src/main.go"),
            start_line: None,
            end_line: None,
            symbol: Some("Authenticate".into()),
            query_kind: QueryKind::Symbol,
        };

        let resolved = resolve_target(&target, &temp.path).expect("Go symbol should resolve");
        assert_eq!(
            resolved,
            ResolvedTarget {
                path: PathBuf::from("src/main.go"),
                start_line: 3,
                end_line: 5,
                query_kind: QueryKind::Symbol,
                symbol: Some("Authenticate".into()),
            }
        );
    }

    #[test]
    fn resolves_java_method_symbol() {
        let temp = TempSourceDir::new();
        temp.write_file(
            "src/AuthService.java",
            "class AuthService {\n    boolean login() {\n        return true;\n    }\n}\n",
        );

        let target = QueryTarget {
            path: PathBuf::from("src/AuthService.java"),
            start_line: None,
            end_line: None,
            symbol: Some("login".into()),
            query_kind: QueryKind::Symbol,
        };

        let resolved = resolve_target(&target, &temp.path).expect("Java symbol should resolve");
        assert_eq!(
            resolved,
            ResolvedTarget {
                path: PathBuf::from("src/AuthService.java"),
                start_line: 2,
                end_line: 4,
                query_kind: QueryKind::Symbol,
                symbol: Some("login".into()),
            }
        );
    }

    #[test]
    fn collects_rust_symbol_matches_from_source() {
        let source = "pub struct AuthService;\n\nimpl AuthService {\n    pub fn login(&self) -> bool {\n        true\n    }\n}\n\npub fn authenticate() -> bool {\n    true\n}\n";
        let matches = collect_symbol_matches(SupportedLanguage::Rust, source)
            .expect("rust symbols should be collected");

        assert!(matches.iter().any(|candidate| {
            candidate.name == "AuthService" && candidate.start_line == 1 && candidate.end_line == 1
        }));
        assert!(matches.iter().any(|candidate| {
            candidate.name == "login"
                && candidate.qualified_name.as_deref() == Some("AuthService::login")
                && candidate.start_line == 4
                && candidate.end_line == 6
                && candidate.nesting_depth > 0
        }));
        assert!(matches.iter().any(|candidate| {
            candidate.name == "authenticate"
                && candidate.start_line == 9
                && candidate.end_line == 11
        }));
    }

    #[test]
    fn resolves_qualified_rust_impl_method() {
        let temp = TempSourceDir::new();
        temp.write_file(
            "src/lib.rs",
            "pub struct AuthService;\n\nimpl AuthService {\n    pub fn login(&self) -> bool {\n        true\n    }\n}\n",
        );

        let target = QueryTarget {
            path: PathBuf::from("src/lib.rs"),
            start_line: None,
            end_line: None,
            symbol: Some("AuthService::login".into()),
            query_kind: QueryKind::QualifiedSymbol,
        };

        let resolved =
            resolve_target(&target, &temp.path).expect("qualified symbol should resolve");
        assert_eq!(
            resolved,
            ResolvedTarget {
                path: PathBuf::from("src/lib.rs"),
                start_line: 4,
                end_line: 6,
                query_kind: QueryKind::QualifiedSymbol,
                symbol: Some("AuthService::login".into()),
            }
        );
    }

    #[test]
    fn resolves_python_decorated_function_symbol() {
        let temp = TempSourceDir::new();
        temp.write_file(
            "src/auth.py",
            "def audit_auth(fn):\n    return fn\n\n@audit_auth\ndef authenticate(token: str) -> bool:\n    return token.startswith(\"sk-\")\n",
        );

        let target = QueryTarget {
            path: PathBuf::from("src/auth.py"),
            start_line: None,
            end_line: None,
            symbol: Some("authenticate".into()),
            query_kind: QueryKind::Symbol,
        };

        let resolved = resolve_target(&target, &temp.path).expect("python symbol should resolve");
        assert_eq!(
            resolved,
            ResolvedTarget {
                path: PathBuf::from("src/auth.py"),
                start_line: 4,
                end_line: 6,
                query_kind: QueryKind::Symbol,
                symbol: Some("authenticate".into()),
            }
        );
    }

    #[test]
    fn resolves_python_top_level_function_over_method_with_same_name() {
        let temp = TempSourceDir::new();
        temp.write_file(
            "src/auth.py",
            "class AuthService:\n    def login(self) -> bool:\n        return True\n\ndef login() -> bool:\n    return False\n",
        );

        let target = QueryTarget {
            path: PathBuf::from("src/auth.py"),
            start_line: None,
            end_line: None,
            symbol: Some("login".into()),
            query_kind: QueryKind::Symbol,
        };

        let resolved = resolve_target(&target, &temp.path)
            .expect("python top-level symbol should resolve deterministically");
        assert_eq!(
            resolved,
            ResolvedTarget {
                path: PathBuf::from("src/auth.py"),
                start_line: 5,
                end_line: 6,
                query_kind: QueryKind::Symbol,
                symbol: Some("login".into()),
            }
        );
    }

    #[test]
    fn rejects_ambiguous_python_methods_at_same_depth() {
        let temp = TempSourceDir::new();
        temp.write_file(
            "src/auth.py",
            "class Primary:\n    def login(self) -> bool:\n        return True\n\nclass Backup:\n    def login(self) -> bool:\n        return False\n",
        );

        let target = QueryTarget {
            path: PathBuf::from("src/auth.py"),
            start_line: None,
            end_line: None,
            symbol: Some("login".into()),
            query_kind: QueryKind::Symbol,
        };

        let error = resolve_target(&target, &temp.path)
            .expect_err("equally nested python matches should stay ambiguous");
        let message = error.to_string();
        assert!(message.contains("symbol 'login' is ambiguous"));
        assert!(message.contains("src/auth.py:2-3"));
        assert!(message.contains("src/auth.py:6-7"));
    }

    #[test]
    fn lists_python_symbols_from_source() {
        let source = "class AuthService:\n    def login(self) -> bool:\n        return True\n\n@audit_auth\ndef authenticate(token: str) -> bool:\n    return True\n";
        let symbols = list_all_symbols(SupportedLanguage::Python, source)
            .expect("python symbols should list");

        assert_eq!(
            symbols,
            vec![
                ("AuthService".to_string(), 1, 3),
                ("login".to_string(), 2, 3),
                ("authenticate".to_string(), 5, 7),
            ]
        );
    }

    #[test]
    fn lists_go_symbols_from_source() {
        let source = "package main\n\ntype AuthService struct{}\n\nfunc Authenticate(token string) bool {\n    return token != \"\"\n}\n";
        let symbols = list_all_symbols(SupportedLanguage::Go, source)
            .expect("go symbols should list");

        assert_eq!(
            symbols,
            vec![
                ("AuthService".to_string(), 3, 3),
                ("Authenticate".to_string(), 5, 7),
            ]
        );
    }

    #[test]
    fn lists_java_symbols_from_source() {
        let source = "class AuthService {\n    AuthService() {}\n\n    boolean login() {\n        return true;\n    }\n}\n";
        let symbols = list_all_symbols(SupportedLanguage::Java, source)
            .expect("java symbols should list");

        assert_eq!(
            symbols,
            vec![
                ("AuthService".to_string(), 1, 7),
                ("login".to_string(), 4, 6),
            ]
        );
    }
}
