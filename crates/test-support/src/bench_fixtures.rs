use std::fs;
use std::path::Path;
use std::process::Command;

pub fn create_large_history_repo(repo_root: &Path, commit_count: usize) -> Result<(), String> {
    if commit_count == 0 {
        return Err("commit_count must be greater than zero".into());
    }

    init_repo(repo_root)?;
    fs::create_dir_all(repo_root.join("src")).map_err(|err| err.to_string())?;

    for index in 0..commit_count {
        let history_path = repo_root.join("src").join("history.rs");
        fs::write(&history_path, render_history_source(index)).map_err(|err| err.to_string())?;

        if index % 5 == 0 {
            let support_path = repo_root.join("src").join("support.rs");
            fs::write(&support_path, render_support_source(index))
                .map_err(|err| err.to_string())?;
        }

        git(repo_root, ["add", "src/history.rs", "src/support.rs"])?;
        let message = format!("history: evolve authentication guard v{}", index + 1);
        git(repo_root, ["commit", "-m", message.as_str()])?;
    }

    Ok(())
}

pub fn create_scanner_scale_repo(
    repo_root: &Path,
    file_count: usize,
    commit_count: usize,
) -> Result<(), String> {
    if file_count == 0 {
        return Err("file_count must be greater than zero".into());
    }
    if commit_count == 0 {
        return Err("commit_count must be greater than zero".into());
    }

    init_repo(repo_root)?;
    fs::create_dir_all(repo_root.join("src")).map_err(|err| err.to_string())?;

    for file_index in 0..file_count {
        let path = repo_root
            .join("src")
            .join(format!("module_{file_index}.rs"));
        fs::write(&path, render_scanner_file(file_index, 0)).map_err(|err| err.to_string())?;
    }
    git(repo_root, ["add", "src"])?;
    git(
        repo_root,
        ["commit", "-m", "feat: seed scanner scale fixture"],
    )?;

    for commit_index in 1..commit_count {
        let hotspot_path = repo_root.join("src").join("module_0.rs");
        fs::write(&hotspot_path, render_scanner_file(0, commit_index))
            .map_err(|err| err.to_string())?;

        if file_count > 1 && commit_index % 2 == 0 {
            let path = repo_root.join("src").join("module_1.rs");
            fs::write(&path, render_scanner_file(1, commit_index))
                .map_err(|err| err.to_string())?;
        }

        if file_count > 2 && commit_index % 5 == 0 {
            let path = repo_root.join("src").join("module_2.rs");
            fs::write(&path, render_scanner_file(2, commit_index))
                .map_err(|err| err.to_string())?;
        }

        git(repo_root, ["add", "src"])?;
        let message = format!("scan: churn hotspot commit {commit_index}");
        git(repo_root, ["commit", "-m", message.as_str()])?;
    }

    Ok(())
}

fn init_repo(repo_root: &Path) -> Result<(), String> {
    fs::create_dir_all(repo_root).map_err(|err| err.to_string())?;
    git(repo_root, ["init", "-b", "main"]).or_else(|_| git(repo_root, ["init"]))?;
    git(repo_root, ["config", "user.email", "bench@example.com"])?;
    git(repo_root, ["config", "user.name", "Benchmark Fixture"])?;
    Ok(())
}

fn git<I, S>(repo_root: &Path, args: I) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args = args
        .into_iter()
        .map(|arg| arg.as_ref().to_string())
        .collect::<Vec<_>>();
    let output = Command::new("git")
        .args(&args)
        .current_dir(repo_root)
        .output()
        .map_err(|err| err.to_string())?;

    if output.status.success() {
        return Ok(());
    }

    Err(format!(
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    ))
}

fn render_history_source(index: usize) -> String {
    let hardening = if index % 11 == 0 {
        "    // security: rotate validation logic after incident review\n"
    } else {
        ""
    };
    let compatibility = if index % 7 == 0 {
        "    // compatibility: preserve legacy token fallback during staged migration\n"
    } else {
        ""
    };

    format!(
        "pub fn evaluate_request(token: &str, account_age_days: u64) -> bool {{\n{hardening}{compatibility}    let version_marker = {};\n    let minimum_age = {};\n    token.len() + version_marker > minimum_age as usize\n        && account_age_days >= minimum_age\n}}\n",
        index + 1,
        (index % 30) + 1
    )
}

fn render_support_source(index: usize) -> String {
    format!(
        "pub fn support_window_days() -> u64 {{\n    {}\n}}\n",
        30 + (index % 10)
    )
}

fn render_scanner_file(file_index: usize, commit_index: usize) -> String {
    let marker = if file_index == 0 && commit_index % 9 == 0 {
        "// TODO(2024-01-01): retire this migration guard after the final rollout\n"
    } else if file_index == 1 && commit_index % 6 == 0 {
        "// HACK: keep duplicate compatibility path until v3 traffic is gone\n"
    } else {
        ""
    };

    format!(
        "{marker}pub fn module_{file_index}_checkpoint() -> usize {{\n    {}\n}}\n",
        commit_index + file_index + 1
    )
}

#[cfg(test)]
mod tests {
    use super::{create_large_history_repo, create_scanner_scale_repo};
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use tempfile::tempdir;

    #[test]
    fn large_history_repo_creates_expected_commit_count_and_source_files() {
        let dir = tempdir().expect("tempdir");
        create_large_history_repo(dir.path(), 12).expect("large history fixture should build");

        assert!(dir.path().join("src/history.rs").is_file());
        assert!(dir.path().join("src/support.rs").is_file());
        assert_eq!(
            git_stdout(dir.path(), ["rev-list", "--count", "HEAD"]),
            "12"
        );

        let history = fs::read_to_string(dir.path().join("src/history.rs")).expect("history file");
        assert!(history.contains("evaluate_request"));
    }

    #[test]
    fn scanner_scale_repo_creates_seed_and_churn_history() {
        let dir = tempdir().expect("tempdir");
        create_scanner_scale_repo(dir.path(), 3, 8).expect("scanner scale fixture should build");

        assert!(dir.path().join("src/module_0.rs").is_file());
        assert!(dir.path().join("src/module_1.rs").is_file());
        assert!(dir.path().join("src/module_2.rs").is_file());
        assert_eq!(git_stdout(dir.path(), ["rev-list", "--count", "HEAD"]), "8");

        let hotspot =
            fs::read_to_string(dir.path().join("src/module_0.rs")).expect("module_0 file");
        assert!(hotspot.contains("module_0_checkpoint"));
    }

    #[test]
    fn rejects_zero_sized_inputs() {
        let dir = tempdir().expect("tempdir");
        assert!(create_large_history_repo(dir.path(), 0).is_err());
        assert!(create_scanner_scale_repo(dir.path(), 0, 5).is_err());
        assert!(create_scanner_scale_repo(dir.path(), 3, 0).is_err());
    }

    fn git_stdout<I, S>(repo_root: &Path, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let args = args
            .into_iter()
            .map(|arg| arg.as_ref().to_string())
            .collect::<Vec<_>>();
        let output = Command::new("git")
            .args(&args)
            .current_dir(repo_root)
            .output()
            .expect("git command should run");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }
}
