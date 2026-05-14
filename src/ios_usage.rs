use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use tree_sitter::{Node, Parser};

use crate::model::SourceFile;
use crate::mr::ApiChange;

#[derive(Debug, Clone)]
pub struct IosUsageHit {
    pub file: String,
    pub symbol: String,
    pub kind: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Default)]
pub struct IosUsageReport {
    pub candidate_files: usize,
    pub parsed_files: usize,
    pub hits: Vec<IosUsageHit>,
}

pub fn find_ios_usages(api_changes: &[ApiChange], ios_files: &[SourceFile]) -> Result<IosUsageReport> {
    let index = build_search_index(api_changes);
    if index.tokens.is_empty() {
        return Ok(IosUsageReport::default());
    }

    let mut candidates = Vec::new();
    for file in ios_files {
        if !contains_shared_import(&file.contents) {
            continue;
        }
        if !contains_any_token(&file.contents, &index.tokens) {
            continue;
        }
        candidates.push(file.clone());
    }
    let progress = Arc::new(ProgressBar::new(candidates.len() as u64));
    progress.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} {msg:<72} [{bar:30.cyan/blue}] {pos}/{len} ({percent}%)",
        )
        .expect("progress style")
        .progress_chars("=> "),
    );
    progress.set_message("iOS usage AST | waiting...");

    let hits_by_file: Vec<(bool, Vec<IosUsageHit>)> = candidates
        .par_iter()
        .map(|file| {
            let mut parser = Parser::new();
            if parser.set_language(&tree_sitter_swift::language()).is_err() {
                progress.inc(1);
                return (false, Vec::new());
            }
            let Some(tree) = parser.parse(&file.contents, None) else {
                progress.inc(1);
                return (false, Vec::new());
            };
            let identifiers = collect_identifiers(tree.root_node(), &file.contents);
            let mut hits = Vec::new();
            for change in api_changes {
                let expected = expected_tokens_for_change(change);
                if expected.is_empty() {
                    continue;
                }
                if expected.iter().all(|token| identifiers.contains(token)) {
                    hits.push(IosUsageHit {
                        file: file.path.clone(),
                        symbol: change.symbol.clone(),
                        kind: change.kind.clone(),
                        evidence: expected.into_iter().collect::<Vec<_>>().join(", "),
                    });
                }
            }
            progress.set_message(format!("iOS usage AST | last file: {}", file.path));
            progress.inc(1);
            (true, hits)
        })
        .collect();
    progress.finish_with_message("iOS usage AST done");

    let mut report = IosUsageReport {
        candidate_files: candidates.len(),
        ..IosUsageReport::default()
    };
    for (parsed, hits) in hits_by_file {
        if parsed {
            report.parsed_files += 1;
        }
        report.hits.extend(hits);
    }

    Ok(report)
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

fn contains_shared_import(contents: &str) -> bool {
    contents.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == "import shared" || trimmed.starts_with("import shared.")
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
