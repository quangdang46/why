use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const PRE_COMMIT: &str = "pre-commit";
const PRE_PUSH: &str = "pre-push";
const MANAGED_MARKER: &str = "# why-managed-hook";
const BACKUP_SUFFIX: &str = ".why-backup";

pub fn install(repo_root: &Path, warn_only: bool) -> Result<()> {
    let hooks_dir = hooks_dir(repo_root)?;
    fs::create_dir_all(&hooks_dir)
        .with_context(|| format!("failed to create hooks dir {}", hooks_dir.display()))?;

    install_hook(&hooks_dir, PRE_COMMIT, &render_pre_commit_hook(warn_only))?;
    install_hook(&hooks_dir, PRE_PUSH, &render_pre_push_hook(warn_only))?;
    Ok(())
}

pub fn uninstall(repo_root: &Path) -> Result<()> {
    let hooks_dir = hooks_dir(repo_root)?;
    uninstall_hook(&hooks_dir, PRE_COMMIT)?;
    uninstall_hook(&hooks_dir, PRE_PUSH)?;
    Ok(())
}

fn hooks_dir(repo_root: &Path) -> Result<PathBuf> {
    let git_dir = repo_root.join(".git");
    if !git_dir.is_dir() {
        bail!(
            "{} does not look like a git repository root",
            repo_root.display()
        );
    }
    Ok(git_dir.join("hooks"))
}

fn install_hook(hooks_dir: &Path, hook_name: &str, script: &str) -> Result<()> {
    let hook_path = hooks_dir.join(hook_name);
    let backup_path = backup_path(hooks_dir, hook_name);

    if hook_path.exists() {
        let existing = fs::read_to_string(&hook_path)
            .with_context(|| format!("failed to read existing hook {}", hook_path.display()))?;
        if !is_managed_hook(&existing) && !backup_path.exists() {
            fs::copy(&hook_path, &backup_path).with_context(|| {
                format!(
                    "failed to back up hook {} to {}",
                    hook_path.display(),
                    backup_path.display()
                )
            })?;
        }
    }

    fs::write(&hook_path, script)
        .with_context(|| format!("failed to write hook {}", hook_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = fs::Permissions::from_mode(0o755);
        fs::set_permissions(&hook_path, permissions)
            .with_context(|| format!("failed to chmod hook {}", hook_path.display()))?;
    }

    Ok(())
}

fn uninstall_hook(hooks_dir: &Path, hook_name: &str) -> Result<()> {
    let hook_path = hooks_dir.join(hook_name);
    let backup_path = backup_path(hooks_dir, hook_name);

    if backup_path.exists() {
        fs::rename(&backup_path, &hook_path).with_context(|| {
            format!(
                "failed to restore backup {} to {}",
                backup_path.display(),
                hook_path.display()
            )
        })?;
        return Ok(());
    }

    if hook_path.exists() {
        let contents = fs::read_to_string(&hook_path)
            .with_context(|| format!("failed to read hook {}", hook_path.display()))?;
        if is_managed_hook(&contents) {
            fs::remove_file(&hook_path)
                .with_context(|| format!("failed to remove hook {}", hook_path.display()))?;
        }
    }

    Ok(())
}

fn backup_path(hooks_dir: &Path, hook_name: &str) -> PathBuf {
    hooks_dir.join(format!("{hook_name}{BACKUP_SUFFIX}"))
}

fn is_managed_hook(contents: &str) -> bool {
    contents.contains(MANAGED_MARKER)
}

fn render_pre_commit_hook(warn_only: bool) -> String {
    render_hook(
        PRE_COMMIT,
        warn_only,
        "git diff --cached --name-only --diff-filter=ACMR",
        "pre-commit",
    )
}

fn render_pre_push_hook(warn_only: bool) -> String {
    render_hook(
        PRE_PUSH,
        warn_only,
        "git diff --name-only HEAD~1..HEAD",
        "pre-push",
    )
}

fn render_hook(hook_name: &str, warn_only: bool, file_command: &str, label: &str) -> String {
    let mode_logic = if warn_only {
        "echo \"why: HIGH risk changes detected during {label}; continuing because --warn-only is enabled\"\nexit 0"
            .replace("{label}", label)
    } else {
        "printf \"why: HIGH risk changes detected during {label}. Continue? [y/N] \"\nread answer\ncase \"$answer\" in\n  y|Y|yes|YES) exit 0 ;;\n  *) exit 1 ;;\nesac"
            .replace("{label}", label)
    };

    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
{MANAGED_MARKER}
# installed by why for {hook_name}

if ! command -v why >/dev/null 2>&1; then
  exit 0
fi

files=$({file_command} || true)
if [ -z "${{files}}" ]; then
  exit 0
fi

high_risk=0
while IFS= read -r file; do
  [ -n "$file" ] || continue
  [ -f "$file" ] || continue

  line_count=$(awk 'END {{print NR}}' "$file")
  if [ -z "${{line_count}}" ] || [ "${{line_count}}" = "0" ]; then
    continue
  fi

  risk_output=$(why "$file" --lines "1:${{line_count}}" --no-llm --json 2>/dev/null || true)
  if printf '%s' "$risk_output" | grep -q '"risk_level"[[:space:]]*:[[:space:]]*"HIGH"'; then
    high_risk=1
    break
  fi
done <<< "$files"

if [ "$high_risk" -eq 1 ]; then
  {mode_logic}
fi

exit 0
"#
    )
}

