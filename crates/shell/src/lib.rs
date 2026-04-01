//! Interactive shell support.

use anyhow::{Context, Result, anyhow};
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{Config, Editor, Helper};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::{self, BufRead, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use walkdir::WalkDir;
use why_locator::{SupportedLanguage, list_all_symbols};

pub fn run() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let mut index = CompletionIndex::build(&cwd)?;
    print_startup(&index);

    if !io::stdin().is_terminal() {
        return run_non_interactive(&cwd, &mut index);
    }

    let config = Config::builder().auto_add_history(true).build();
    let mut editor = Editor::<ShellHelper, DefaultHistory>::with_config(config)?;
    let history_path = history_path()?;
    let _ = editor.load_history(&history_path);
    editor.set_helper(Some(ShellHelper {
        index: index.clone(),
    }));

    loop {
        match editor.readline("why> ") {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed == "exit" || trimmed == "quit" {
                    break;
                }
                if trimmed == "help" {
                    print_help();
                    continue;
                }
                if trimmed == "reload" {
                    index = CompletionIndex::build(&cwd)?;
                    if let Some(helper) = editor.helper_mut() {
                        helper.index = index.clone();
                    }
                    println!(
                        "reloaded {} symbols across {} files.",
                        index.symbol_count, index.file_count
                    );
                    continue;
                }

                if let Err(error) = run_shell_command(&cwd, trimmed) {
                    eprintln!("why shell: {error}");
                }
            }
            Err(ReadlineError::Interrupted) => {
                println!("^C (type 'help' for commands, Ctrl-D to exit)");
            }
            Err(ReadlineError::Eof) => {
                println!();
                break;
            }
            Err(error) => return Err(anyhow!(error)).context("shell input failed"),
        }
    }

    let _ = editor.save_history(&history_path);
    Ok(())
}

fn run_non_interactive(cwd: &Path, index: &mut CompletionIndex) -> Result<()> {
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == "exit" || trimmed == "quit" {
            break;
        }
        if trimmed == "help" {
            print_help();
            continue;
        }
        if trimmed == "reload" {
            *index = CompletionIndex::build(cwd)?;
            println!(
                "reloaded {} symbols across {} files.",
                index.symbol_count, index.file_count
            );
            continue;
        }

        if let Err(error) = run_shell_command(cwd, trimmed) {
            eprintln!("why shell: {error}");
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Default)]
struct CompletionIndex {
    files: Vec<String>,
    symbols_by_file: HashMap<String, Vec<String>>,
    file_count: usize,
    symbol_count: usize,
}

impl CompletionIndex {
    fn build(repo_root: &Path) -> Result<Self> {
        let mut files = Vec::new();
        let mut symbols_by_file = HashMap::new();
        let mut symbol_count = 0usize;

        for entry in WalkDir::new(repo_root)
            .into_iter()
            .filter_entry(|entry| !is_ignored(entry.path()))
            .filter_map(|entry| entry.ok())
        {
            let path = entry.path();
            if !entry.file_type().is_file() {
                continue;
            }

            let relative = match path.strip_prefix(repo_root) {
                Ok(relative) => relative,
                Err(_) => continue,
            };

            if SupportedLanguage::detect(relative).is_err() {
                continue;
            }

            let relative_text = normalize_path(relative);
            files.push(relative_text.clone());

            let source = match fs::read_to_string(path) {
                Ok(source) => source,
                Err(_) => continue,
            };
            let language = match SupportedLanguage::detect(relative) {
                Ok(language) => language,
                Err(_) => continue,
            };
            let mut unique = BTreeSet::new();
            for (name, _, _) in list_all_symbols(language, &source).unwrap_or_default() {
                unique.insert(name);
            }
            symbol_count += unique.len();
            symbols_by_file.insert(relative_text, unique.into_iter().collect());
        }

        files.sort();
        let file_count = files.len();
        Ok(Self {
            files,
            symbols_by_file,
            file_count,
            symbol_count,
        })
    }

    fn complete(&self, input: &str) -> Vec<String> {
        let trimmed = input.trim_start();
        if trimmed.is_empty() {
            return self.files.iter().take(20).cloned().collect();
        }

        if let Some(prefix) = trimmed.strip_prefix(":") {
            return self
                .command_suggestions(prefix)
                .into_iter()
                .map(|command| format!(":{command}"))
                .collect();
        }

        if matches!(trimmed, "help" | "reload" | "exit" | "quit") {
            return vec![trimmed.to_string()];
        }

        if is_subcommand_prefix(trimmed) {
            return ["hotspots", "health", "ghost"]
                .into_iter()
                .filter(|candidate| candidate.starts_with(trimmed))
                .map(str::to_string)
                .collect();
        }

        if let Some((file, symbol_prefix)) = trimmed.split_once(':') {
            let file = file.trim();
            let mut matches = Vec::new();
            if let Some(symbols) = self.symbols_by_file.get(file) {
                for symbol in symbols {
                    if symbol.starts_with(symbol_prefix) {
                        matches.push(format!("{file}:{symbol}"));
                    }
                }
            }
            return matches;
        }

        self.files
            .iter()
            .filter(|file| file.starts_with(trimmed))
            .take(50)
            .cloned()
            .collect()
    }

