use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use regex::Regex;
use tree_sitter::{Node, Parser};

use crate::analyzer::{compare_project, introduced_diagnostics};
use crate::config::{Config, Severity};
use crate::git::{git_changed_files_between, git_changed_files_worktree, merge_base};
use crate::ios_usage::{IosUsageReport, find_ios_usages};
use crate::model::{
    Contract, ContractKind, Diagnostic, Member, MemberSignature, ProjectSnapshot, SourceFile,
};
use crate::source::{
    collect_git_paths_scoped, collect_worktree_paths_scoped, load_from_git_scoped,
    load_from_worktree_scoped,
};

#[derive(Debug, Clone)]
pub struct ApiChange {
    pub symbol: String,
    pub kind: String,
    pub file: Option<String>,
    pub details: String,
}

#[derive(Debug, Clone)]
pub struct MrResult {
    pub diagnostics: Vec<Diagnostic>,
    pub api_changes: Vec<ApiChange>,
    pub ios_usage: IosUsageReport,
    pub debug: MrDebugInfo,
}

#[derive(Debug, Clone, Default)]
pub struct MrDebugInfo {
    pub kotlin_changed_paths: Vec<String>,
    pub kotlin_file_stats: Vec<KotlinFileDebugStat>,
}

#[derive(Debug, Clone)]
pub struct KotlinFileDebugStat {
    pub path: String,
    pub before_present: bool,
    pub after_present: bool,
    pub api_change_count: usize,
}

pub fn run_mr(
    repo: &Path,
    target: &str,
    config: &Config,
    verbose: bool,
    shared_sdk_name: &str,
) -> Result<MrResult> {
    let repo = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    let base = merge_base(&repo, target, "HEAD")?;

    let kotlin_changed = collect_kotlin_changed_paths(&repo, &base)?;
    let swift_changed = collect_swift_changed_paths(&repo, &base)?;
    let ios_paths = collect_worktree_paths_scoped(&repo, config, "swift")?;
    let swift_scope_hints = collect_swift_scope_hints(&repo, &swift_changed, shared_sdk_name);
    let ios_scope = Some(&swift_changed);
    let initial_kotlin_scope = Some(&kotlin_changed);

    // Stage 1: narrow diff on changed Kotlin files only.
    let base_snapshot_narrow =
        load_from_git_scoped(&repo, &base, config, initial_kotlin_scope, ios_scope)?;
    let head_snapshot_narrow =
        load_from_worktree_scoped(&repo, config, initial_kotlin_scope, ios_scope)?;
    let api_changes = diff_kotlin_api_changes(
        &repo,
        &base,
        &kotlin_changed,
        &base_snapshot_narrow,
        &head_snapshot_narrow,
    )?;

    // Stage 2: auto-expand Kotlin scope with related files.
    let kotlin_scope = compute_auto_kotlin_scope(
        &repo,
        &base,
        config,
        &kotlin_changed,
        &api_changes,
        &swift_scope_hints,
    )?;
    let kotlin_scope_ref = Some(&kotlin_scope);
    let base_snapshot = load_from_git_scoped(&repo, &base, config, kotlin_scope_ref, ios_scope)?;
    let head_snapshot = load_from_worktree_scoped(&repo, config, kotlin_scope_ref, ios_scope)?;

    let base_diags = compare_project(&base_snapshot, config)?;
    let head_diags = compare_project(&head_snapshot, config)?;
    let mut diagnostics = introduced_diagnostics(base_diags, head_diags);

    let ios_usage = find_ios_usages(
        &api_changes,
        &repo,
        &ios_paths,
        shared_sdk_name,
        &swift_changed,
    )?;
    diagnostics.extend(build_ios_impact_diagnostics(
        &api_changes,
        &ios_usage,
        config,
    ));
    if verbose {
        print_verbose_changes(&api_changes);
    }

    Ok(MrResult {
        diagnostics,
        api_changes,
        ios_usage,
        debug: build_debug_info(&kotlin_changed, &base_snapshot, &head_snapshot),
    })
}

fn build_debug_info(
    kotlin_changed: &HashSet<String>,
    base_snapshot: &ProjectSnapshot,
    head_snapshot: &ProjectSnapshot,
) -> MrDebugInfo {
    let mut changed_paths: Vec<String> = kotlin_changed.iter().cloned().collect();
    changed_paths.sort();
    let changed_set: HashSet<&str> = kotlin_changed.iter().map(String::as_str).collect();
    let before_set: HashSet<&str> = base_snapshot
        .kotlin_files
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    let after_set: HashSet<&str> = head_snapshot
        .kotlin_files
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    let mut stats = Vec::with_capacity(changed_paths.len());
    for path in &changed_paths {
        let path_ref = path.as_str();
        if !changed_set.contains(path_ref) {
            continue;
        }
        stats.push(KotlinFileDebugStat {
            path: path.clone(),
            before_present: before_set.contains(path_ref),
            after_present: after_set.contains(path_ref),
            api_change_count: 0,
        });
    }
    MrDebugInfo {
        kotlin_changed_paths: changed_paths,
        kotlin_file_stats: stats,
    }
}

fn build_ios_impact_diagnostics(
    api_changes: &[ApiChange],
    usage: &IosUsageReport,
    config: &Config,
) -> Vec<Diagnostic> {
    let by_key: HashMap<(String, String), &ApiChange> = api_changes
        .iter()
        .map(|change| ((change.kind.clone(), change.symbol.clone()), change))
        .collect();

    let mut out = Vec::new();
    let mut unique = HashSet::<String>::new();
    for hit in &usage.hits {
        let key = (hit.kind.clone(), hit.symbol.clone());
        let Some(change) = by_key.get(&key) else {
            continue;
        };
        let fp = format!("{}|{}|{}", hit.kind, hit.symbol, hit.file);
        if !unique.insert(fp) {
            continue;
        }
        let code = impact_code_for_kind(&hit.kind);
        let severity = if hit.already_touched {
            Severity::Info
        } else {
            config.severity_for(code)
        };
        out.push(Diagnostic {
            code: code.to_string(),
            severity,
            message: format!(
                "Kotlin API change `{}` for `{}` is used in iOS file `{}`",
                hit.kind, hit.symbol, hit.file
            ),
            hint: format!(
                "change detail: {}; matched tokens: {}; swift file status: {}; verify/update call-site",
                change.details,
                hit.evidence,
                if hit.already_touched {
                    "already_touched_in_mr"
                } else {
                    "untouched_in_mr"
                }
            ),
            kotlin_symbol: Some(hit.symbol.clone()),
            ios_symbol: Some(hit.file.clone()),
            member: None,
            kotlin_location: None,
            ios_location: None,
            base_ref: None,
            head_ref: None,
            evidence: vec![
                "mr_mode:diff_aware".to_string(),
                "kotlin_change_detected".to_string(),
                "ios_usage_index_hit".to_string(),
                if hit.already_touched {
                    "swift_file:already_touched".to_string()
                } else {
                    "swift_file:untouched".to_string()
                },
            ],
        });
    }
    out
}

