use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::HashSet;
use walkdir::WalkDir;

use crate::config::{Config, PathMatcher};
use crate::git::{
    git_ls_files_worktree, git_ls_tree, git_show, git_toplevel_for_path, normalize_git_path,
};
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
    load_from_git_scoped(repo, git_ref, config, None, None)
}

pub fn load_from_git_scoped(
    repo: &Path,
    git_ref: &str,
    config: &Config,
    kotlin_changed_paths: Option<&HashSet<String>>,
    ios_changed_paths: Option<&HashSet<String>>,
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
        kotlin_changed_paths,
    )?;
    let ios_files = collect_git_files(
        repo,
        git_ref,
        &files,
        "swift",
        &matcher,
        &config.ios_roots,
        ios_changed_paths,
    )?;

    Ok(ProjectSnapshot {
        label: git_ref.to_string(),
        kotlin_files,
        ios_files,
    })
}

pub fn load_from_worktree(repo: &Path, config: &Config) -> Result<ProjectSnapshot> {
    load_from_worktree_scoped(repo, config, None, None)
}

pub fn load_from_worktree_scoped(
    repo: &Path,
    config: &Config,
    kotlin_changed_paths: Option<&HashSet<String>>,
    ios_changed_paths: Option<&HashSet<String>>,
) -> Result<ProjectSnapshot> {
    let matcher = config.path_matcher()?;
    let files = git_ls_files_worktree(repo)?;
    let kotlin_files = collect_worktree_git_list_files(
        repo,
        &files,
        "kt",
        &matcher,
        &config.kotlin_roots,
        kotlin_changed_paths,
    )?;
    let ios_files = collect_worktree_git_list_files(
        repo,
        &files,
        "swift",
        &matcher,
        &config.ios_roots,
        ios_changed_paths,
    )?;

    Ok(ProjectSnapshot {
        label: "WORKTREE".to_string(),
        kotlin_files,
        ios_files,
    })
}

pub fn collect_worktree_paths_scoped(
    repo: &Path,
    config: &Config,
    extension: &str,
) -> Result<Vec<String>> {
    let matcher = config.path_matcher()?;
    let files = git_ls_files_worktree(repo)?;
    let roots = if extension == "swift" {
        &config.ios_roots
    } else {
        &config.kotlin_roots
    };
    collect_worktree_paths_from_git_list(&files, extension, &matcher, roots)
}

fn collect_path_files(
    root: &Path,
    extension: &str,
    matcher: &PathMatcher,
) -> Result<Vec<SourceFile>> {
    if let Some(files) = collect_path_files_from_git(root, extension, matcher)? {
        return Ok(files);
    }

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

fn collect_path_files_from_git(
    root: &Path,
    extension: &str,
    matcher: &PathMatcher,
) -> Result<Option<Vec<SourceFile>>> {
    let repo_root = match git_toplevel_for_path(root)? {
        Some(path) => path,
        None => return Ok(None),
    };
    let root_rel = root
        .strip_prefix(&repo_root)
        .ok()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    let files = git_ls_files_worktree(&repo_root)?;

    let mut collected = Vec::new();
    for path in files {
        let normalized = normalize_git_path(&path);
        if !root_rel.is_empty() && !normalized.starts_with(root_rel.trim_end_matches('/')) {
            continue;
        }
        if is_ignored_generated_path(&normalized) {
            continue;
        }
        if Path::new(&normalized).extension() != Some(OsStr::new(extension)) {
            continue;
        }
        let relative_for_matcher = if root_rel.is_empty() {
            normalized.clone()
        } else {
            normalized
                .strip_prefix(&(root_rel.clone() + "/"))
                .unwrap_or(&normalized)
                .to_string()
        };
        if !matcher.is_included(&relative_for_matcher) {
            continue;
        }

        let absolute_path = repo_root.join(&normalized);
        let contents = fs::read_to_string(&absolute_path)
            .with_context(|| format!("failed to read source file {}", absolute_path.display()))?;
        collected.push(SourceFile {
            path: absolute_path.display().to_string(),
            contents,
            snapshot: None,
        });
    }

    Ok(Some(collected))
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

        if changed_paths.is_none()
            && !roots.is_empty()
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

    let progress = ProgressBar::new(candidates.len() as u64);
    progress.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} {msg:<72} [{bar:30.cyan/blue}] {pos}/{len} ({percent}%)",
        )
        .expect("snapshot progress style")
        .progress_chars("=> "),
    );
    progress.set_message(format!(
        "Snapshot load {} .{} | waiting...",
        git_ref, extension
    ));

    for path in candidates.iter() {
        progress.set_message(format!(
            "Snapshot load {} .{} | last file: {}",
            git_ref, extension, path
        ));
        let contents = git_show(repo, git_ref, path)?;
        files.push(SourceFile {
            path: (*path).clone(),
            contents,
            snapshot: Some(git_ref.to_string()),
        });
        progress.inc(1);
    }
    progress.finish_with_message(format!("Snapshot load {} .{} | done", git_ref, extension));

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

        if changed_paths.is_none()
            && !roots.is_empty()
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

fn collect_worktree_paths_from_git_list(
    files: &[String],
    extension: &str,
    matcher: &PathMatcher,
    roots: &[String],
) -> Result<Vec<String>> {
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
        collected.push(relative);
    }
    Ok(collected)
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

#[allow(dead_code)]
fn repo_relative(repo: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(repo).unwrap_or(path).to_path_buf()
}
