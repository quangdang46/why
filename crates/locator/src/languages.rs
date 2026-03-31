use anyhow::{Result, anyhow, bail};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use tree_sitter::{Language, Query};

static SYMBOL_QUERY_CACHE: LazyLock<Mutex<HashMap<SupportedLanguage, Query>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportedLanguage {
    Rust,
    Go,
    JavaScript,
    TypeScript,
    Java,
    Python,
}

impl SupportedLanguage {
    pub fn detect(path: &Path) -> Result<Self> {
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .ok_or_else(|| anyhow!("cannot detect language for {}", path.display()))?;

        match extension {
            "rs" => Ok(Self::Rust),
            "go" => Ok(Self::Go),
            "js" => Ok(Self::JavaScript),
            "ts" | "tsx" => Ok(Self::TypeScript),
            "java" => Ok(Self::Java),
            "py" => Ok(Self::Python),
            _ => bail!(
                "unsupported file extension .{} for {}; supported extensions: .rs, .go, .js, .ts, .tsx, .java, .py",
                extension,
                path.display()
            ),
        }
    }

    pub fn grammar_name(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Go => "go",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Java => "java",
            Self::Python => "python",
        }
    }

    pub fn symbol_query(self) -> &'static str {
        match self {
            Self::Rust => {
                r#"
(function_item
  name: (identifier) @symbol.name) @symbol.definition

(struct_item
  name: (type_identifier) @symbol.name) @symbol.definition

(enum_item
  name: (type_identifier) @symbol.name) @symbol.definition

(trait_item
  name: (type_identifier) @symbol.name) @symbol.definition

(impl_item
  body: (declaration_list
    (function_item
      name: (identifier) @symbol.name) @symbol.definition))
"#
            }
            Self::Go => {
                r#"
(function_declaration
  name: (identifier) @symbol.name) @symbol.definition

(method_declaration
  name: (field_identifier) @symbol.name) @symbol.definition

(type_declaration
  (type_spec
    name: (type_identifier) @symbol.name)) @symbol.definition
"#
            }
            Self::JavaScript => {
                r#"
(function_declaration
  name: (identifier) @symbol.name) @symbol.definition

(lexical_declaration
  (variable_declarator
    name: (identifier) @symbol.name
    value: (arrow_function)) @symbol.definition)

(variable_declaration
  (variable_declarator
    name: (identifier) @symbol.name
    value: (arrow_function)) @symbol.definition)

(method_definition
  name: (property_identifier) @symbol.name) @symbol.definition

(class_declaration
  name: (identifier) @symbol.name) @symbol.definition
"#
            }
            Self::TypeScript => {
                r#"
(function_declaration
  name: (identifier) @symbol.name) @symbol.definition

(export_statement
  declaration: (function_declaration
    name: (identifier) @symbol.name) @symbol.definition)

(lexical_declaration
  (variable_declarator
    name: (identifier) @symbol.name
    value: (arrow_function)) @symbol.definition)

(export_statement
  declaration: (lexical_declaration
    (variable_declarator
      name: (identifier) @symbol.name
      value: (arrow_function)) @symbol.definition))

(method_definition
  name: (property_identifier) @symbol.name) @symbol.definition

(class_declaration
  name: (type_identifier) @symbol.name) @symbol.definition

(interface_declaration
  name: (type_identifier) @symbol.name) @symbol.definition
"#
            }
            Self::Java => {
                r#"
(class_declaration
  name: (identifier) @symbol.name) @symbol.definition

(interface_declaration
  name: (identifier) @symbol.name) @symbol.definition

(enum_declaration
  name: (identifier) @symbol.name) @symbol.definition

(record_declaration
  name: (identifier) @symbol.name) @symbol.definition

(method_declaration
  name: (identifier) @symbol.name) @symbol.definition

(constructor_declaration
  name: (identifier) @symbol.name) @symbol.definition
"#
            }
            Self::Python => {
                r#"
(function_definition
  name: (identifier) @symbol.name) @symbol.definition

(decorated_definition
  definition: (function_definition
    name: (identifier) @symbol.name)) @symbol.definition

(class_definition
  name: (identifier) @symbol.name) @symbol.definition
"#
            }
        }
    }

    pub fn tree_sitter_language(self) -> Language {
        match self {
            Self::Rust => tree_sitter_rust::language(),
            Self::Go => tree_sitter_go::language(),
            Self::JavaScript => tree_sitter_javascript::language(),
            Self::TypeScript => tree_sitter_typescript::language_typescript(),
            Self::Java => tree_sitter_java::language(),
            Self::Python => tree_sitter_python::language(),
        }
    }

    pub fn with_symbol_query<R>(self, f: impl FnOnce(&Query) -> R) -> Result<R> {
        let mut cache = SYMBOL_QUERY_CACHE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let std::collections::hash_map::Entry::Vacant(entry) = cache.entry(self) {
            let query = Query::new(&self.tree_sitter_language(), self.symbol_query())?;
            entry.insert(query);
        }
        let query = cache
            .get(&self)
            .ok_or_else(|| anyhow!("symbol query cache miss for {}", self.grammar_name()))?;
        Ok(f(query))
    }
}

#[cfg(test)]
mod tests {
    use super::SupportedLanguage;
    use std::path::Path;

    #[test]
    fn detects_supported_languages_from_extension() {
        assert_eq!(
            SupportedLanguage::detect(Path::new("src/lib.rs")).unwrap(),
            SupportedLanguage::Rust
        );
        assert_eq!(
            SupportedLanguage::detect(Path::new("src/main.go")).unwrap(),
            SupportedLanguage::Go
        );
        assert_eq!(
            SupportedLanguage::detect(Path::new("src/app.ts")).unwrap(),
            SupportedLanguage::TypeScript
        );
        assert_eq!(
            SupportedLanguage::detect(Path::new("src/component.tsx")).unwrap(),
            SupportedLanguage::TypeScript
        );
        assert_eq!(
            SupportedLanguage::detect(Path::new("src/index.js")).unwrap(),
            SupportedLanguage::JavaScript
        );
        assert_eq!(
            SupportedLanguage::detect(Path::new("src/Main.java")).unwrap(),
            SupportedLanguage::Java
        );
        assert_eq!(
            SupportedLanguage::detect(Path::new("src/main.py")).unwrap(),
            SupportedLanguage::Python
        );
    }

    #[test]
    fn rejects_unsupported_language_extensions() {
        let error = SupportedLanguage::detect(Path::new("src/main.kt"))
            .expect_err("unsupported extension should fail");
        assert!(error.to_string().contains("unsupported file extension"));
    }

    #[test]
    fn exposes_non_empty_symbol_queries() {
        for language in [
            SupportedLanguage::Rust,
            SupportedLanguage::Go,
            SupportedLanguage::JavaScript,
            SupportedLanguage::TypeScript,
            SupportedLanguage::Java,
            SupportedLanguage::Python,
        ] {
            assert!(!language.grammar_name().is_empty());
            assert!(!language.symbol_query().trim().is_empty());
        }
    }

    #[test]
    fn loads_tree_sitter_queries_for_each_supported_language() {
        for language in [
            SupportedLanguage::Rust,
            SupportedLanguage::Go,
            SupportedLanguage::JavaScript,
            SupportedLanguage::TypeScript,
            SupportedLanguage::Java,
            SupportedLanguage::Python,
        ] {
            language
                .with_symbol_query(|query| {
                    assert!(query.capture_names().contains(&"symbol.name"));
                    assert!(query.capture_names().contains(&"symbol.definition"));
                })
                .expect("query should compile");
        }
    }
}