fn impact_code_for_kind(kind: &str) -> &'static str {
    match kind {
        "constructor" => "mr_constructor_ios_impact",
        "enum_sealed" => "mr_enum_sealed_ios_impact",
        "top_level" => "mr_top_level_ios_impact",
        "companion" => "mr_companion_ios_impact",
        "typealias" => "mr_typealias_ios_impact",
        "member" => "mr_member_ios_impact",
        "type" => "mr_type_ios_impact",
        _ => "mr_kotlin_api_ios_impact",
    }
}

fn collect_kotlin_changed_paths(repo: &Path, base: &str) -> Result<HashSet<String>> {
    let mut changed = git_changed_files_between(repo, base, "HEAD")?;
    changed.extend(git_changed_files_worktree(repo)?);
    Ok(changed
        .into_iter()
        .filter(|path| path.ends_with(".kt"))
        .filter(|path| path.contains("/commonMain/") || path.contains("/iosMain/"))
        .collect())
}

fn collect_swift_changed_paths(repo: &Path, base: &str) -> Result<HashSet<String>> {
    let mut changed = git_changed_files_between(repo, base, "HEAD")?;
    changed.extend(git_changed_files_worktree(repo)?);
    Ok(changed
        .into_iter()
        .filter(|path| path.ends_with(".swift"))
        .collect())
}

fn compute_auto_kotlin_scope(
    repo: &Path,
    base: &str,
    config: &Config,
    kotlin_changed: &HashSet<String>,
    api_changes: &[ApiChange],
    swift_scope_hints: &HashSet<String>,
) -> Result<HashSet<String>> {
    let mut scope = kotlin_changed.clone();
    if api_changes.is_empty() && swift_scope_hints.is_empty() {
        return Ok(scope);
    }

    let mut universe = HashSet::<String>::new();
    let worktree_paths = collect_worktree_paths_scoped(repo, config, "kt")?;
    let base_paths = collect_git_paths_scoped(repo, base, config, "kt")?;
    universe.extend(
        worktree_paths
            .into_iter()
            .filter(|path| is_kotlin_shared_source_set_path(path)),
    );
    universe.extend(
        base_paths
            .into_iter()
            .filter(|path| is_kotlin_shared_source_set_path(path)),
    );
    universe.extend(kotlin_changed.iter().cloned());

    let owners = change_owner_type_names(api_changes);
    let top_level_symbols = top_level_change_symbols(api_changes);
    if owners.is_empty() && top_level_symbols.is_empty() && swift_scope_hints.is_empty() {
        return Ok(scope);
    }

    let candidates: Vec<String> = universe.into_iter().collect();
    let progress = Arc::new(ProgressBar::new(candidates.len() as u64));
    progress.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} {msg:<72} [{bar:30.cyan/blue}] {pos}/{len} ({percent}%)",
        )
        .expect("progress style")
        .progress_chars("=> "),
    );
    progress.set_message("Kotlin scope expand | waiting...");

    let expanded: HashSet<String> = candidates
        .par_iter()
        .filter_map(|path| {
            let include = std::fs::read_to_string(repo.join(path))
                .ok()
                .is_some_and(|contents| {
                    kotlin_file_may_be_related(
                        &contents,
                        &owners,
                        &top_level_symbols,
                        swift_scope_hints,
                    )
                });
            progress.set_message(format!("Kotlin scope expand | last file: {path}"));
            progress.inc(1);
            include.then(|| path.clone())
        })
        .collect();

    progress.finish_with_message("Kotlin scope expand done");
    scope.extend(expanded);
    Ok(scope)
}

fn is_kotlin_shared_source_set_path(path: &str) -> bool {
    path.ends_with(".kt") && (path.contains("/commonMain/") || path.contains("/iosMain/"))
}

fn simple_type_name(symbol: &str) -> String {
    symbol
        .rsplit('.')
        .next()
        .unwrap_or(symbol)
        .trim()
        .to_string()
}

fn change_owner_type_names(changes: &[ApiChange]) -> HashSet<String> {
    changes
        .iter()
        .filter(|change| {
            matches!(
                change.kind.as_str(),
                "type" | "member" | "constructor" | "enum_sealed" | "companion"
            )
        })
        .map(|change| simple_type_name(&change.symbol))
        .filter(|name| !name.is_empty())
        .collect()
}

fn top_level_change_symbols(changes: &[ApiChange]) -> HashSet<String> {
    changes
        .iter()
        .filter(|change| change.kind == "top_level")
        .map(|change| change.symbol.clone())
        .filter(|name| !name.is_empty())
        .collect()
}

fn kotlin_file_may_be_related(
    contents: &str,
    owner_types: &HashSet<String>,
    top_level_symbols: &HashSet<String>,
    swift_scope_hints: &HashSet<String>,
) -> bool {
    for owner in owner_types {
        if contents.contains(owner) {
            return true;
        }
    }
    for symbol in top_level_symbols {
        if contents.contains(symbol) {
            return true;
        }
    }
    for token in swift_scope_hints {
        if contents.contains(token) {
            return true;
        }
    }
    false
}

fn collect_swift_scope_hints(
    repo: &Path,
    swift_changed: &HashSet<String>,
    shared_sdk_name: &str,
) -> HashSet<String> {
    let mut hints = HashSet::new();
    for path in swift_changed {
        if !path.ends_with(".swift") {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(repo.join(path)) else {
            continue;
        };
        hints.extend(extract_swift_scope_hints_from_contents(
            &contents,
            shared_sdk_name,
        ));
    }
    hints
}

fn extract_swift_scope_hints_from_contents(
    contents: &str,
    shared_sdk_name: &str,
) -> HashSet<String> {
    let mut out = HashSet::new();
    if !contains_shared_import_for_scope(contents, shared_sdk_name) {
        return out;
    }

    let companion_usage_regex = Regex::new(r"\b([A-Z][A-Za-z_0-9]*)\.companion\.([A-Za-z_]\w*)")
        .expect("companion usage regex");
    for captures in companion_usage_regex.captures_iter(contents) {
        out.insert(captures[1].to_string());
        out.insert(captures[2].to_string());
    }

    let kt_usage_regex =
        Regex::new(r"\b([A-Z][A-Za-z_0-9]*Kt)\.([A-Za-z_]\w*)").expect("kt usage regex");
    for captures in kt_usage_regex.captures_iter(contents) {
        out.insert(captures[1].to_string());
        out.insert(captures[2].to_string());
    }

    let common_swift_types: HashSet<&'static str> = [
        "String",
        "Int",
        "Int32",
        "Int64",
        "UInt",
        "UInt32",
        "UInt64",
        "Double",
        "Float",
        "Bool",
        "Void",
        "Any",
        "AnyObject",
        "Error",
        "Result",
        "Array",
        "Dictionary",
        "Set",
        "Optional",
        "URL",
        "Data",
    ]
    .into_iter()
    .collect();

    let type_identifier_regex =
        Regex::new(r"\b([A-Z][A-Za-z_0-9]*)\b").expect("type identifier regex");
    for captures in type_identifier_regex.captures_iter(contents) {
        let token = captures[1].to_string();
        if common_swift_types.contains(token.as_str()) {
            continue;
        }
        out.insert(token);
    }

    out
}

