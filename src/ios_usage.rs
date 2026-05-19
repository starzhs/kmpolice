use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use tree_sitter::{Node, Parser};

use crate::mr::ApiChange;

#[derive(Debug, Clone)]
pub struct IosUsageHit {
    pub file: String,
    pub symbol: String,
    pub kind: String,
    pub evidence: String,
    pub already_touched: bool,
}

#[derive(Debug, Clone, Default)]
pub struct IosUsageReport {
    pub swift_files_total: usize,
    pub candidate_files: usize,
    pub parsed_files: usize,
    pub touched_hits: usize,
    pub untouched_hits: usize,
    pub hits: Vec<IosUsageHit>,
}

pub fn find_ios_usages(
    api_changes: &[ApiChange],
    repo: &Path,
    ios_paths: &[String],
    shared_sdk_name: &str,
    swift_changed_paths: &HashSet<String>,
) -> Result<IosUsageReport> {
    let index = build_search_index(api_changes);
    if index.tokens.is_empty() {
        return Ok(IosUsageReport {
            swift_files_total: ios_paths.len(),
            ..IosUsageReport::default()
        });
    }

    let multi = MultiProgress::new();
    let style = ProgressStyle::with_template(
        "{spinner:.green} {msg:<72} [{bar:30.cyan/blue}] {pos}/{len} ({percent}%)",
    )
    .expect("progress style")
    .progress_chars("=> ");

    let stage_enum = multi.add(ProgressBar::new(ios_paths.len() as u64));
    stage_enum.set_style(style.clone());
    stage_enum.set_message("Swift enumerate | scanning paths");
    for _ in ios_paths {
        stage_enum.inc(1);
    }
    stage_enum.finish_with_message("Swift enumerate | done");

    let stage_import = Arc::new(multi.add(ProgressBar::new(ios_paths.len() as u64)));
    stage_import.set_style(style.clone());
    stage_import.set_message("Swift import filter | waiting...");
    let import_hits: Vec<CandidateFile> = ios_paths
        .par_iter()
        .filter_map(|path| {
            let full = repo.join(path);
            let contents = match fs::read_to_string(&full) {
                Ok(v) => v,
                Err(_) => {
                    stage_import.inc(1);
                    return None;
                }
            };
            stage_import.set_message(format!("Swift import filter | last file: {path}"));
            stage_import.inc(1);
            if !contains_shared_import(&contents, shared_sdk_name) {
                return None;
            }
            Some(CandidateFile {
                path: path.clone(),
                contents,
                already_touched: swift_changed_paths.contains(path),
            })
        })
        .collect();
    stage_import.finish_with_message("Swift import filter | done");

    let stage_token = Arc::new(multi.add(ProgressBar::new(import_hits.len() as u64)));
    stage_token.set_style(style.clone());
    stage_token.set_message("Swift token filter | waiting...");
    let token_hits: Vec<CandidateFile> = import_hits
        .par_iter()
        .filter_map(|file| {
            stage_token.set_message(format!("Swift token filter | last file: {}", file.path));
            stage_token.inc(1);
            if !contains_any_token(&file.contents, &index.tokens) {
                return None;
            }
            Some(file.clone())
        })
        .collect();
    stage_token.finish_with_message("Swift token filter | done");

    let stage_ast = Arc::new(multi.add(ProgressBar::new(token_hits.len() as u64)));
    stage_ast.set_style(style.clone());
    stage_ast.set_message("Swift AST parse | waiting...");
    let parsed_files: Vec<ParsedSwiftFile> = token_hits
        .par_iter()
        .filter_map(|file| {
            let mut parser = Parser::new();
            if parser.set_language(&tree_sitter_swift::language()).is_err() {
                stage_ast.inc(1);
                return None;
            }
            let Some(tree) = parser.parse(&file.contents, None) else {
                stage_ast.inc(1);
                return None;
            };
            let root = tree.root_node();
            let identifiers = collect_identifiers(root, &file.contents);
            let (bindings, inheritance, member_calls) = collect_swift_semantics(root, &file.contents);
            stage_ast.set_message(format!("Swift AST parse | last file: {}", file.path));
            stage_ast.inc(1);
            Some(ParsedSwiftFile {
                path: file.path.clone(),
                identifiers,
                bindings,
                inheritance,
                member_calls,
                already_touched: file.already_touched,
            })
        })
        .collect();
    stage_ast.finish_with_message("Swift AST parse | done");

    let stage_match = Arc::new(multi.add(ProgressBar::new(parsed_files.len() as u64)));
    stage_match.set_style(style);
    stage_match.set_message("Swift usage match | waiting...");
    let hits_by_file: Vec<Vec<IosUsageHit>> = parsed_files
        .par_iter()
        .map(|file| {
            let mut hits = Vec::new();
            for change in api_changes {
                let expected = expected_tokens_for_change(change);
                if expected.is_empty() {
                    continue;
                }
                let strict_outcome = if change.kind == "member" {
                    strict_member_type_aware_match(change, file)
                } else {
                    None
                };
                let strict_match = strict_outcome
                    .unwrap_or_else(|| expected.iter().all(|token| file.identifiers.contains(token)));
                let member_fallback = member_only_fallback_match(change, &file.identifiers);
                let allow_fallback = change.kind != "member" || strict_outcome.is_none();
                if strict_match || (allow_fallback && member_fallback) {
                    let evidence = if strict_match {
                        if change.kind == "member" {
                            strict_member_evidence(change, file)
                                .unwrap_or_else(|| expected.into_iter().collect::<Vec<_>>().join(", "))
                        } else {
                            expected.into_iter().collect::<Vec<_>>().join(", ")
                        }
                    } else {
                        let member = member_name_from_details(&change.details)
                            .unwrap_or_else(|| "<unknown_member>".to_string());
                        format!("member_only_fallback:{member}")
                    };
                    hits.push(IosUsageHit {
                        file: file.path.clone(),
                        symbol: change.symbol.clone(),
                        kind: change.kind.clone(),
                        evidence,
                        already_touched: file.already_touched,
                    });
                }
            }
            stage_match.set_message(format!("Swift usage match | last file: {}", file.path));
            stage_match.inc(1);
            hits
        })
        .collect();
    stage_match.finish_with_message("Swift usage match | done");

    let mut report = IosUsageReport {
        swift_files_total: ios_paths.len(),
        candidate_files: token_hits.len(),
        parsed_files: parsed_files.len(),
        ..IosUsageReport::default()
    };
    for hits in hits_by_file {
        for hit in hits {
            if hit.already_touched {
                report.touched_hits += 1;
            } else {
                report.untouched_hits += 1;
            }
            report.hits.push(hit);
        }
    }

    Ok(report)
}