    fn command_suggestions(&self, prefix: &str) -> Vec<String> {
        ["help", "reload", "exit", "quit"]
            .into_iter()
            .filter(|candidate| candidate.starts_with(prefix))
            .map(str::to_string)
            .collect()
    }
}

#[derive(Clone)]
struct ShellHelper {
    index: CompletionIndex,
}

impl Helper for ShellHelper {}
impl Hinter for ShellHelper {
    type Hint = String;
}
impl Highlighter for ShellHelper {}
impl Validator for ShellHelper {}

impl Completer for ShellHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let prefix = &line[..pos];
        let start = prefix
            .rfind(char::is_whitespace)
            .map(|idx| idx + 1)
            .unwrap_or(0);
        let token = &prefix[start..];
        let candidates = self
            .index
            .complete(token)
            .into_iter()
            .map(|candidate| Pair {
                display: candidate.clone(),
                replacement: candidate,
            })
            .collect();
        Ok((start, candidates))
    }
}

fn run_shell_command(cwd: &Path, line: &str) -> Result<()> {
    let mut args = split_shell_words(line);
    if args.is_empty() {
        return Ok(());
    }

    if !matches!(
        args.first().map(String::as_str),
        Some("hotspots" | "health" | "ghost")
    ) && !args.iter().any(|arg| arg == "--no-llm")
    {
        args.push("--no-llm".to_string());
    }

    let exe = std::env::current_exe().context("failed to locate current why binary")?;
    let output = Command::new(exe)
        .args(&args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .context("failed to execute nested why command")?;

    if !output.stdout.is_empty() {
        print!("{}", String::from_utf8_lossy(&output.stdout));
    }
    if !output.stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
    }

    Ok(())
}

fn split_shell_words(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

fn print_startup(index: &CompletionIndex) {
    println!("why shell — loading repository index...");
    println!(
        "  {} symbols, {} files indexed.",
        index.symbol_count, index.file_count
    );
    println!("  Type a target (e.g. src/auth.rs:authenticate) or 'help'. Ctrl-D to exit.");
    println!();
}

fn print_help() {
    println!("Shell commands:");
    println!("  <target> [flags]   Run a normal why query (defaults to --no-llm in shell)");
    println!("  hotspots [flags]   Run the hotspots scanner");
    println!("  health [flags]     Run the health scanner");
    println!("  ghost [flags]      Run the ghost scanner");
    println!("  reload             Rebuild the completion index");
    println!("  help               Show this help");
    println!("  exit | quit        Leave the shell");
}

fn history_path() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("home directory is not available"))?;
    Ok(home.join(".why_history"))
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn is_ignored(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component.as_os_str().to_str(),
            Some(".git" | ".why" | "target" | "node_modules")
        )
    })
}

fn is_subcommand_prefix(text: &str) -> bool {
    ["hotspots", "health", "ghost"]
        .into_iter()
        .any(|candidate| candidate.starts_with(text))
}

#[cfg(test)]
mod tests {
    use super::{CompletionIndex, is_ignored, split_shell_words};
    use std::collections::HashMap;
    use std::path::Path;

    #[test]
    fn shell_word_split_is_whitespace_based() {
        assert_eq!(
            split_shell_words("src/lib.rs:42 --json --no-llm"),
            vec!["src/lib.rs:42", "--json", "--no-llm"]
        );
    }

    #[test]
    fn completion_index_suggests_symbols_for_file_prefix() {
        let index = CompletionIndex {
            files: vec!["src/auth.rs".into()],
            symbols_by_file: HashMap::from([(
                "src/auth.rs".into(),
                vec!["authenticate".into(), "authorize".into()],
            )]),
            file_count: 1,
            symbol_count: 2,
        };

        assert_eq!(
            index.complete("src/auth.rs:au"),
            vec![
                "src/auth.rs:authenticate".to_string(),
                "src/auth.rs:authorize".to_string()
            ]
        );
    }

    #[test]
    fn ignored_paths_skip_repo_metadata_and_build_outputs() {
        assert!(is_ignored(Path::new(".git/hooks/pre-commit")));
        assert!(is_ignored(Path::new("target/debug/why")));
        assert!(!is_ignored(Path::new("src/main.rs")));
    }
}
