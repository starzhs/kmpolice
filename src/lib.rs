pub mod analyzer;
pub mod cli;
pub mod config;
pub mod model;
pub mod parser;
pub mod report;
pub mod source;

use anyhow::Result;
use clap::Parser;
use regex::Regex;
use std::collections::HashSet;

use analyzer::{compare_project, introduced_diagnostics};
use cli::{CheckCommand, Cli, OutputFormat};
use config::{Config, Severity};
use model::{Diagnostic, ProjectSnapshot};
use report::{render_json, render_text};
use source::{
    has_meaningful_code_diff, has_unmerged_paths, is_head_detached, is_shallow_repository,
    is_worktree_dirty, load_from_git, load_from_paths, load_from_worktree, merge_base, resolve_ref,
};

pub fn run() -> Result<i32> {
    let cli = Cli::parse().normalized_command();
    let config = Config::load(cli.config_path())?;

    let diagnostics = match &cli.command {
        CheckCommand::Paths(args) => {
            eprintln!("[kmpolice] mode=paths: collecting files...");
            let snapshot = load_from_paths(&args.kotlin, &args.ios, &config)?;
            eprintln!(
                "[kmpolice] mode=paths: kotlin_files={} ios_files={} -> analyzing...",
                snapshot.kotlin_files.len(),
                snapshot.ios_files.len()
            );
            let mut diagnostics = compare_project(&snapshot, &config)?;
            downgrade_unverified_type_usage(&mut diagnostics);
            eprintln!(
                "[kmpolice] mode=paths: completed diagnostics={}",
                diagnostics.len()
            );
            diagnostics
        }
        CheckCommand::Git(args) => {
            if has_unmerged_paths(&args.repo)? {
                return Err(anyhow::anyhow!(
                    "repository has unresolved merge conflicts (unmerged paths). Resolve conflicts before running kmpolice."
                ));
            }
            if is_head_detached(&args.repo)? {
                eprintln!("[kmpolice] mode=git: HEAD is detached; continuing with explicit refs.");
            }
            if is_shallow_repository(&args.repo)? {
                eprintln!(
                    "[kmpolice] mode=git: shallow repository detected; if refs are missing, fetch additional history."
                );
            }
            eprintln!(
                "[kmpolice] mode=git: resolving refs base={} head={}...",
                args.base_ref, args.head_ref
            );
            let base_sha = resolve_ref(&args.repo, &args.base_ref)?;
            let head_sha = resolve_ref(&args.repo, &args.head_ref)?;
            let worktree_dirty = is_worktree_dirty(&args.repo)?;
            if base_sha == head_sha && !worktree_dirty {
                eprintln!(
                    "[kmpolice] mode=git: base and head are identical ({}), worktree is clean, fast exit.",
                    base_sha
                );
                Vec::new()
            } else if !worktree_dirty
                && !has_meaningful_code_diff(&args.repo, &args.base_ref, &args.head_ref)?
            {
                eprintln!(
                    "[kmpolice] mode=git: only non-meaningful changes (eol/filemode/whitespace-at-eol), fast exit."
                );
                Vec::new()
            } else {
                eprintln!("[kmpolice] mode=git: loading base snapshot...");
                let base_snapshot = load_from_git(&args.repo, &args.base_ref, &config)?;
                eprintln!(
                    "[kmpolice] mode=git: base kotlin_files={} ios_files={}",
                    base_snapshot.kotlin_files.len(),
                    base_snapshot.ios_files.len()
                );
                eprintln!("[kmpolice] mode=git: loading head snapshot...");
                let head_snapshot = if base_sha == head_sha && worktree_dirty {
                    eprintln!(
                        "[kmpolice] mode=git: refs are identical but worktree is dirty, using WORKTREE as head snapshot."
                    );
                    load_from_worktree(&args.repo, &config)?
                } else {
                    load_from_git(&args.repo, &args.head_ref, &config)?
                };
                eprintln!(
                    "[kmpolice] mode=git: head kotlin_files={} ios_files={} -> analyzing...",
                    head_snapshot.kotlin_files.len(),
                    head_snapshot.ios_files.len()
                );

                let base_diagnostics = compare_project(&base_snapshot, &config)?;
                let mut head_diagnostics = compare_project(&head_snapshot, &config)?;
                apply_diff_aware_type_usage_severity(
                    &base_snapshot,
                    &head_snapshot,
                    &mut head_diagnostics,
                );

                for diagnostic in &mut head_diagnostics {
                    diagnostic.base_ref = Some(args.base_ref.clone());
                    diagnostic.head_ref = Some(if base_sha == head_sha && worktree_dirty {
                        "WORKTREE".to_string()
                    } else {
                        args.head_ref.clone()
                    });
                }

                if args.introduced_only {
                    let introduced = introduced_diagnostics(base_diagnostics, head_diagnostics);
                    eprintln!(
                        "[kmpolice] mode=git: completed introduced_only diagnostics={}",
                        introduced.len()
                    );
                    introduced
                } else {
                    eprintln!(
                        "[kmpolice] mode=git: completed diagnostics={}",
                        head_diagnostics.len()
                    );
                    head_diagnostics
                }
            }
        }
        CheckCommand::Mr(args) => {
            if has_unmerged_paths(&args.repo)? {
                return Err(anyhow::anyhow!(
                    "repository has unresolved merge conflicts (unmerged paths). Resolve conflicts before running kmpolice."
                ));
            }
            if is_head_detached(&args.repo)? {
                eprintln!("[kmpolice] mode=mr: HEAD is detached; continuing with explicit refs.");
            }
            let shallow_repo = is_shallow_repository(&args.repo)?;
            if shallow_repo {
                eprintln!(
                    "[kmpolice] mode=mr: shallow repository detected; merge-base can be incomplete."
                );
            }
            eprintln!(
                "[kmpolice] mode=mr: resolving merge-base target={} head=HEAD...",
                args.target
            );
            let head_ref = "HEAD".to_string();
            let base_ref = merge_base(&args.repo, &args.target, &head_ref).map_err(|err| {
                if shallow_repo {
                    anyhow::anyhow!(
                        "{}; shallow history may hide merge-base. Try `git fetch --unshallow` or fetch the target branch depth.",
                        err
                    )
                } else {
                    err
                }
            })?;
            let base_sha = resolve_ref(&args.repo, &base_ref)?;
            let head_sha = resolve_ref(&args.repo, &head_ref)?;
            let worktree_dirty = is_worktree_dirty(&args.repo)?;
            if base_sha == head_sha && !worktree_dirty {
                eprintln!(
                    "[kmpolice] mode=mr: base and head are identical ({}), worktree is clean, fast exit.",
                    base_sha
                );
                Vec::new()
            } else if !worktree_dirty
                && !has_meaningful_code_diff(&args.repo, &base_ref, &head_ref)?
            {
                eprintln!(
                    "[kmpolice] mode=mr: only non-meaningful changes (eol/filemode/whitespace-at-eol), fast exit."
                );
                Vec::new()
            } else {
                eprintln!("[kmpolice] mode=mr: loading base snapshot...");

                let base_snapshot = load_from_git(&args.repo, &base_ref, &config)?;
                eprintln!(
                    "[kmpolice] mode=mr: base kotlin_files={} ios_files={}",
                    base_snapshot.kotlin_files.len(),
                    base_snapshot.ios_files.len()
                );
                eprintln!("[kmpolice] mode=mr: loading head snapshot...");
                let head_snapshot = if base_sha == head_sha && worktree_dirty {
                    eprintln!(
                        "[kmpolice] mode=mr: refs are identical but worktree is dirty, using WORKTREE as head snapshot."
                    );
                    load_from_worktree(&args.repo, &config)?
                } else {
                    load_from_git(&args.repo, &head_ref, &config)?
                };
                eprintln!(
                    "[kmpolice] mode=mr: head kotlin_files={} ios_files={} -> analyzing...",
                    head_snapshot.kotlin_files.len(),
                    head_snapshot.ios_files.len()
                );

                let base_diagnostics = compare_project(&base_snapshot, &config)?;
                let mut head_diagnostics = compare_project(&head_snapshot, &config)?;
                apply_diff_aware_type_usage_severity(
                    &base_snapshot,
                    &head_snapshot,
                    &mut head_diagnostics,
                );

                for diagnostic in &mut head_diagnostics {
                    diagnostic.base_ref = Some(base_ref.clone());
                    diagnostic.head_ref = Some(if base_sha == head_sha && worktree_dirty {
                        "WORKTREE".to_string()
                    } else {
                        head_ref.clone()
                    });
                }

                let introduced = introduced_diagnostics(base_diagnostics, head_diagnostics);
                eprintln!(
                    "[kmpolice] mode=mr: completed introduced diagnostics={}",
                    introduced.len()
                );
                introduced
            }
        }
        CheckCommand::Check(_) => unreachable!("cli command should be normalized before execution"),
    };

    let output = match cli.output_format() {
        OutputFormat::Text => render_text(&diagnostics),
        OutputFormat::Json => render_json(&diagnostics)?,
    };

    println!("{output}");

    Ok(if diagnostics.is_empty() { 0 } else { 1 })
}

