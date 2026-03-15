mod finder;
mod languages;

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub use finder::{
    list_all_symbols, list_symbol_definitions, resolve_target, ResolvedTarget, SymbolDefinition,
};
pub use languages::SupportedLanguage;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueryKind {
    Line,
    Range,
    Symbol,
    QualifiedSymbol,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueryTarget {
    pub path: PathBuf,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub symbol: Option<String>,
    pub query_kind: QueryKind,
}

impl QueryTarget {
    pub fn line_range(&self) -> Option<(u32, u32)> {
        match (self.start_line, self.end_line) {
            (Some(start_line), Some(end_line)) => Some((start_line, end_line)),
            _ => None,
        }
    }
}

pub fn detect_language(path: &Path) -> Result<SupportedLanguage> {
    SupportedLanguage::detect(path)
}

pub fn parse_target(target: &str, lines: Option<&str>) -> Result<QueryTarget> {
    match lines {
        Some(lines) => parse_range_target(target, lines),
        None => parse_target_without_lines(target),
    }
}

fn parse_target_without_lines(input: &str) -> Result<QueryTarget> {
    let (path, specifier) = if let Some((path, specifier)) = input.split_once(":") {
        if path.contains('.') && !path.contains("::") {
            (path, specifier)
        } else {
            input.rsplit_once(':').ok_or_else(|| {
                anyhow!(
                    "target must use <file>:<line>, <file>:<symbol>, or <file> --lines <start:end>"
                )
            })?
        }
    } else {
        return Err(anyhow!(
            "target must use <file>:<line>, <file>:<symbol>, or <file> --lines <start:end>"
        ));
    };

    if path.is_empty() {
        bail!("target path cannot be empty");
    }

    if let Ok(line_number) = specifier.parse::<u32>() {
        return build_line_target(path, line_number, "line");
    }

    if specifier.is_empty() {
        bail!("target specifier cannot be empty");
    }

    let query_kind = if specifier.contains("::") {
        QueryKind::QualifiedSymbol
    } else {
        QueryKind::Symbol
    };

    Ok(QueryTarget {
        path: PathBuf::from(path),
        start_line: None,
        end_line: None,
        symbol: Some(specifier.to_string()),
        query_kind,
    })
}

fn build_line_target(path: &str, line_number: u32, label: &str) -> Result<QueryTarget> {
    let line = validate_line_number(line_number, label)?;

    Ok(QueryTarget {
        path: PathBuf::from(path),
        start_line: Some(line),
        end_line: Some(line),
        symbol: None,
        query_kind: QueryKind::Line,
    })
}

fn parse_range_target(path: &str, lines: &str) -> Result<QueryTarget> {
    if path.is_empty() {
        bail!("target path cannot be empty");
    }

    if path.contains(':') {
        bail!(
            "range queries use <file> --lines <start:end>; do not combine a colon specifier with --lines"
        );
    }

    let (start_text, end_text) = lines
        .split_once(':')
        .ok_or_else(|| anyhow!("--lines must use START:END syntax"))?;

    let start_line = parse_line_number(start_text, "range start")?;
    let end_line = parse_line_number(end_text, "range end")?;

    if end_line < start_line {
        bail!("range end must be greater than or equal to range start");
    }

    Ok(QueryTarget {
        path: PathBuf::from(path),
        start_line: Some(start_line),
        end_line: Some(end_line),
        symbol: None,
        query_kind: QueryKind::Range,
    })
}

fn parse_line_number(input: &str, label: &str) -> Result<u32> {
    let line = input
        .parse::<u32>()
        .map_err(|_| anyhow!("invalid {label}: expected a positive integer"))?;

    validate_line_number(line, label)
}

fn validate_line_number(line: u32, label: &str) -> Result<u32> {
    if line == 0 {
        bail!("invalid {label}: line numbers are 1-based");
    }

    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::{detect_language, parse_target, QueryKind, QueryTarget, SupportedLanguage};
    use std::path::{Path, PathBuf};

    #[test]
    fn parses_file_colon_line_target() {
        let target = parse_target("src/lib.rs:42", None).expect("line target should parse");
        assert_eq!(
            target,
            QueryTarget {
                path: PathBuf::from("src/lib.rs"),
                start_line: Some(42),
                end_line: Some(42),
                symbol: None,
                query_kind: QueryKind::Line,
            }
        );
    }

    #[test]
    fn parses_lines_override_target() {
        let target = parse_target("src/lib.rs", Some("80:120")).expect("range target should parse");
        assert_eq!(
            target,
            QueryTarget {
                path: PathBuf::from("src/lib.rs"),
                start_line: Some(80),
                end_line: Some(120),
                symbol: None,
                query_kind: QueryKind::Range,
            }
        );
    }

    #[test]
    fn parses_file_colon_symbol_target() {
        let target =
            parse_target("src/lib.rs:authenticate", None).expect("symbol target should parse");
        assert_eq!(
            target,
            QueryTarget {
                path: PathBuf::from("src/lib.rs"),
                start_line: None,
                end_line: None,
                symbol: Some("authenticate".to_string()),
                query_kind: QueryKind::Symbol,
            }
        );
    }

    #[test]
    fn parses_qualified_symbol_target() {
        let target = parse_target("src/lib.rs:AuthService::login", None)
            .expect("qualified symbol target should parse");
        assert_eq!(
            target,
            QueryTarget {
                path: PathBuf::from("src/lib.rs"),
                start_line: None,
                end_line: None,
                symbol: Some("AuthService::login".to_string()),
                query_kind: QueryKind::QualifiedSymbol,
            }
        );
    }

    #[test]
    fn rejects_reversed_range() {
        let error =
            parse_target("src/lib.rs", Some("45:40")).expect_err("reversed range should fail");
        assert!(error
            .to_string()
            .contains("range end must be greater than or equal to range start"));
    }

    #[test]
    fn rejects_zero_line_number() {
        let error = parse_target("src/lib.rs:0", None).expect_err("zero line should fail");
        assert!(error.to_string().contains("line numbers are 1-based"));
    }

    #[test]
    fn rejects_mixing_colon_target_with_lines_override() {
        let error = parse_target("src/lib.rs:authenticate", Some("10:20"))
            .expect_err("mixed target forms should fail");
        assert!(error
            .to_string()
            .contains("do not combine a colon specifier with --lines"));
    }

    #[test]
    fn detects_language_from_extension() {
        assert_eq!(
            detect_language(Path::new("src/lib.rs")).unwrap(),
            SupportedLanguage::Rust
        );
        assert_eq!(
            detect_language(Path::new("src/file.go")).unwrap(),
            SupportedLanguage::Go
        );
        assert_eq!(
            detect_language(Path::new("src/file.ts")).unwrap(),
            SupportedLanguage::TypeScript
        );
        assert_eq!(
            detect_language(Path::new("src/Main.java")).unwrap(),
            SupportedLanguage::Java
        );
        assert_eq!(
            detect_language(Path::new("src/file.py")).unwrap(),
            SupportedLanguage::Python
        );
    }
}