fn contains_shared_import_for_scope(contents: &str, shared_sdk_name: &str) -> bool {
    contents.lines().any(|line| {
        let mut parts = line.split_whitespace().peekable();
        while let Some(token) = parts.peek().copied() {
            if token.starts_with('@') {
                parts.next();
                continue;
            }
            break;
        }

        let Some(keyword) = parts.next() else {
            return false;
        };
        if keyword != "import" {
            return false;
        }

        let Some(module_token) = parts.next() else {
            return false;
        };
        module_token == shared_sdk_name || module_token.starts_with(&format!("{shared_sdk_name}."))
    })
}

fn diff_kotlin_api_changes(
    repo: &Path,
    base: &str,
    kotlin_changed: &HashSet<String>,
    base_snapshot: &ProjectSnapshot,
    head_snapshot: &ProjectSnapshot,
) -> Result<Vec<ApiChange>> {
    let _ = (repo, base);
    let before_by_path: HashMap<&str, &SourceFile> = base_snapshot
        .kotlin_files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    let after_by_path: HashMap<&str, &SourceFile> = head_snapshot
        .kotlin_files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();

    let mut paths: BTreeSet<String> = BTreeSet::new();
    paths.extend(kotlin_changed.iter().cloned());
    paths.extend(before_by_path.keys().map(|p| (*p).to_string()));
    paths.extend(after_by_path.keys().map(|p| (*p).to_string()));

    let entries: Vec<(String, Option<SourceFile>, Option<SourceFile>)> = paths
        .into_iter()
        .map(|path| {
            let before = before_by_path.get(path.as_str()).copied().cloned();
            let after = after_by_path.get(path.as_str()).copied().cloned();
            (path, before, after)
        })
        .collect();

    let progress = Arc::new(ProgressBar::new(entries.len() as u64));
    progress.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} {msg:<72} [{bar:30.cyan/blue}] {pos}/{len} ({percent}%)",
        )
        .expect("progress style")
        .progress_chars("=> "),
    );
    progress.set_message("Kotlin AST expand | waiting...");

    let per_file_changes: Vec<Vec<ApiChange>> = entries
        .par_iter()
        .map(|(path, before, after)| {
            let before_ref = before.as_ref();
            let after_ref = after.as_ref();

            let mut local = Vec::new();
            let before_contracts = contracts_for_file(before_ref).unwrap_or_default();
            let after_contracts = contracts_for_file(after_ref).unwrap_or_default();
            local.extend(diff_contract_sets(
                &before_contracts,
                &after_contracts,
                path,
            ));
            local.extend(diff_first_class_symbols(before_ref, after_ref, path));
            progress.set_message(format!("Kotlin AST expand | last file: {path}"));
            progress.inc(1);
            local
        })
        .collect();
    progress.finish_with_message("Kotlin AST expand done");

    let mut changes = Vec::new();
    let mut unique = HashSet::<String>::new();
    for group in per_file_changes {
        for change in group {
            push_unique_change(&mut changes, &mut unique, change);
        }
    }
    Ok(changes)
}

fn push_unique_change(
    changes: &mut Vec<ApiChange>,
    unique: &mut HashSet<String>,
    change: ApiChange,
) {
    let key = format!(
        "{}|{}|{}|{}",
        change.kind,
        change.symbol,
        change.file.clone().unwrap_or_default(),
        change.details
    );
    if unique.insert(key) {
        changes.push(change);
    }
}

fn contracts_for_file(file: Option<&SourceFile>) -> Result<Vec<Contract>> {
    let Some(file) = file else {
        return Ok(Vec::new());
    };
    let snapshot = ProjectSnapshot {
        label: "tmp".to_string(),
        kotlin_files: vec![file.clone()],
        ios_files: Vec::new(),
    };
    let analysis = crate::parser::analyze(&snapshot)?;
    Ok(analysis
        .kotlin_contracts
        .into_iter()
        .filter(|contract| {
            matches!(
                contract.kind,
                ContractKind::KotlinClass | ContractKind::KotlinInterface
            )
        })
        .collect())
}

fn diff_contract_sets(before: &[Contract], after: &[Contract], path: &str) -> Vec<ApiChange> {
    let mut out = Vec::new();
    let before_map: HashMap<&str, &Contract> =
        before.iter().map(|c| (c.fq_name.as_str(), c)).collect();
    let after_map: HashMap<&str, &Contract> =
        after.iter().map(|c| (c.fq_name.as_str(), c)).collect();

    for (name, before_c) in &before_map {
        let Some(after_c) = after_map.get(name) else {
            out.push(ApiChange {
                symbol: (*name).to_string(),
                kind: "type".to_string(),
                file: Some(path.to_string()),
                details: "removed".to_string(),
            });
            continue;
        };
        out.extend(diff_members(before_c, after_c, path));
    }
    for name in after_map.keys() {
        if !before_map.contains_key(name) {
            out.push(ApiChange {
                symbol: (*name).to_string(),
                kind: "type".to_string(),
                file: Some(path.to_string()),
                details: "added".to_string(),
            });
        }
    }
    out
}

fn diff_members(before: &Contract, after: &Contract, path: &str) -> Vec<ApiChange> {
    let mut out = Vec::new();
    let before_members: HashMap<&str, &Member> = before
        .members
        .iter()
        .map(|m| (m.name.as_str(), m))
        .collect();
    let after_members: HashMap<&str, &Member> =
        after.members.iter().map(|m| (m.name.as_str(), m)).collect();

    for (name, left) in &before_members {
        let Some(right) = after_members.get(name) else {
            out.push(ApiChange {
                symbol: before.fq_name.clone(),
                kind: "member".to_string(),
                file: Some(path.to_string()),
                details: format!("removed `{name}`"),
            });
            continue;
        };
        if member_signature_fingerprint(left) != member_signature_fingerprint(right) {
            out.push(ApiChange {
                symbol: before.fq_name.clone(),
                kind: "member".to_string(),
                file: Some(path.to_string()),
                details: format!("changed `{name}`"),
            });
        }
    }
    for name in after_members.keys() {
        if !before_members.contains_key(name) {
            out.push(ApiChange {
                symbol: before.fq_name.clone(),
                kind: "member".to_string(),
                file: Some(path.to_string()),
                details: format!("added `{name}`"),
            });
        }
    }
    out
}

