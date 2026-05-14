use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};

pub fn merge_base(repo: &Path, target: &str, head_ref: &str) -> Result<String> {
    let output = git_command(repo, ["merge-base", target, head_ref])?;
    Ok(output.trim().to_string())
}

pub fn resolve_ref(repo: &Path, git_ref: &str) -> Result<String> {
    let output = git_command(repo, ["rev-parse", git_ref])?;
    Ok(output.trim().to_string())
}

pub fn is_worktree_dirty(repo: &Path) -> Result<bool> {
    let output = git_command(repo, ["status", "--porcelain"])?;
    Ok(!output.trim().is_empty())
}

pub fn has_unmerged_paths(repo: &Path) -> Result<bool> {
    let output = git_command(repo, ["ls-files", "-u"])?;
    Ok(!output.trim().is_empty())
}

pub fn is_head_detached(repo: &Path) -> Result<bool> {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["symbolic-ref", "-q", "HEAD"])
        .status()
        .with_context(|| format!("failed to run git in {}", repo.display()))?;
    Ok(!status.success())
}

pub fn has_meaningful_code_diff(repo: &Path, base_ref: &str, head_ref: &str) -> Result<bool> {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "diff",
            "--quiet",
            "--ignore-cr-at-eol",
            "--ignore-space-at-eol",
            base_ref,
            head_ref,
            "--",
            "*.kt",
            "*.swift",
        ])
        .status()
        .with_context(|| format!("failed to run git in {}", repo.display()))?;

    match status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        Some(code) => Err(anyhow!("git diff failed with exit code {code}")),
        None => Err(anyhow!("git diff failed: process terminated by signal")),
    }
}

pub fn is_shallow_repository(repo: &Path) -> Result<bool> {
    let output = git_command(repo, ["rev-parse", "--is-shallow-repository"])?;
    Ok(output.trim() == "true")
}

pub fn git_changed_files_between(
    repo: &Path,
    base_ref: &str,
    head_ref: &str,
) -> Result<HashSet<String>> {
    let output = git_command(repo, ["diff", "--name-only", base_ref, head_ref])?;
    Ok(output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(normalize_git_path)
        .collect())
}

pub fn git_changed_files_worktree(repo: &Path) -> Result<HashSet<String>> {
    let output = git_command(repo, ["status", "--porcelain"])?;
    let mut changed = HashSet::new();
    for line in output
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
    {
        let Some(rest) = line.get(3..) else {
            continue;
        };
        if let Some((_, new_path)) = rest.split_once(" -> ") {
            changed.insert(normalize_git_path(new_path.trim()));
        } else {
            changed.insert(normalize_git_path(rest.trim()));
        }
    }
    Ok(changed)
}

pub(crate) fn git_toplevel_for_path(path: &Path) -> Result<Option<PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output();
    let Ok(output) = output else {
        return Ok(None);
    };
    if !output.status.success() {
        return Ok(None);
    }
    let stdout = String::from_utf8(output.stdout).context("git output was not valid UTF-8")?;
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(PathBuf::from(trimmed)))
}

pub(crate) fn git_ls_files_worktree(repo: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .output()
        .with_context(|| format!("failed to run git in {}", repo.display()))?;

    if !output.status.success() {
        return Err(anyhow!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let stdout = String::from_utf8(output.stdout).context("git output was not valid UTF-8")?;
    Ok(stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub(crate) fn git_ls_tree(repo: &Path, git_ref: &str) -> Result<Vec<String>> {
    let output = git_command(repo, ["ls-tree", "-r", "--name-only", git_ref])?;
    Ok(output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub(crate) fn git_show(repo: &Path, git_ref: &str, path: &str) -> Result<String> {
    git_command(repo, ["show", &format!("{git_ref}:{path}")])
}

pub(crate) fn normalize_git_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn git_command<const N: usize>(repo: &Path, args: [&str; N]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git in {}", repo.display()))?;

    if output.status.success() {
        String::from_utf8(output.stdout).context("git output was not valid UTF-8")
    } else {
        Err(anyhow!(
            "git command failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}
