use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use std::collections::HashSet;
use walkdir::WalkDir;

use crate::config::{Config, PathMatcher};
use crate::model::{ProjectSnapshot, SourceFile};

pub fn load_from_paths(
    kotlin_root: &Path,
    ios_root: &Path,
    config: &Config,
) -> Result<ProjectSnapshot> {
    let matcher = config.path_matcher()?;
    let kotlin_files = collect_path_files(kotlin_root, "kt", &matcher)?;
    let ios_files = collect_path_files(ios_root, "swift", &matcher)?;

    Ok(ProjectSnapshot {
        label: "workspace".to_string(),
        kotlin_files,
        ios_files,
    })
}

pub fn load_from_git(repo: &Path, git_ref: &str, config: &Config) -> Result<ProjectSnapshot> {
    load_from_git_scoped(repo, git_ref, config, None)
}

pub fn load_from_git_scoped(
    repo: &Path,
    git_ref: &str,
    config: &Config,
    changed_paths: Option<&HashSet<String>>,
) -> Result<ProjectSnapshot> {
    let matcher = config.path_matcher()?;
    let files = git_ls_tree(repo, git_ref)?;

    let kotlin_files = collect_git_files(
        repo,
        git_ref,
        &files,
        "kt",
        &matcher,
        &config.kotlin_roots,
        changed_paths,
    )?;
    let ios_files = collect_git_files(
        repo,
        git_ref,
        &files,
        "swift",
        &matcher,
        &config.ios_roots,
        changed_paths,
    )?;

    Ok(ProjectSnapshot {
        label: git_ref.to_string(),
        kotlin_files,
        ios_files,
    })
}

pub fn load_from_worktree(repo: &Path, config: &Config) -> Result<ProjectSnapshot> {
    load_from_worktree_scoped(repo, config, None)
}

pub fn load_from_worktree_scoped(
    repo: &Path,
    config: &Config,
    changed_paths: Option<&HashSet<String>>,
) -> Result<ProjectSnapshot> {
    let matcher = config.path_matcher()?;
    let files = git_ls_files_worktree(repo)?;
    let kotlin_files = collect_worktree_git_list_files(
        repo,
        &files,
        "kt",
        &matcher,
        &config.kotlin_roots,
        changed_paths,
    )?;
    let ios_files = collect_worktree_git_list_files(
        repo,
        &files,
        "swift",
        &matcher,
        &config.ios_roots,
        changed_paths,
    )?;

    Ok(ProjectSnapshot {
        label: "WORKTREE".to_string(),
        kotlin_files,
        ios_files,
    })
}

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

fn collect_path_files(
    root: &Path,
    extension: &str,
    matcher: &PathMatcher,
) -> Result<Vec<SourceFile>> {
    let mut files = Vec::new();

    for entry in WalkDir::new(root).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }

        if entry.path().extension() != Some(OsStr::new(extension)) {
            continue;
        }

        let relative = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .replace('\\', "/");

        if !matcher.is_included(&relative) {
            continue;
        }

        let path = fs::canonicalize(entry.path()).unwrap_or_else(|_| entry.path().to_path_buf());
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("failed to read source file {}", path.display()))?;

        files.push(SourceFile {
            path: path.display().to_string(),
            contents,
            snapshot: None,
        });
    }

    Ok(files)
}

fn collect_git_files(
    repo: &Path,
    git_ref: &str,
    paths: &[String],
    extension: &str,
    matcher: &PathMatcher,
    roots: &[String],
    changed_paths: Option<&HashSet<String>>,
) -> Result<Vec<SourceFile>> {
    let mut files = Vec::new();
    let mut candidates = Vec::new();

    for path in paths {
        if is_ignored_generated_path(path) {
            continue;
        }

        if Path::new(path).extension() != Some(OsStr::new(extension)) {
            continue;
        }

        if !roots.is_empty()
            && !roots
                .iter()
                .any(|root| path.starts_with(root.trim_end_matches('/')))
        {
            continue;
        }

        if !matcher.is_included(path) {
            continue;
        }
        if let Some(changed) = changed_paths {
            let normalized = normalize_git_path(path);
            if !changed.contains(&normalized) {
                continue;
            }
        }

        candidates.push(path);
    }

    eprintln!(
        "[kmpolice] snapshot={} ext=.{} candidates={}",
        git_ref,
        extension,
        candidates.len()
    );

    let started_at = Instant::now();
    for (index, path) in candidates.iter().enumerate() {
        if index > 0 && index % 200 == 0 {
            eprintln!(
                "[kmpolice] snapshot={} ext=.{} loaded {}/{} files (elapsed: {:.1}s)...",
                git_ref,
                extension,
                index,
                candidates.len(),
                started_at.elapsed().as_secs_f64()
            );
        }

        let contents = git_show(repo, git_ref, path)?;
        files.push(SourceFile {
            path: (*path).clone(),
            contents,
            snapshot: Some(git_ref.to_string()),
        });
    }

    if !candidates.is_empty() {
        eprintln!(
            "[kmpolice] snapshot={} ext=.{} loaded {}/{} files (elapsed: {:.1}s).",
            git_ref,
            extension,
            candidates.len(),
            candidates.len(),
            started_at.elapsed().as_secs_f64()
        );
    }

    Ok(files)
}

fn collect_worktree_git_list_files(
    repo: &Path,
    files: &[String],
    extension: &str,
    matcher: &PathMatcher,
    roots: &[String],
    changed_paths: Option<&HashSet<String>>,
) -> Result<Vec<SourceFile>> {
    let mut collected = Vec::new();

    for path in files {
        if is_ignored_generated_path(path) {
            continue;
        }

        if Path::new(path).extension() != Some(OsStr::new(extension)) {
            continue;
        }

        let relative = path.replace('\\', "/");

        if !roots.is_empty()
            && !roots
                .iter()
                .any(|root| relative.starts_with(root.trim_end_matches('/')))
        {
            continue;
        }

        if !matcher.is_included(&relative) {
            continue;
        }
        if let Some(changed) = changed_paths {
            let normalized = normalize_git_path(&relative);
            if !changed.contains(&normalized) {
                continue;
            }
        }

        let absolute_path = repo.join(path);
        let contents = fs::read_to_string(&absolute_path)
            .with_context(|| format!("failed to read source file {}", absolute_path.display()))?;

        collected.push(SourceFile {
            path: relative,
            contents,
            snapshot: Some("WORKTREE".to_string()),
        });
    }

    Ok(collected)
}

fn git_ls_files_worktree(repo: &Path) -> Result<Vec<String>> {
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

fn is_ignored_generated_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let generated_markers = [
        "/build/",
        "/.build/",
        "/DerivedData/",
        "/Pods/",
        "/.gradle/",
        "/.swiftpm/",
        "/Generated/",
        "/generated/",
    ];
    generated_markers
        .iter()
        .any(|marker| normalized.contains(marker))
}

fn normalize_git_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn git_ls_tree(repo: &Path, git_ref: &str) -> Result<Vec<String>> {
    let output = git_command(repo, ["ls-tree", "-r", "--name-only", git_ref])?;
    Ok(output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn git_show(repo: &Path, git_ref: &str, path: &str) -> Result<String> {
    git_command(repo, ["show", &format!("{git_ref}:{path}")])
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

#[allow(dead_code)]
fn repo_relative(repo: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(repo).unwrap_or(path).to_path_buf()
}