fn member_signature_fingerprint(member: &Member) -> String {
    match &member.signature {
        MemberSignature::Method(method) => format!(
            "method({})->{}",
            method
                .parameters
                .iter()
                .map(|p| format!("{}:{}", p.display_name, p.parameter_type))
                .collect::<Vec<_>>()
                .join(","),
            method.return_type
        ),
        MemberSignature::Property(property) => {
            format!("property({}:{})", property.property_type, property.mutable)
        }
    }
}

fn print_verbose_changes(changes: &[ApiChange]) {
    eprintln!("[kmpolice] kotlin api change summary:");
    if changes.is_empty() {
        eprintln!("[kmpolice] no kotlin public API changes detected in commonMain/iosMain");
        return;
    }
    for change in changes {
        eprintln!(
            "[kmpolice] {} {} -> {}",
            change.kind, change.symbol, change.details
        );
    }
}

pub fn render_verbose_changes(changes: &[ApiChange]) -> String {
    let mut out = String::new();
    out.push_str("Kotlin API changes (verbose):");
    if changes.is_empty() {
        out.push_str("\n- none");
        return out;
    }
    let mut grouped: BTreeMap<&str, Vec<&ApiChange>> = BTreeMap::new();
    for change in changes {
        grouped
            .entry(change.kind.as_str())
            .or_default()
            .push(change);
    }
    for (kind, group) in grouped {
        out.push_str(&format!("\n\n{kind}:"));
        for change in group {
            out.push_str(&format!("\n- symbol: {}", change.symbol));
            if let Some(file) = &change.file {
                out.push_str(&format!("\n  file: {file}"));
            }
            out.push_str(&format!("\n  change: {}", change.details));
        }
    }
    out
}

pub fn render_ios_usage_report(report: &IosUsageReport) -> String {
    let mut out = String::new();
    out.push_str("iOS usage index:");
    out.push_str(&format!(
        "\n- swift files total: {}",
        report.swift_files_total
    ));
    out.push_str(&format!("\n- candidate files: {}", report.candidate_files));
    out.push_str(&format!("\n- parsed files: {}", report.parsed_files));
    out.push_str(&format!("\n- matches: {}", report.hits.len()));
    out.push_str(&format!("\n- touched matches: {}", report.touched_hits));
    out.push_str(&format!("\n- untouched matches: {}", report.untouched_hits));
    for hit in &report.hits {
        out.push_str(&format!(
            "\n- [{}] {} in {} ({}, {})",
            hit.kind,
            hit.symbol,
            hit.file,
            hit.evidence,
            if hit.already_touched {
                "already_touched"
            } else {
                "untouched"
            }
        ));
    }
    out
}

pub fn render_mr_debug(debug: &MrDebugInfo, api_changes: &[ApiChange]) -> String {
    let mut by_file: HashMap<&str, usize> = HashMap::new();
    for change in api_changes {
        if let Some(file) = &change.file {
            *by_file.entry(file.as_str()).or_insert(0) += 1;
        }
    }

    let mut out = String::new();
    out.push_str("MR debug:");
    out.push_str(&format!(
        "\n- kotlin changed paths: {}",
        debug.kotlin_changed_paths.len()
    ));
    for path in &debug.kotlin_changed_paths {
        out.push_str(&format!("\n  - {}", path));
    }
    out.push_str("\n- kotlin file stats:");
    for stat in &debug.kotlin_file_stats {
        let api_change_count = by_file.get(stat.path.as_str()).copied().unwrap_or(0);
        out.push_str(&format!(
            "\n  - {} | before={} after={} api_changes={}",
            stat.path, stat.before_present, stat.after_present, api_change_count
        ));
    }
    out
}

#[derive(Debug, Default, Clone)]
struct FirstClassSymbols {
    constructors: HashMap<String, BTreeSet<String>>,
    enum_or_sealed_cases: HashMap<String, BTreeSet<String>>,
    top_level_members: HashMap<String, String>,
    companion_members: HashMap<String, BTreeSet<String>>,
    extension_members: HashMap<String, HashMap<String, BTreeSet<String>>>,
    typealiases: HashMap<String, String>,
}