#[derive(Debug, Clone)]
struct CandidateFile {
    path: String,
    contents: String,
    already_touched: bool,
}

#[derive(Debug, Clone)]
struct ParsedSwiftFile {
    path: String,
    identifiers: HashSet<String>,
    bindings: HashMap<String, String>,
    inheritance: HashMap<String, HashSet<String>>,
    member_calls: Vec<MemberCall>,
    already_touched: bool,
}

#[derive(Debug, Clone)]
struct MemberCall {
    receiver: String,
    member: String,
}

#[derive(Debug, Clone, Default)]
struct SearchIndex {
    tokens: BTreeSet<String>,
}

fn build_search_index(api_changes: &[ApiChange]) -> SearchIndex {
    let mut tokens = BTreeSet::new();
    for change in api_changes {
        if let Some(root) = root_type_name(&change.symbol) {
            tokens.insert(root);
        }
        if looks_like_identifier(&change.symbol) {
            tokens.insert(change.symbol.clone());
        }
        if let Some(member) = member_name_from_details(&change.details) {
            tokens.insert(member);
        }
    }
    SearchIndex { tokens }
}

fn expected_tokens_for_change(change: &ApiChange) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Some(root) = root_type_name(&change.symbol) {
        out.insert(root);
    }
    if matches!(
        change.kind.as_str(),
        "top_level" | "typealias" | "member" | "companion"
    ) && looks_like_identifier(&change.symbol)
    {
        out.insert(change.symbol.clone());
    }
    if let Some(member) = member_name_from_details(&change.details) {
        out.insert(member);
    }
    out
}