fn downgrade_unverified_type_usage(diagnostics: &mut [Diagnostic]) {
    for diagnostic in diagnostics.iter_mut() {
        if diagnostic.code == "kotlin_type_usage_missing" {
            diagnostic.severity = Severity::Warning;
            diagnostic.evidence.push("mode:paths_no_diff".to_string());
            diagnostic
                .evidence
                .push("origin_unresolved_without_diff".to_string());
        }
    }
}

fn apply_diff_aware_type_usage_severity(
    base_snapshot: &ProjectSnapshot,
    head_snapshot: &ProjectSnapshot,
    diagnostics: &mut [Diagnostic],
) {
    let base_symbols = collect_kotlin_declared_symbols(base_snapshot);
    let head_symbols = collect_kotlin_declared_symbols(head_snapshot);
    let added_swift_symbols = collect_added_swift_declared_symbols(base_snapshot, head_snapshot);
    let dependency_manifests_changed = dependency_manifests_changed(base_snapshot, head_snapshot);

    for diagnostic in diagnostics.iter_mut() {
        if diagnostic.code != "kotlin_type_usage_missing" {
            continue;
        }
        let Some(symbol) = diagnostic.kotlin_symbol.as_deref() else {
            diagnostic.severity = Severity::Warning;
            diagnostic
                .evidence
                .push("origin_symbol_missing_in_diagnostic".to_string());
            continue;
        };
        let removed_in_diff = base_symbols.contains(symbol) && !head_symbols.contains(symbol);
        let replacement_added_in_swift_diff = added_swift_symbols.contains(symbol);
        diagnostic.evidence.push(if removed_in_diff {
            "kotlin_symbol_removed_in_diff".to_string()
        } else {
            "kotlin_symbol_not_removed_in_diff".to_string()
        });
        if replacement_added_in_swift_diff {
            diagnostic
                .evidence
                .push("swift_replacement_symbol_added_in_diff".to_string());
        }
        if dependency_manifests_changed {
            diagnostic
                .evidence
                .push("dependency_manifest_changed_in_diff".to_string());
        } else {
            diagnostic
                .evidence
                .push("dependency_manifests_unchanged".to_string());
        }
        diagnostic.severity =
            if removed_in_diff && !replacement_added_in_swift_diff && !dependency_manifests_changed
            {
                diagnostic
                    .evidence
                    .push("high_confidence_breakage_signal".to_string());
                Severity::Error
            } else {
                diagnostic
                    .evidence
                    .push("softened_due_to_ambiguity".to_string());
                Severity::Warning
            };
    }
}