fn diff_first_class_symbols(
    before: Option<&SourceFile>,
    after: Option<&SourceFile>,
    path: &str,
) -> Vec<ApiChange> {
    let before_symbols = extract_first_class_symbols(before.map(|f| f.contents.as_str()));
    let after_symbols = extract_first_class_symbols(after.map(|f| f.contents.as_str()));
    let mut changes = Vec::new();

    for (owner, before_overloads) in &before_symbols.constructors {
        let after_overloads = after_symbols
            .constructors
            .get(owner)
            .cloned()
            .unwrap_or_default();
        if *before_overloads != after_overloads {
            changes.push(ApiChange {
                symbol: owner.clone(),
                kind: "constructor".to_string(),
                file: Some(path.to_string()),
                details: format!(
                    "before [{}] -> after [{}]",
                    before_overloads
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" | "),
                    after_overloads
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" | ")
                ),
            });
        }
    }
    for (owner, after_overloads) in &after_symbols.constructors {
        if !before_symbols.constructors.contains_key(owner) {
            changes.push(ApiChange {
                symbol: owner.clone(),
                kind: "constructor".to_string(),
                file: Some(path.to_string()),
                details: format!(
                    "added [{}]",
                    after_overloads
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" | ")
                ),
            });
        }
    }

    for (name, before_cases) in &before_symbols.enum_or_sealed_cases {
        let after_cases = after_symbols
            .enum_or_sealed_cases
            .get(name)
            .cloned()
            .unwrap_or_default();
        if *before_cases != after_cases {
            changes.push(ApiChange {
                symbol: name.clone(),
                kind: "enum_sealed".to_string(),
                file: Some(path.to_string()),
                details: format!(
                    "before [{}] -> after [{}]",
                    before_cases.iter().cloned().collect::<Vec<_>>().join(", "),
                    after_cases.iter().cloned().collect::<Vec<_>>().join(", ")
                ),
            });
        }
    }
    for (name, after_cases) in &after_symbols.enum_or_sealed_cases {
        if !before_symbols.enum_or_sealed_cases.contains_key(name) {
            changes.push(ApiChange {
                symbol: name.clone(),
                kind: "enum_sealed".to_string(),
                file: Some(path.to_string()),
                details: format!(
                    "added cases [{}]",
                    after_cases.iter().cloned().collect::<Vec<_>>().join(", ")
                ),
            });
        }
    }

    for (name, before_sig) in &before_symbols.top_level_members {
        let Some(after_sig) = after_symbols.top_level_members.get(name) else {
            changes.push(ApiChange {
                symbol: name.clone(),
                kind: "top_level".to_string(),
                file: Some(path.to_string()),
                details: format!("removed: {before_sig}"),
            });
            continue;
        };
        if before_sig != after_sig {
            changes.push(ApiChange {
                symbol: name.clone(),
                kind: "top_level".to_string(),
                file: Some(path.to_string()),
                details: format!("changed: {before_sig} -> {after_sig}"),
            });
        }
    }
    for (name, after_sig) in &after_symbols.top_level_members {
        if before_symbols.top_level_members.contains_key(name) {
            continue;
        }
        changes.push(ApiChange {
            symbol: name.clone(),
            kind: "top_level".to_string(),
            file: Some(path.to_string()),
            details: format!("added: {after_sig}"),
        });
    }

    for (owner, before_members) in &before_symbols.companion_members {
        let after_members = after_symbols
            .companion_members
            .get(owner)
            .cloned()
            .unwrap_or_default();
        if *before_members != after_members {
            changes.push(ApiChange {
                symbol: owner.clone(),
                kind: "companion".to_string(),
                file: Some(path.to_string()),
                details: format!(
                    "before [{}] -> after [{}]",
                    before_members
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", "),
                    after_members.iter().cloned().collect::<Vec<_>>().join(", ")
                ),
            });
        }
    }
    for (owner, after_members) in &after_symbols.companion_members {
        if !before_symbols.companion_members.contains_key(owner) {
            changes.push(ApiChange {
                symbol: owner.clone(),
                kind: "companion".to_string(),
                file: Some(path.to_string()),
                details: format!(
                    "added [{}]",
                    after_members.iter().cloned().collect::<Vec<_>>().join(", ")
                ),
            });
        }
    }

    let extension_owners: BTreeSet<String> = before_symbols
        .extension_members
        .keys()
        .chain(after_symbols.extension_members.keys())
        .cloned()
        .collect();
    for owner in extension_owners {
        let before_by_name = before_symbols
            .extension_members
            .get(&owner)
            .cloned()
            .unwrap_or_default();
        let after_by_name = after_symbols
            .extension_members
            .get(&owner)
            .cloned()
            .unwrap_or_default();
        let member_names: BTreeSet<String> = before_by_name
            .keys()
            .chain(after_by_name.keys())
            .cloned()
            .collect();
        for member in member_names {
            match (before_by_name.get(&member), after_by_name.get(&member)) {
                (Some(before_sigs), Some(after_sigs)) if before_sigs != after_sigs => {
                    changes.push(ApiChange {
                        symbol: owner.clone(),
                        kind: "member".to_string(),
                        file: Some(path.to_string()),
                        details: format!("changed `{member}`"),
                    });
                }
                (Some(_), None) => {
                    changes.push(ApiChange {
                        symbol: owner.clone(),
                        kind: "member".to_string(),
                        file: Some(path.to_string()),
                        details: format!("removed `{member}`"),
                    });
                }
                (None, Some(_)) => {
                    changes.push(ApiChange {
                        symbol: owner.clone(),
                        kind: "member".to_string(),
                        file: Some(path.to_string()),
                        details: format!("added `{member}`"),
                    });
                }
                _ => {}
            }
        }
    }

    let all_aliases: BTreeSet<String> = before_symbols
        .typealiases
        .keys()
        .chain(after_symbols.typealiases.keys())
        .cloned()
        .collect();
    for alias in all_aliases {
        match (
            before_symbols.typealiases.get(&alias),
            after_symbols.typealiases.get(&alias),
        ) {
            (Some(before_target), Some(after_target)) if before_target != after_target => {
                changes.push(ApiChange {
                    symbol: alias.clone(),
                    kind: "typealias".to_string(),
                    file: Some(path.to_string()),
                    details: format!("target changed: {before_target} -> {after_target}"),
                });
            }
            (Some(_), None) => changes.push(ApiChange {
                symbol: alias.clone(),
                kind: "typealias".to_string(),
                file: Some(path.to_string()),
                details: "removed".to_string(),
            }),
            (None, Some(target)) => changes.push(ApiChange {
                symbol: alias.clone(),
                kind: "typealias".to_string(),
                file: Some(path.to_string()),
                details: format!("added -> {target}"),
            }),
            _ => {}
        }
    }

    changes
}

fn extract_first_class_symbols(source: Option<&str>) -> FirstClassSymbols {
    let Some(source) = source else {
        return FirstClassSymbols::default();
    };
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_kotlin::language())
        .is_err()
    {
        return FirstClassSymbols::default();
    }
    let Some(tree) = parser.parse(source, None) else {
        return FirstClassSymbols::default();
    };
    let root = tree.root_node();
    let mut symbols = FirstClassSymbols::default();
    collect_top_level_ast(root, source, &mut symbols);
    symbols
}

fn collect_top_level_ast(root: Node<'_>, source: &str, symbols: &mut FirstClassSymbols) {
    let mut cursor = root.walk();
    for node in root.named_children(&mut cursor) {
        match node.kind() {
            "type_alias" => collect_typealias(node, source, symbols),
            "function_declaration" => collect_top_level_callable(node, source, symbols, "fun"),
            "property_declaration" => collect_top_level_property(node, source, symbols),
            "class_declaration" => collect_class_like(node, source, symbols),
            _ => {}
        }
    }
}

fn collect_typealias(node: Node<'_>, source: &str, symbols: &mut FirstClassSymbols) {
    if !is_public_like(node, source) {
        return;
    }
    let Some(name_node) = first_named_child_of_kind(node, "type_identifier") else {
        return;
    };
    let Some(name) = node_text(name_node, source) else {
        return;
    };
    let target = node_text(node, source)
        .and_then(|text| text.split('=').nth(1))
        .map(str::trim)
        .unwrap_or("unknown");
    symbols
        .typealiases
        .insert(name.to_string(), target.to_string());
}