fn member_only_fallback_match(change: &ApiChange, identifiers: &HashSet<String>) -> bool {
    if change.kind != "member" {
        return false;
    }
    let Some(member) = member_name_from_details(&change.details) else {
        return false;
    };
    identifiers.contains(&member)
}

fn contains_shared_import(contents: &str, shared_sdk_name: &str) -> bool {
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

fn contains_any_token(contents: &str, tokens: &BTreeSet<String>) -> bool {
    tokens.iter().any(|token| contains_word(contents, token))
}

fn contains_word(contents: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let bytes = contents.as_bytes();
    let needle = word.as_bytes();
    let mut start = 0usize;
    while let Some(pos) = contents[start..].find(word) {
        let idx = start + pos;
        let left_ok = idx == 0 || !is_ident_char(bytes[idx - 1] as char);
        let end = idx + needle.len();
        let right_ok = end >= bytes.len() || !is_ident_char(bytes[end] as char);
        if left_ok && right_ok {
            return true;
        }
        start = idx + 1;
        if start >= bytes.len() {
            break;
        }
    }
    false
}

fn is_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn collect_identifiers(root: Node<'_>, source: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind().contains("identifier")
            && let Some(text) = source.get(node.start_byte()..node.end_byte())
        {
            let candidate = text.trim();
            if looks_like_identifier(candidate) {
                out.insert(candidate.to_string());
            }
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    out
}

fn collect_swift_semantics(
    root: Node<'_>,
    source: &str,
) -> (HashMap<String, String>, HashMap<String, HashSet<String>>, Vec<MemberCall>) {
    let mut bindings = HashMap::new();
    let mut inheritance = HashMap::<String, HashSet<String>>::new();
    let mut member_calls = Vec::new();
    let mut stack = vec![root];

    while let Some(node) = stack.pop() {
        match node.kind() {
            "parameter" => collect_parameter_binding(node, source, &mut bindings),
            "property_declaration" => collect_property_binding(node, source, &mut bindings),
            "class_declaration" | "protocol_declaration" | "struct_declaration" => {
                collect_decl_inheritance(node, source, &mut inheritance)
            }
            "call_expression" => collect_member_call(node, source, &mut member_calls),
            _ => {}
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            stack.push(child);
        }
    }

    (bindings, inheritance, member_calls)
}

fn collect_parameter_binding(node: Node<'_>, source: &str, out: &mut HashMap<String, String>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(type_node) = node.child_by_field_name("type") else {
        return;
    };
    let Some(name_text) = node_text(name_node, source) else {
        return;
    };
    let Some(name) = last_identifier_segment(name_text) else {
        return;
    };
    let Some(type_text) = node_text(type_node, source) else {
        return;
    };
    let Some(type_name) = canonical_type_name(type_text) else {
        return;
    };
    out.insert(name, type_name);
}

fn collect_property_binding(node: Node<'_>, source: &str, out: &mut HashMap<String, String>) {
    let Some(pattern) = node.child_by_field_name("name") else {
        return;
    };
    let Some(type_node) = first_direct_named_child_of_kind(node, "type_annotation") else {
        return;
    };
    let name_node = pattern
        .child_by_field_name("bound_identifier")
        .or_else(|| pattern.child_by_field_name("name"))
        .unwrap_or(pattern);
    let Some(name_text) = node_text(name_node, source) else {
        return;
    };
    let Some(name) = last_identifier_segment(name_text) else {
        return;
    };
    let Some(type_text) = node_text(type_node, source) else {
        return;
    };
    let Some(type_name) = canonical_type_name(type_text) else {
        return;
    };
    out.insert(name, type_name);
}

fn collect_decl_inheritance(
    node: Node<'_>,
    source: &str,
    out: &mut HashMap<String, HashSet<String>>,
) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let Some(raw_name) = node_text(name_node, source) else {
        return;
    };
    let Some(name) = canonical_type_name(raw_name) else {
        return;
    };

    let mut direct = HashSet::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "inheritance_specifier" {
            continue;
        }
        if let Some(text) = node_text(child, source) {
            let raw = text.trim().trim_start_matches(':');
            for entry in raw.split(',') {
                if let Some(parent) = canonical_type_name(entry) {
                    direct.insert(parent);
                }
            }
        }
    }
    if !direct.is_empty() {
        out.entry(name).or_default().extend(direct);
    }
}

