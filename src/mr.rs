use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::Result;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use tree_sitter::{Node, Parser};

use crate::analyzer::{compare_project, introduced_diagnostics};
use crate::config::Config;
use crate::git::{git_changed_files_between, git_changed_files_worktree, merge_base};
use crate::ios_usage::{find_ios_usages, IosUsageReport};
use crate::model::{
    Contract, ContractKind, Diagnostic, Member, MemberSignature, ProjectSnapshot, SourceFile,
};
use crate::source::{load_from_git_scoped, load_from_worktree_scoped};

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
}

pub fn run_mr(repo: &Path, target: &str, config: &Config, verbose: bool) -> Result<MrResult> {
    let repo = repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf());
    let base = merge_base(&repo, target, "HEAD")?;

    let kotlin_changed = collect_kotlin_changed_paths(&repo, &base)?;
    let ios_scope = None;
    let kotlin_scope = Some(&kotlin_changed);

    let base_snapshot = load_from_git_scoped(&repo, &base, config, kotlin_scope, ios_scope)?;
    let head_snapshot = load_from_worktree_scoped(&repo, config, kotlin_scope, ios_scope)?;

    let base_diags = compare_project(&base_snapshot, config)?;
    let head_diags = compare_project(&head_snapshot, config)?;
    let mut diagnostics = introduced_diagnostics(base_diags, head_diags);

    let api_changes = diff_kotlin_api_changes(&repo, &base, &kotlin_changed, &base_snapshot, &head_snapshot)?;
    let ios_usage = find_ios_usages(&api_changes, &head_snapshot.ios_files)?;
    diagnostics.extend(build_ios_impact_diagnostics(&api_changes, &ios_usage, config));
    if verbose {
        print_verbose_changes(&api_changes);
    }

    Ok(MrResult {
        diagnostics,
        api_changes,
        ios_usage,
    })
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
        out.push(Diagnostic {
            code: code.to_string(),
            severity: config.severity_for(code),
            message: format!(
                "Kotlin API change `{}` for `{}` is used in iOS file `{}`",
                hit.kind, hit.symbol, hit.file
            ),
            hint: format!(
                "change detail: {}; matched tokens: {}; verify/update Swift usage",
                change.details, hit.evidence
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

pub fn run_mock_progress(kotlin_files: usize, ios_files: usize) -> MrResult {
    let multi = MultiProgress::new();
    let kotlin_bar = multi.add(ProgressBar::new(kotlin_files as u64));
    let ios_bar = multi.add(ProgressBar::new(ios_files as u64));
    let total = kotlin_files.saturating_add(ios_files);
    let overall_bar = multi.add(ProgressBar::new(total as u64));

    let style = ProgressStyle::with_template(
        "{spinner:.green} {msg:<44} [{bar:30.cyan/blue}] {pos}/{len} ({percent}%)",
    )
    .expect("progress style should compile")
    .progress_chars("=> ");

    kotlin_bar.set_style(style.clone());
    ios_bar.set_style(style.clone());
    overall_bar.set_style(style);

    kotlin_bar.set_message("kotlin: waiting");
    ios_bar.set_message("ios: waiting");
    overall_bar.set_message("overall");

    let kotlin_worker = {
        let kotlin_bar = kotlin_bar.clone();
        let overall_bar = overall_bar.clone();
        thread::spawn(move || {
            for idx in 0..kotlin_files {
                let file = format!("shared/src/commonMain/kotlin/mock/File{idx}.kt");
                kotlin_bar.set_message(format!("kotlin: {file}"));
                thread::sleep(Duration::from_millis(2));
                kotlin_bar.inc(1);
                overall_bar.inc(1);
            }
            kotlin_bar.finish_with_message("kotlin: done");
        })
    };

    let ios_worker = {
        let ios_bar = ios_bar.clone();
        let overall_bar = overall_bar.clone();
        thread::spawn(move || {
            for idx in 0..ios_files {
                let file = format!("iosApp/iosApp/mock/File{idx}.swift");
                ios_bar.set_message(format!("ios: {file}"));
                thread::sleep(Duration::from_millis(2));
                ios_bar.inc(1);
                overall_bar.inc(1);
            }
            ios_bar.finish_with_message("ios: done");
        })
    };

    let _ = kotlin_worker.join();
    let _ = ios_worker.join();
    overall_bar.finish_with_message("overall: done");

    MrResult {
        diagnostics: Vec::new(),
        api_changes: Vec::new(),
        ios_usage: IosUsageReport::default(),
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
            local.extend(diff_contract_sets(&before_contracts, &after_contracts, path));
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

fn push_unique_change(changes: &mut Vec<ApiChange>, unique: &mut HashSet<String>, change: ApiChange) {
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
        .filter(|contract| matches!(contract.kind, ContractKind::KotlinClass | ContractKind::KotlinInterface))
        .collect())
}

fn diff_contract_sets(before: &[Contract], after: &[Contract], path: &str) -> Vec<ApiChange> {
    let mut out = Vec::new();
    let before_map: HashMap<&str, &Contract> = before.iter().map(|c| (c.fq_name.as_str(), c)).collect();
    let after_map: HashMap<&str, &Contract> = after.iter().map(|c| (c.fq_name.as_str(), c)).collect();

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
    let before_members: HashMap<&str, &Member> =
        before.members.iter().map(|m| (m.name.as_str(), m)).collect();
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
        grouped.entry(change.kind.as_str()).or_default().push(change);
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
        "\n- candidate files: {}",
        report.candidate_files
    ));
    out.push_str(&format!("\n- parsed files: {}", report.parsed_files));
    out.push_str(&format!("\n- matches: {}", report.hits.len()));
    for hit in &report.hits {
        out.push_str(&format!(
            "\n- [{}] {} in {} ({})",
            hit.kind, hit.symbol, hit.file, hit.evidence
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
                    before_overloads.iter().cloned().collect::<Vec<_>>().join(" | "),
                    after_overloads.iter().cloned().collect::<Vec<_>>().join(" | ")
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
                    after_overloads.iter().cloned().collect::<Vec<_>>().join(" | ")
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
                    before_members.iter().cloned().collect::<Vec<_>>().join(", "),
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
    if parser.set_language(&tree_sitter_kotlin::language()).is_err() {
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
    symbols.typealiases.insert(name.to_string(), target.to_string());
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
    let signature = node_text(node, source)
        .map(str::trim)
        .unwrap_or(kind)
        .lines()
        .next()
        .unwrap_or(kind)
        .to_string();
    symbols.top_level_members.insert(name.to_string(), signature);
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
    let signature = node_text(node, source)
        .map(str::trim)
        .unwrap_or("val")
        .lines()
        .next()
        .unwrap_or("val")
        .to_string();
    symbols.top_level_members.insert(name.to_string(), signature);
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
        if child.kind() == "primary_constructor" || child.kind() == "class_parameter_list" {
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
                    if let Some(var_node) = first_named_child_of_kind(member, "variable_declaration")
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
    let text = node_text(node, source).unwrap_or_default();
    text.split(',')
        .filter_map(|raw| {
            let clean = raw.trim().trim_start_matches('(').trim_end_matches(')');
            if clean.is_empty() {
                return None;
            }
            let before_colon = clean.split(':').next().unwrap_or(clean).trim();
            let name = before_colon
                .split_whitespace()
                .last()
                .unwrap_or(before_colon)
                .to_string();
            if name.is_empty() { None } else { Some(name) }
        })
        .collect()
}

fn is_public_like(node: Node<'_>, source: &str) -> bool {
    let text = node_text(node, source).unwrap_or_default();
    !text.contains("private ")
        && !text.contains("internal ")
        && !text.contains("protected ")
}

fn first_named_child_of_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).find(|child| child.kind() == kind)
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
