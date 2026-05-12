use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
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
    let matcher = config.path_matcher()?;
    let files = git_ls_tree(repo, git_ref)?;

    let kotlin_files =
        collect_git_files(repo, git_ref, &files, "kt", &matcher, &config.kotlin_roots)?;
    let ios_files = collect_git_files(repo, git_ref, &files, "swift", &matcher, &config.ios_roots)?;

    Ok(ProjectSnapshot {
        label: git_ref.to_string(),
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
) -> Result<Vec<SourceFile>> {
    let mut files = Vec::new();

    for path in paths {
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

        let contents = git_show(repo, git_ref, path)?;
        files.push(SourceFile {
            path: path.clone(),
            contents,
            snapshot: Some(git_ref.to_string()),
        });
    }

    Ok(files)
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