fn collect_member_call(node: Node<'_>, source: &str, out: &mut Vec<MemberCall>) {
    let mut call_suffix = None;
    let mut target_expr = None;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "call_suffix" {
            call_suffix = Some(child);
        } else if target_expr.is_none() {
            target_expr = Some(child);
        }
    }
    let Some(_suffix) = call_suffix else {
        return;
    };
    let Some(target) = target_expr else {
        return;
    };
    if target.kind() != "navigation_expression" {
        return;
    }

    let Some(nav_suffix) = target.child_by_field_name("suffix") else {
        return;
    };
    let Some(member_node) = nav_suffix.child_by_field_name("suffix") else {
        return;
    };
    let Some(member_text) = node_text(member_node, source) else {
        return;
    };
    let Some(member) = last_identifier_segment(member_text) else {
        return;
    };

    let receiver_node = target
        .child_by_field_name("target")
        .or_else(|| {
            let mut target_cursor = target.walk();
            target.named_children(&mut target_cursor).next()
        });
    let Some(receiver_node) = receiver_node else {
        return;
    };
    let Some(receiver_text) = node_text(receiver_node, source) else {
        return;
    };
    let Some(receiver) = last_identifier_segment(receiver_text) else {
        return;
    };

    out.push(MemberCall { receiver, member });
}

fn strict_member_type_aware_match(change: &ApiChange, file: &ParsedSwiftFile) -> Option<bool> {
    if change.kind != "member" {
        return None;
    }
    let owner = root_type_name(&change.symbol)?;
    let member = member_name_from_details(&change.details)?;

    let mut has_relevant_calls = false;
    let mut has_resolved_receiver_type = false;
    for call in &file.member_calls {
        if call.member != member {
            continue;
        }
        has_relevant_calls = true;
        let Some(receiver_type) = resolve_receiver_type(&call.receiver, &file.bindings) else {
            continue;
        };
        has_resolved_receiver_type = true;
        if receiver_type == owner || is_subtype_of(&receiver_type, &owner, &file.inheritance) {
            return Some(true);
        }
    }
    if !has_relevant_calls {
        return None;
    }
    if !has_resolved_receiver_type {
        return None;
    }
    Some(false)
}

fn strict_member_evidence(change: &ApiChange, file: &ParsedSwiftFile) -> Option<String> {
    if change.kind != "member" {
        return None;
    }
    let owner = root_type_name(&change.symbol)?;
    let member = member_name_from_details(&change.details)?;

    for call in &file.member_calls {
        if call.member != member {
            continue;
        }
        let Some(receiver_type) = resolve_receiver_type(&call.receiver, &file.bindings) else {
            continue;
        };
        if receiver_type == owner || is_subtype_of(&receiver_type, &owner, &file.inheritance) {
            return Some(format!(
                "type_aware_call:{}:{} -> {}",
                call.receiver, receiver_type, call.member
            ));
        }
    }
    None
}