fn collect_added_swift_declared_symbols(
    base_snapshot: &ProjectSnapshot,
    head_snapshot: &ProjectSnapshot,
) -> HashSet<String> {
    let base_symbols = collect_swift_declared_symbols(base_snapshot);
    let head_symbols = collect_swift_declared_symbols(head_snapshot);
    head_symbols
        .into_iter()
        .filter(|symbol| !base_symbols.contains(symbol))
        .collect()
}

fn collect_swift_declared_symbols(snapshot: &ProjectSnapshot) -> HashSet<String> {
    let mut symbols = HashSet::new();
    let swift_decl_regex = Regex::new(r"(?m)^\s*(?:struct|class|protocol|enum)\s+([A-Za-z_]\w*)")
        .expect("swift decl regex should compile");
    for file in &snapshot.ios_files {
        for captures in swift_decl_regex.captures_iter(&file.contents) {
            symbols.insert(captures[1].to_string());
        }
    }
    symbols
}

fn collect_kotlin_declared_symbols(snapshot: &ProjectSnapshot) -> HashSet<String> {
    let mut symbols = HashSet::new();
    let type_decl_regex = Regex::new(
        r"(?m)^\s*(?:public\s+)?(?:data\s+class|class|interface|object|enum\s+class|sealed\s+class)\s+([A-Za-z_]\w*)",
    )
    .expect("type decl regex should compile");
    let typealias_regex = Regex::new(r"(?m)^\s*(?:public\s+)?typealias\s+([A-Za-z_]\w*)\s*=")
        .expect("typealias regex should compile");

    for file in &snapshot.kotlin_files {
        for captures in type_decl_regex.captures_iter(&file.contents) {
            symbols.insert(captures[1].to_string());
        }
        for captures in typealias_regex.captures_iter(&file.contents) {
            symbols.insert(captures[1].to_string());
        }
    }

    symbols
}

fn dependency_manifests_changed(
    base_snapshot: &ProjectSnapshot,
    head_snapshot: &ProjectSnapshot,
) -> bool {
    let manifest_suffixes = [
        "Package.swift",
        "Podfile",
        "Podfile.lock",
        "Cartfile",
        "Cartfile.resolved",
        "project.pbxproj",
        "Package.resolved",
        "gradle/libs.versions.toml",
    ];
    let manifest_file = |path: &str| {
        manifest_suffixes
            .iter()
            .any(|suffix| path.ends_with(suffix))
    };

    let base_manifest_contents: HashSet<(String, String)> = base_snapshot
        .ios_files
        .iter()
        .chain(base_snapshot.kotlin_files.iter())
        .filter(|file| manifest_file(&file.path))
        .map(|file| (file.path.clone(), file.contents.clone()))
        .collect();

    let head_manifest_contents: HashSet<(String, String)> = head_snapshot
        .ios_files
        .iter()
        .chain(head_snapshot.kotlin_files.iter())
        .filter(|file| manifest_file(&file.path))
        .map(|file| (file.path.clone(), file.contents.clone()))
        .collect();

    base_manifest_contents != head_manifest_contents
}
