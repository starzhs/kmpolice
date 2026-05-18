use std::collections::{BTreeSet, HashSet};
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
            let identifiers = collect_identifiers(tree.root_node(), &file.contents);
            stage_ast.set_message(format!("Swift AST parse | last file: {}", file.path));
            stage_ast.inc(1);
            Some(ParsedSwiftFile {
                path: file.path.clone(),
                identifiers,
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
                if expected.iter().all(|token| file.identifiers.contains(token)) {
                    hits.push(IosUsageHit {
                        file: file.path.clone(),
                        symbol: change.symbol.clone(),
                        kind: change.kind.clone(),
                        evidence: expected.into_iter().collect::<Vec<_>>().join(", "),
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
    already_touched: bool,
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

fn contains_shared_import(contents: &str, shared_sdk_name: &str) -> bool {
    let exact = format!("import {shared_sdk_name}");
    let dotted = format!("import {shared_sdk_name}.");
    contents.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == exact || trimmed.starts_with(&dotted)
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
}