fn collect_top_level_callable(
    node: Node<'_>,
    source: &str,
    symbols: &mut FirstClassSymbols,
    kind: &str,
) {
    if !is_public_like(node, source) {
        return;
    }
    let Some(name_node) = first_named_child_of_kind(node, "simple_identifier") else {
        return;
    };
    let Some(name) = node_text(name_node, source) else {
        return;
    };
    let labels = parameter_labels_from_node(node, source);
    let return_type =
        kotlin_return_type_for_first_class(node, source).unwrap_or("Unit".to_string());
    let signature = format!("{kind} {}({}) -> {return_type}", name, labels.join(","));
    if let Some(receiver) = extension_receiver(node, source, name_node) {
        if !receiver.is_companion {
            symbols
                .extension_members
                .entry(receiver.owner)
                .or_default()
                .entry(name.to_string())
                .or_default()
                .insert(signature);
            return;
        }
    }
    symbols
        .top_level_members
        .insert(name.to_string(), signature);
}

fn collect_top_level_property(node: Node<'_>, source: &str, symbols: &mut FirstClassSymbols) {
    if !is_public_like(node, source) {
        return;
    }
    let Some(var_node) = first_named_child_of_kind(node, "variable_declaration") else {
        return;
    };
    let Some(name_node) = first_named_child_of_kind(var_node, "simple_identifier") else {
        return;
    };
    let Some(name) = node_text(name_node, source) else {
        return;
    };
    let mutable = node_text(node, source)
        .is_some_and(|text| text.contains("var ") || text.trim_start().starts_with("var "));
    let property_type = first_type_like_child(node)
        .and_then(|type_node| node_text(type_node, source))
        .map(clean_type)
        .unwrap_or_else(|| "Any".to_string());
    let signature = format!(
        "{} {}: {}",
        if mutable { "var" } else { "val" },
        name,
        property_type
    );
    if let Some(receiver) = extension_receiver(node, source, name_node) {
        if !receiver.is_companion {
            symbols
                .extension_members
                .entry(receiver.owner)
                .or_default()
                .entry(name.to_string())
                .or_default()
                .insert(signature);
            return;
        }
    }
    symbols
        .top_level_members
        .insert(name.to_string(), signature);
}

struct ExtensionReceiver {
    owner: String,
    is_companion: bool,
}

fn extension_receiver(
    node: Node<'_>,
    source: &str,
    name_node: Node<'_>,
) -> Option<ExtensionReceiver> {
    if let Some(receiver_node) = node
        .child_by_field_name("receiver")
        .or_else(|| node.child_by_field_name("receiver_type"))
    {
        if let Some(expr) = node_text(receiver_node, source).map(str::trim)
            && !expr.is_empty()
        {
            return extension_receiver_from_expr(expr);
        }
    }

    let mut receiver_expr = None::<String>;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.end_byte() > name_node.start_byte() {
            continue;
        }
        if is_type_like(child.kind())
            || child.kind() == "type_identifier"
            || child.kind() == "receiver_type"
        {
            receiver_expr = node_text(child, source).map(str::trim).map(str::to_string);
        }
    }

    let receiver_expr = match receiver_expr {
        Some(expr) if !expr.is_empty() => expr,
        _ => {
            // Fallback for syntaxes where receiver type is not exposed as a direct child.
            let prefix = source.get(node.start_byte()..name_node.start_byte())?;
            parse_extension_receiver_expr_from_prefix(prefix)?
        }
    };

    extension_receiver_from_expr(&receiver_expr)
}

fn parse_extension_receiver_expr_from_prefix(prefix: &str) -> Option<String> {
    let receiver_token = prefix
        .trim_end()
        .trim_end_matches('.')
        .split_whitespace()
        .next_back()?;
    if receiver_token.is_empty() {
        return None;
    }
    let kotlin_decl_keywords = [
        "fun",
        "val",
        "var",
        "public",
        "private",
        "internal",
        "protected",
        "suspend",
        "inline",
        "operator",
        "infix",
        "tailrec",
        "external",
        "expect",
        "actual",
    ];
    if kotlin_decl_keywords.contains(&receiver_token) {
        return None;
    }
    if let Some(member_dot) = receiver_token.rfind('.') {
        let expr = receiver_token[..member_dot].trim();
        if !expr.is_empty() {
            return Some(expr.to_string());
        }
    }
    Some(receiver_token.to_string())
}

fn extension_receiver_from_expr(receiver_expr: &str) -> Option<ExtensionReceiver> {
    let mut segments: Vec<&str> = receiver_expr
        .trim_matches(|ch| ch == '(' || ch == ')')
        .split('.')
        .collect();
    if segments.is_empty() {
        return None;
    }

    let mut is_companion = false;
    if let Some(last) = segments.last()
        && (*last == "Companion" || *last == "companion")
    {
        is_companion = true;
        segments.pop();
    }
    let owner_raw = *segments.last()?;
    let owner = owner_raw
        .split('<')
        .next()
        .unwrap_or(owner_raw)
        .trim()
        .trim_end_matches('?')
        .trim_end_matches('!')
        .to_string();
    if owner.is_empty() {
        return None;
    }
    Some(ExtensionReceiver {
        owner,
        is_companion,
    })
}

fn collect_class_like(node: Node<'_>, source: &str, symbols: &mut FirstClassSymbols) {
    if !is_public_like(node, source) {
        return;
    }
    let Some(name_node) = first_named_child_of_kind(node, "type_identifier") else {
        return;
    };
    let Some(class_name) = node_text(name_node, source).map(str::to_string) else {
        return;
    };

    let header = source
        .get(node.start_byte()..name_node.start_byte())
        .unwrap_or_default();
    let is_enum = header.contains("enum");
    let is_sealed = header.contains("sealed");

    collect_constructors(node, source, symbols, &class_name);
    collect_companion(node, source, symbols, &class_name);
    if is_enum || is_sealed {
        collect_enum_sealed_cases(node, source, symbols, &class_name, is_enum, is_sealed);
    }
}

fn collect_constructors(
    class_node: Node<'_>,
    source: &str,
    symbols: &mut FirstClassSymbols,
    class_name: &str,
) {
    let mut overloads = BTreeSet::new();
    let mut cursor = class_node.walk();
    for child in class_node.named_children(&mut cursor) {
        if child.kind() == "primary_constructor" {
            let labels = parameter_labels_from_node(child, source);
            overloads.insert(labels.join(","));
        }
        if child.kind() == "class_body" {
            let mut body_cursor = child.walk();
            for body_child in child.named_children(&mut body_cursor) {
                if body_child.kind() == "secondary_constructor" {
                    let labels = parameter_labels_from_node(body_child, source);
                    overloads.insert(labels.join(","));
                }
            }
        }
    }
    if !overloads.is_empty() {
        symbols
            .constructors
            .entry(class_name.to_string())
            .or_default()
            .extend(overloads);
    }
}