fn resolve_receiver_type(receiver: &str, bindings: &HashMap<String, String>) -> Option<String> {
    if let Some(found) = bindings.get(receiver) {
        return Some(found.clone());
    }
    let short = receiver.strip_prefix("self.").unwrap_or(receiver);
    bindings.get(short).cloned()
}

fn is_subtype_of(
    child: &str,
    parent: &str,
    inheritance: &HashMap<String, HashSet<String>>,
) -> bool {
    if child == parent {
        return true;
    }
    let mut queue = VecDeque::new();
    let mut seen = HashSet::new();
    queue.push_back(child.to_string());
    seen.insert(child.to_string());

    while let Some(current) = queue.pop_front() {
        let Some(parents) = inheritance.get(&current) else {
            continue;
        };
        for item in parents {
            if item == parent {
                return true;
            }
            if seen.insert(item.clone()) {
                queue.push_back(item.clone());
            }
        }
    }
    false
}

fn first_direct_named_child_of_kind<'tree>(
    node: Node<'tree>,
    kind: &str,
) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).find(|child| child.kind() == kind)
}

fn canonical_type_name(raw: &str) -> Option<String> {
    let mut text = raw.trim().trim_start_matches(':').trim().to_string();
    if text.is_empty() {
        return None;
    }
    text = text.split_whitespace().collect::<String>();
    if let Some(stripped) = text.strip_prefix("some") {
        text = stripped.to_string();
    } else if let Some(stripped) = text.strip_prefix("any") {
        text = stripped.to_string();
    }
    let mut text = text.trim().trim_end_matches('?').trim_end_matches('!').to_string();
    if let Some((head, _)) = text.split_once('<') {
        text = head.to_string();
    }
    if let Some(last) = text.rsplit('.').next() {
        let candidate = last.trim();
        if looks_like_identifier(candidate) {
            return Some(candidate.to_string());
        }
    }
    None
}

fn last_identifier_segment(raw: &str) -> Option<String> {
    raw.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|part| !part.is_empty())
        .filter(|part| looks_like_identifier(part))
        .next_back()
        .map(str::to_string)
}

fn node_text<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
    source.get(node.start_byte()..node.end_byte())
}

fn root_type_name(symbol: &str) -> Option<String> {
    for part in symbol.split(['.', '$']) {
        let Some(first) = part.chars().next() else {
            continue;
        };
        if first.is_ascii_uppercase() && looks_like_identifier(part) {
            return Some(part.to_string());
        }
    }
    None
}

fn member_name_from_details(details: &str) -> Option<String> {
    if let Some(start) = details.find('`')
        && let Some(end_rel) = details[start + 1..].find('`')
    {
        let value = &details[start + 1..start + 1 + end_rel];
        if looks_like_identifier(value) {
            return Some(value.to_string());
        }
    }
    None
}

fn looks_like_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