#[cfg(test)]
mod tests {
    use super::{install, uninstall};
    use anyhow::Result;
    use std::fs;
    use tempfile::tempdir;

    fn setup_repo() -> Result<tempfile::TempDir> {
        let dir = tempdir()?;
        fs::create_dir_all(dir.path().join(".git/hooks"))?;
        Ok(dir)
    }

    #[test]
    fn install_writes_managed_hooks() -> Result<()> {
        let repo = setup_repo()?;
        install(repo.path(), true)?;

        let pre_commit = fs::read_to_string(repo.path().join(".git/hooks/pre-commit"))?;
        let pre_push = fs::read_to_string(repo.path().join(".git/hooks/pre-push"))?;
        assert!(pre_commit.contains("# why-managed-hook"));
        assert!(pre_push.contains("# why-managed-hook"));
        assert!(pre_commit.contains("--warn-only is enabled"));
        Ok(())
    }

    #[test]
    fn install_backs_up_existing_unmanaged_hook() -> Result<()> {
        let repo = setup_repo()?;
        let hook_path = repo.path().join(".git/hooks/pre-commit");
        fs::write(&hook_path, "#!/bin/sh\necho old\n")?;

        install(repo.path(), false)?;

        let backup = fs::read_to_string(repo.path().join(".git/hooks/pre-commit.why-backup"))?;
        let installed = fs::read_to_string(&hook_path)?;
        assert!(backup.contains("echo old"));
        assert!(installed.contains("# why-managed-hook"));
        Ok(())
    }

    #[test]
    fn uninstall_restores_backup_when_present() -> Result<()> {
        let repo = setup_repo()?;
        let hook_path = repo.path().join(".git/hooks/pre-commit");
        fs::write(&hook_path, "#!/bin/sh\necho old\n")?;

        install(repo.path(), false)?;
        uninstall(repo.path())?;

        let restored = fs::read_to_string(&hook_path)?;
        assert!(restored.contains("echo old"));
        assert!(
            !repo
                .path()
                .join(".git/hooks/pre-commit.why-backup")
                .exists()
        );
        Ok(())
    }

    #[test]
    fn uninstall_removes_managed_hook_without_backup() -> Result<()> {
        let repo = setup_repo()?;
        install(repo.path(), true)?;

        uninstall(repo.path())?;

        assert!(!repo.path().join(".git/hooks/pre-commit").exists());
        assert!(!repo.path().join(".git/hooks/pre-push").exists());
        Ok(())
    }
}