fn collect_companion(
    class_node: Node<'_>,
    source: &str,
    symbols: &mut FirstClassSymbols,
    class_name: &str,
) {
    let Some(body) = first_named_child_of_kind(class_node, "class_body") else {
        return;
    };
    let mut members = BTreeSet::new();
    let mut cursor = body.walk();
    for node in body.named_children(&mut cursor) {
        if node.kind() != "object_declaration" {
            continue;
        }
        let text = node_text(node, source).unwrap_or_default();
        if !text.contains("companion") {
            continue;
        }
        let mut obj_cursor = node.walk();
        for member in node.named_children(&mut obj_cursor) {
            match member.kind() {
                "function_declaration" => {
                    if let Some(name_node) = first_named_child_of_kind(member, "simple_identifier")
                        && let Some(name) = node_text(name_node, source)
                    {
                        members.insert(name.to_string());
                    }
                }
                "property_declaration" => {
                    if let Some(var_node) =
                        first_named_child_of_kind(member, "variable_declaration")
                        && let Some(name_node) =
                            first_named_child_of_kind(var_node, "simple_identifier")
                        && let Some(name) = node_text(name_node, source)
                    {
                        members.insert(name.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    if !members.is_empty() {
        symbols
            .companion_members
            .insert(class_name.to_string(), members);
    }
}

fn collect_enum_sealed_cases(
    class_node: Node<'_>,
    source: &str,
    symbols: &mut FirstClassSymbols,
    class_name: &str,
    is_enum: bool,
    is_sealed: bool,
) {
    let mut cases = BTreeSet::new();
    for node in descendants_by_kind(class_node, "enum_entry") {
        if !is_enum {
            continue;
        }
        if let Some(name_node) = first_named_child_of_kind(node, "simple_identifier")
            && let Some(name) = node_text(name_node, source)
        {
            cases.insert(name.to_string());
        }
    }

    for node in descendants_by_kind(class_node, "object_declaration") {
        if is_sealed
            && let Some(type_node) = first_named_child_of_kind(node, "type_identifier")
            && let Some(name) = node_text(type_node, source)
        {
            cases.insert(name.to_string());
        }
    }
    for node in descendants_by_kind(class_node, "class_declaration") {
        if is_sealed
            && let Some(type_node) = first_named_child_of_kind(node, "type_identifier")
            && let Some(name) = node_text(type_node, source)
            && name != class_name
        {
            cases.insert(name.to_string());
        }
    }
    if !cases.is_empty() {
        symbols
            .enum_or_sealed_cases
            .insert(class_name.to_string(), cases);
    }
}

fn parameter_labels_from_node(node: Node<'_>, source: &str) -> Vec<String> {
    let mut parameters: Vec<(usize, String)> = Vec::new();
    for kind in [
        "class_parameter",
        "parameter",
        "parameter_with_optional_type",
    ] {
        for param in descendants_by_kind(node, kind) {
            let Some(name_node) = first_named_child_of_kind(param, "simple_identifier") else {
                continue;
            };
            let Some(name) = node_text(name_node, source).map(str::trim) else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            parameters.push((name_node.start_byte(), name.to_string()));
        }
    }

    parameters.sort_by_key(|(start, _)| *start);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (_, name) in parameters {
        if seen.insert(name.clone()) {
            out.push(name);
        }
    }
    out
}

fn is_public_like(node: Node<'_>, source: &str) -> bool {
    let Some(name_node) = declaration_name_node(node) else {
        return true;
    };
    let prefix = source
        .get(node.start_byte()..name_node.start_byte())
        .unwrap_or_default();
    !prefix.contains("private ") && !prefix.contains("internal ") && !prefix.contains("protected ")
}

fn declaration_name_node<'a>(node: Node<'a>) -> Option<Node<'a>> {
    match node.kind() {
        "function_declaration" => first_named_child_of_kind(node, "simple_identifier"),
        "class_declaration" | "object_declaration" | "type_alias" => {
            first_named_child_of_kind(node, "type_identifier")
        }
        "property_declaration" => {
            let var_node = first_named_child_of_kind(node, "variable_declaration")?;
            first_named_child_of_kind(var_node, "simple_identifier")
        }
        _ => first_named_child_of_kind(node, "simple_identifier")
            .or_else(|| first_named_child_of_kind(node, "type_identifier")),
    }
}

fn first_type_like_child<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| is_type_like(child.kind()))
}

fn is_type_like(kind: &str) -> bool {
    matches!(
        kind,
        "user_type"
            | "nullable_type"
            | "not_nullable_type"
            | "type_identifier"
            | "function_type"
            | "parenthesized_type"
    )
}

fn clean_type(raw: &str) -> String {
    raw.split_whitespace().collect::<String>()
}

fn kotlin_return_type_for_first_class(node: Node<'_>, source: &str) -> Option<String> {
    let mut last_type = None;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "function_body" {
            break;
        }
        if is_type_like(child.kind()) {
            last_type = node_text(child, source).map(clean_type);
        }
    }
    last_type
}

fn first_named_child_of_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn descendants_by_kind<'a>(node: Node<'a>, kind: &str) -> Vec<Node<'a>> {
    let mut out = Vec::new();
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        let mut cursor = current.walk();
        for child in current.named_children(&mut cursor) {
            if child.kind() == kind {
                out.push(child);
            }
            stack.push(child);
        }
    }
    out
}

fn node_text<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
    source.get(node.start_byte()..node.end_byte())
}

#[cfg(test)]
mod tests {
    use super::{
        ApiChange, build_ios_impact_diagnostics, change_owner_type_names, diff_first_class_symbols,
        extract_swift_scope_hints_from_contents, kotlin_file_may_be_related,
        parse_extension_receiver_expr_from_prefix, top_level_change_symbols,
    };
    use crate::config::{Config, Severity};
    use crate::ios_usage::{IosUsageHit, IosUsageReport};
    use crate::model::SourceFile;
    use std::collections::HashSet;

    fn sf(path: &str, contents: &str) -> SourceFile {
        SourceFile {
            path: path.to_string(),
            contents: contents.to_string(),
            snapshot: None,
        }
    }

    #[test]
    fn detects_top_level_iosmain_function_signature_change() {
        let before = sf(
            "shared/src/iosMain/kotlin/main.kt",
            r#"
            public fun CustomViewController(onClose: () -> Unit): UIViewController {
                return ComposeUIViewController { CustomViewController(onClose) }
            }
            "#,
        );
        let after = sf(
            "shared/src/iosMain/kotlin/main.kt",
            r#"
            public fun CustomViewController(onClose: () -> Unit, title: String): UIViewController {
                return ComposeUIViewController { CustomViewController(onClose, title) }
            }
            "#,
        );

        let changes = diff_first_class_symbols(Some(&before), Some(&after), &before.path);
        assert!(
            changes
                .iter()
                .any(|c| c.kind == "top_level" && c.symbol == "CustomViewController"),
            "expected top_level change for CustomViewController, got: {changes:#?}"
        );
    }