#[cfg(test)]
mod tests {
    use super::find_ios_usages;
    use crate::mr::ApiChange;
    use std::collections::HashSet;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn mk_temp_repo() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be monotonic")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("kmpolice-ios-usage-{nanos}"));
        fs::create_dir_all(&dir).expect("temp repo dir should be created");
        dir
    }

    fn write_file(repo: &Path, rel: &str, contents: &str) {
        let full = repo.join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).expect("parent dirs should be created");
        }
        fs::write(full, contents).expect("file should be written");
    }

    #[test]
    fn finds_usage_with_custom_shared_sdk_and_marks_touched() {
        let repo = mk_temp_repo();
        let swift_rel = "ios/App.swift";
        write_file(
            &repo,
            swift_rel,
            r#"
            import Foundation
            import sharedSdk

            func run(a: A) {
                _ = a.doIt()
            }
            "#,
        );

        let changes = vec![ApiChange {
            symbol: "A.B".to_string(),
            kind: "member".to_string(),
            file: Some("shared/src/commonMain/kotlin/A.kt".to_string()),
            details: "changed `doIt`".to_string(),
        }];

        let mut touched = HashSet::new();
        touched.insert(swift_rel.to_string());

        let report = find_ios_usages(&changes, &repo, &[swift_rel.to_string()], "sharedSdk", &touched)
            .expect("ios usage search should succeed");

        assert_eq!(report.swift_files_total, 1);
        assert_eq!(report.candidate_files, 1);
        assert_eq!(report.parsed_files, 1);
        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.touched_hits, 1);
        assert_eq!(report.untouched_hits, 0);
        assert!(report.hits[0].already_touched);
        assert_eq!(report.hits[0].file, swift_rel);
    }

    #[test]
    fn skips_file_without_required_import() {
        let repo = mk_temp_repo();
        let swift_rel = "ios/NoImport.swift";
        write_file(
            &repo,
            swift_rel,
            r#"
            import Foundation
            func run(a: A) {
                _ = a.doIt()
            }
            "#,
        );

        let changes = vec![ApiChange {
            symbol: "A.B".to_string(),
            kind: "member".to_string(),
            file: Some("shared/src/commonMain/kotlin/A.kt".to_string()),
            details: "changed `doIt`".to_string(),
        }];

        let report = find_ios_usages(
            &changes,
            &repo,
            &[swift_rel.to_string()],
            "sharedSdk",
            &HashSet::new(),
        )
        .expect("ios usage search should succeed");

        assert_eq!(report.swift_files_total, 1);
        assert_eq!(report.candidate_files, 0);
        assert_eq!(report.parsed_files, 0);
        assert!(report.hits.is_empty());
    }

    #[test]
    fn reports_untouched_hits_for_non_changed_swift_file() {
        let repo = mk_temp_repo();
        let swift_rel = "ios/Usage.swift";
        write_file(
            &repo,
            swift_rel,
            r#"
            import shared

            func run(value: ProbeType) {
                _ = value.score
            }
            "#,
        );

        let changes = vec![ApiChange {
            symbol: "ProbeType".to_string(),
            kind: "member".to_string(),
            file: Some("shared/src/commonMain/kotlin/ProbeType.kt".to_string()),
            details: "changed `score`".to_string(),
        }];

        let report =
            find_ios_usages(&changes, &repo, &[swift_rel.to_string()], "shared", &HashSet::new())
                .expect("ios usage search should succeed");

        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.touched_hits, 0);
        assert_eq!(report.untouched_hits, 1);
        assert!(!report.hits[0].already_touched);
    }

    #[test]
    fn finds_interface_member_usage_via_implementation_object() {
        let repo = mk_temp_repo();
        let swift_rel = "ios/TracerUsage.swift";
        write_file(
            &repo,
            swift_rel,
            r#"
            import SharedSdk

            func run() {
                TracerImpl.shared.trace()
            }
            "#,
        );

        let changes = vec![ApiChange {
            symbol: "Tracer".to_string(),
            kind: "member".to_string(),
            file: Some("shared/somemodule/src/commonMain/kotlin/Tracer.kt".to_string()),
            details: "changed `trace`".to_string(),
        }];

        let report = find_ios_usages(
            &changes,
            &repo,
            &[swift_rel.to_string()],
            "SharedSdk",
            &HashSet::new(),
        )
        .expect("ios usage search should succeed");

        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.hits[0].symbol, "Tracer");
        assert_eq!(report.hits[0].kind, "member");
        assert!(report.hits[0].evidence.contains("trace"));
    }

    #[test]
    fn finds_interface_member_usage_via_protocol_typed_field() {
        let repo = mk_temp_repo();
        let swift_rel = "ios/TracerConsumer.swift";
        write_file(
            &repo,
            swift_rel,
            r#"
            import SharedSdk

            final class Consumer {
                private let tracer: Tracer

                init(tracer: Tracer) {
                    self.tracer = tracer
                }

                func run() {
                    tracer.trace()
                }
            }
            "#,
        );

        let changes = vec![ApiChange {
            symbol: "com.example.tracing.Tracer".to_string(),
            kind: "member".to_string(),
            file: Some("shared/src/commonMain/kotlin/Tracer.kt".to_string()),
            details: "changed `trace`".to_string(),
        }];

        let report = find_ios_usages(
            &changes,
            &repo,
            &[swift_rel.to_string()],
            "SharedSdk",
            &HashSet::new(),
        )
        .expect("ios usage search should succeed");

        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.hits[0].symbol, "com.example.tracing.Tracer");
        assert_eq!(report.hits[0].kind, "member");
        assert!(report.hits[0].evidence.contains("Tracer"));
        assert!(report.hits[0].evidence.contains("trace"));
    }

    #[test]
    fn supports_testable_import_for_shared_sdk() {
        let repo = mk_temp_repo();
        let swift_rel = "ios/TestableImport.swift";
        write_file(
            &repo,
            swift_rel,
            r#"
            @testable import SharedSdk

            func run(tracer: Tracer) {
                tracer.trace()
            }
            "#,
        );

        let changes = vec![ApiChange {
            symbol: "Tracer".to_string(),
            kind: "member".to_string(),
            file: Some("shared/src/commonMain/kotlin/Tracer.kt".to_string()),
            details: "changed `trace`".to_string(),
        }];

        let report = find_ios_usages(
            &changes,
            &repo,
            &[swift_rel.to_string()],
            "SharedSdk",
            &HashSet::new(),
        )
        .expect("ios usage search should succeed");

        assert_eq!(report.hits.len(), 1);
        assert_eq!(report.hits[0].kind, "member");
    }

    #[test]
    fn finds_member_call_on_receiver_subtype_via_type_aware_matching() {
        let repo = mk_temp_repo();
        let swift_rel = "ios/SubtypeUsage.swift";
        write_file(
            &repo,
            swift_rel,
            r#"
            import SharedSdk

            protocol ParentType {
                func trace()
            }

            final class ChildType: ParentType {
                func trace() {}
            }

            func run(child: ChildType) {
                child.trace()
            }
            "#,
        );

        let changes = vec![ApiChange {
            symbol: "ParentType".to_string(),
            kind: "member".to_string(),
            file: Some("shared/src/commonMain/kotlin/Tracer.kt".to_string()),
            details: "changed `trace`".to_string(),
        }];

        let report = find_ios_usages(
            &changes,
            &repo,
            &[swift_rel.to_string()],
            "SharedSdk",
            &HashSet::new(),
        )
        .expect("ios usage search should succeed");

        assert_eq!(report.hits.len(), 1);
        assert!(
            report.hits[0].evidence.contains("type_aware_call"),
            "expected type-aware evidence, got {:?}",
            report.hits[0].evidence
        );
    }

    #[test]
    fn does_not_match_member_call_when_receiver_type_is_unrelated() {
        let repo = mk_temp_repo();
        let swift_rel = "ios/UnrelatedTypeUsage.swift";
        write_file(
            &repo,
            swift_rel,
            r#"
            import SharedSdk

            struct LocalOnly {
                func trace() {}
            }

            func run(value: LocalOnly) {
                value.trace()
            }
            "#,
        );

        let changes = vec![ApiChange {
            symbol: "Tracer".to_string(),
            kind: "member".to_string(),
            file: Some("shared/src/commonMain/kotlin/Tracer.kt".to_string()),
            details: "changed `trace`".to_string(),
        }];

        let report = find_ios_usages(
            &changes,
            &repo,
            &[swift_rel.to_string()],
            "SharedSdk",
            &HashSet::new(),
        )
        .expect("ios usage search should succeed");

        assert!(report.hits.is_empty(), "unexpected hits: {:?}", report.hits);
    }
}