    #[test]
    fn detects_constructor_change_even_with_private_members_in_body() {
        let before = sf(
            "shared/src/commonMain/kotlin/SomeClass.kt",
            r#"
            public class SomeClass : SomeInterface {
                private val secret = 1
            }
            "#,
        );
        let after = sf(
            "shared/src/commonMain/kotlin/SomeClass.kt",
            r#"
            public class SomeClass(public val name: String) : SomeInterface {
                private val secret = 1
            }
            "#,
        );

        let changes = diff_first_class_symbols(Some(&before), Some(&after), &before.path);
        assert!(
            changes
                .iter()
                .any(|c| c.kind == "constructor" && c.symbol == "SomeClass"),
            "expected constructor change for SomeClass, got: {changes:#?}"
        );
    }

    #[test]
    fn marks_ios_impact_as_info_for_already_touched_swift_file() {
        let config = Config::default();
        let api_changes = vec![ApiChange {
            symbol: "MainKt".to_string(),
            kind: "top_level".to_string(),
            file: Some("shared/src/iosMain/kotlin/main.kt".to_string()),
            details: "changed `CustomViewController`".to_string(),
        }];
        let usage = IosUsageReport {
            swift_files_total: 1,
            candidate_files: 1,
            parsed_files: 1,
            touched_hits: 1,
            untouched_hits: 0,
            hits: vec![IosUsageHit {
                file: "ios/App.swift".to_string(),
                symbol: "MainKt".to_string(),
                kind: "top_level".to_string(),
                evidence: "MainKt, CustomViewController".to_string(),
                already_touched: true,
            }],
        };

        let diags = build_ios_impact_diagnostics(&api_changes, &usage, &config);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Info);
    }

    #[test]
    fn keeps_configured_severity_for_untouched_swift_file() {
        let config = Config::default();
        let api_changes = vec![ApiChange {
            symbol: "MainKt".to_string(),
            kind: "top_level".to_string(),
            file: Some("shared/src/iosMain/kotlin/main.kt".to_string()),
            details: "changed `CustomViewController`".to_string(),
        }];
        let usage = IosUsageReport {
            swift_files_total: 1,
            candidate_files: 1,
            parsed_files: 1,
            touched_hits: 0,
            untouched_hits: 1,
            hits: vec![IosUsageHit {
                file: "ios/App.swift".to_string(),
                symbol: "MainKt".to_string(),
                kind: "top_level".to_string(),
                evidence: "MainKt, CustomViewController".to_string(),
                already_touched: false,
            }],
        };

        let diags = build_ios_impact_diagnostics(&api_changes, &usage, &config);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, Severity::Error);
    }

    #[test]
    fn expands_scope_tokens_for_owner_and_top_level_changes() {
        let changes = vec![
            ApiChange {
                symbol: "com.example.Route".to_string(),
                kind: "member".to_string(),
                file: Some("shared/src/commonMain/kotlin/Route.kt".to_string()),
                details: "changed `push`".to_string(),
            },
            ApiChange {
                symbol: "CustomViewController".to_string(),
                kind: "top_level".to_string(),
                file: Some("shared/src/iosMain/kotlin/main.kt".to_string()),
                details: "changed signature".to_string(),
            },
        ];
        let owners = change_owner_type_names(&changes);
        let top_level = top_level_change_symbols(&changes);

        assert!(owners.contains("Route"));
        assert!(top_level.contains("CustomViewController"));
    }

    #[test]
    fn prefilter_marks_related_kotlin_files() {
        let owners = HashSet::from([String::from("Route")]);
        let top_level = HashSet::from([String::from("CustomViewController")]);
        let swift_hints = HashSet::new();

        let route_file = r#"
            public fun Route.Companion.webViewRoute(url: String): Route = Route("x")
        "#;
        let top_level_file = r#"
            public fun CustomViewController(onClose: () -> Unit): UIViewController = TODO()
        "#;
        let unrelated = r#"
            public class AnalyticsLogger
        "#;

        assert!(kotlin_file_may_be_related(
            route_file,
            &owners,
            &top_level,
            &swift_hints
        ));
        assert!(kotlin_file_may_be_related(
            top_level_file,
            &owners,
            &top_level,
            &swift_hints
        ));
        assert!(!kotlin_file_may_be_related(
            unrelated,
            &owners,
            &top_level,
            &swift_hints
        ));
    }

    #[test]
    fn detects_class_extension_function_change_as_member_change() {
        let before = sf(
            "shared/src/commonMain/kotlin/RouteExtensions.kt",
            r#"
            public class Route
            public fun Route.push(parameters: Map<String, String>): Route = this
            "#,
        );
        let after = sf(
            "shared/src/commonMain/kotlin/RouteExtensions.kt",
            r#"
            public class Route
            public fun Route.push(parameters: Map<String, String>, animated: Boolean): Route = this
            "#,
        );

        let changes = diff_first_class_symbols(Some(&before), Some(&after), &before.path);
        assert!(
            changes
                .iter()
                .any(|c| c.kind == "member" && c.symbol == "Route" && c.details.contains("`push`")),
            "expected member-level change for Route.push extension, got: {changes:#?}"
        );
        assert!(
            changes
                .iter()
                .all(|c| !(c.kind == "top_level" && c.symbol == "push")),
            "extension should not be reported as top_level push: {changes:#?}"
        );
    }

    #[test]
    fn parses_extension_receiver_expr_from_prefix_for_regular_and_companion() {
        let regular = parse_extension_receiver_expr_from_prefix("public fun Route.")
            .expect("regular extension receiver should parse");
        assert_eq!(regular, "Route");

        let companion =
            parse_extension_receiver_expr_from_prefix("public fun com.example.Route.Companion.")
                .expect("companion extension receiver should parse");
        assert_eq!(companion, "com.example.Route");
    }

    #[test]
    fn extracts_swift_scope_hints_for_companion_usage_with_shared_import() {
        let swift = r#"
            import SharedSdk

            func run() {
                _ = Route.companion.push(parameters: [:])
                _ = MainKt.CustomViewController(onClose: {})
            }
        "#;
        let hints = extract_swift_scope_hints_from_contents(swift, "SharedSdk");
        assert!(hints.contains("Route"));
        assert!(hints.contains("push"));
        assert!(hints.contains("MainKt"));
        assert!(hints.contains("CustomViewController"));
    }

    #[test]
    fn does_not_extract_swift_scope_hints_without_shared_import() {
        let swift = r#"
            import Foundation
            func run() { _ = Route.companion.push(parameters: [:]) }
        "#;
        let hints = extract_swift_scope_hints_from_contents(swift, "SharedSdk");
        assert!(hints.is_empty());
    }
}
