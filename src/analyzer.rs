use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use anyhow::Result;
use regex::Regex;

use crate::config::Config;
use crate::model::{
    AnalysisResult, Contract, ContractKind, Diagnostic, Member, MemberSignature, MethodSignature,
    Parameter, ProjectSnapshot, SourceFile, SourceLocation,
};
use crate::parser;

pub fn compare_project(snapshot: &ProjectSnapshot, config: &Config) -> Result<Vec<Diagnostic>> {
    let analysis = parser::analyze(snapshot)?;
    Ok(compare_analysis(&analysis, snapshot, config))
}

pub fn introduced_diagnostics(base: Vec<Diagnostic>, head: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let existing: HashSet<String> = base
        .into_iter()
        .map(|diagnostic| diagnostic.fingerprint())
        .collect();
    head.into_iter()
        .filter(|diagnostic| !existing.contains(&diagnostic.fingerprint()))
        .collect()
}

fn compare_analysis(
    analysis: &AnalysisResult,
    snapshot: &ProjectSnapshot,
    config: &Config,
) -> Vec<Diagnostic> {
    let kotlin_by_name = index_by_name(&analysis.kotlin_contracts);
    let kotlin_by_fq_name = index_by_fq_name(&analysis.kotlin_contracts);
    let swift_protocols = index_by_name(&analysis.swift_protocols);
    let swift_types = index_by_name(&analysis.swift_types);

    let mut diagnostics = Vec::new();

    for kotlin_contract in analysis
        .kotlin_contracts
        .iter()
        .filter(|contract| contract.kind == ContractKind::KotlinInterface)
    {
        let Some(swift_protocol) =
            find_matching_protocol(kotlin_contract, &swift_protocols, config)
        else {
            // Only report missing protocol when an explicit mapping exists.
            // Unmapped Kotlin interfaces may be internal implementation details
            // and should not require a Swift-side contract.
            if has_explicit_mapping(kotlin_contract, config) {
                diagnostics.push(build_diagnostic(
                    "missing_protocol",
                    config,
                    Some(kotlin_contract),
                    None,
                    None,
                    format!(
                        "Swift protocol matching Kotlin contract `{}` was not found",
                        kotlin_contract.fq_name
                    ),
                    format!(
                        "add a Swift protocol for `{}` or define an explicit mapping",
                        kotlin_contract.name
                    ),
                ));
            }
            continue;
        };

        let kotlin_members =
            flattened_members(kotlin_contract, &kotlin_by_name, &kotlin_by_fq_name, false);
        let swift_protocol_members =
            flattened_members(swift_protocol, &swift_protocols, &swift_protocols, false);
        let swift_default_members =
            flattened_members(swift_protocol, &swift_protocols, &swift_protocols, true);

        compare_member_sets(
            kotlin_contract,
            swift_protocol,
            &kotlin_members,
            &swift_protocol_members,
            false,
            config,
            &mut diagnostics,
        );

        let required_members = merge_member_maps(&swift_protocol_members, &swift_default_members);

        for swift_type in analysis.swift_types.iter().filter(|contract| {
            contract
                .conformances
                .iter()
                .any(|conformance| conformance == &swift_protocol.name)
        }) {
            compare_conformance(
                kotlin_contract,
                swift_protocol,
                swift_type,
                &required_members,
                &swift_types,
                config,
                &mut diagnostics,
            );
        }
    }

    diagnostics.extend(compare_class_method_invocations(
        analysis,
        &snapshot.ios_files,
        config,
    ));
    diagnostics.extend(compare_additional_kotlin_api_usages(analysis, snapshot, config));

    diagnostics
        .into_iter()
        .filter(|diagnostic| {
            !config.should_ignore_diagnostic(
                &diagnostic.code,
                diagnostic.kotlin_symbol.as_deref(),
                diagnostic.ios_symbol.as_deref(),
            )
        })
        .collect()
}

#[derive(Debug, Clone)]
struct KotlinProperty {
    property_type: String,
    mutable: bool,
    nullable: bool,
}

#[derive(Debug, Clone)]
struct KotlinApiIndex {
    class_constructors: HashMap<String, Vec<MethodSignature>>,
    class_properties: HashMap<String, HashMap<String, KotlinProperty>>,
    enum_or_sealed_cases: HashMap<String, HashSet<String>>,
    typealiases: HashSet<String>,
    declared_types: HashSet<String>,
    companion_members: HashMap<String, HashSet<String>>,
    top_level_members_by_kt_type: HashMap<String, HashSet<String>>,
}

fn compare_additional_kotlin_api_usages(
    analysis: &AnalysisResult,
    snapshot: &ProjectSnapshot,
    config: &Config,
) -> Vec<Diagnostic> {
    let index = build_kotlin_api_index(analysis, snapshot);
    let mut diagnostics = Vec::new();

    diagnostics.extend(check_constructor_calls(snapshot, &index, config));
    diagnostics.extend(check_property_usages(snapshot, &index, config));
    diagnostics.extend(check_enum_and_sealed_switches(snapshot, &index, config));
    diagnostics.extend(check_typealias_and_nested_type_usages(snapshot, &index, config));
    diagnostics.extend(check_top_level_usages(snapshot, &index, config));
    diagnostics.extend(check_companion_member_usages(snapshot, &index, config));

    diagnostics
}

fn has_explicit_mapping(kotlin_contract: &Contract, config: &Config) -> bool {
    config
        .mapped_ios_name(&kotlin_contract.fq_name)
        .or_else(|| config.mapped_ios_name(&kotlin_contract.name))
        .is_some()
}

fn compare_class_method_invocations(
    analysis: &AnalysisResult,
    ios_files: &[SourceFile],
    config: &Config,
) -> Vec<Diagnostic> {
    let classes: HashMap<&str, &Contract> = analysis
        .kotlin_contracts
        .iter()
        .filter(|contract| contract.kind == ContractKind::KotlinClass)
        .map(|contract| (contract.name.as_str(), contract))
        .collect();
    if classes.is_empty() {
        return Vec::new();
    }

    let declaration_regex = Regex::new(r"\b(?:let|var)\s+([A-Za-z_]\w*)\s*:\s*([A-Za-z_]\w*)")
        .expect("declaration regex should compile");
    let call_regex = Regex::new(r"\b([A-Za-z_]\w*)\.([A-Za-z_]\w*)\s*\(([^)]*)\)")
        .expect("call regex should compile");

    let mut diagnostics = Vec::new();

    for file in ios_files {
        let variable_types = collect_typed_variables(&file.contents, &declaration_regex);
        for captures in call_regex.captures_iter(&file.contents) {
            let (Some(receiver), Some(method_name), Some(arguments)) =
                (captures.get(1), captures.get(2), captures.get(3))
            else {
                continue;
            };

            let Some(receiver_type) = variable_types.get(receiver.as_str()) else {
                continue;
            };
            let Some(kotlin_class) = classes.get(receiver_type.as_str()) else {
                continue;
            };
            let kotlin_method_members: Vec<&Member> = kotlin_class
                .members
                .iter()
                .filter(|member| {
                    member.name == method_name.as_str()
                        && matches!(member.signature, MemberSignature::Method(_))
                })
                .collect();
            if kotlin_method_members.is_empty() {
                continue;
            }

            let swift_argument_count = count_swift_arguments(arguments.as_str());
            let swift_labels = extract_swift_argument_labels(arguments.as_str());
            let method_signatures: Vec<&MethodSignature> = kotlin_method_members
                .iter()
                .filter_map(|member| match &member.signature {
                    MemberSignature::Method(method) => Some(method),
                    _ => None,
                })
                .collect();
            let expected_counts: Vec<usize> =
                method_signatures.iter().map(|method| method.parameters.len()).collect();

            if expected_counts
                .iter()
                .any(|count| *count == swift_argument_count)
            {
                let same_arity: Vec<&MethodSignature> = method_signatures
                    .iter()
                    .copied()
                    .filter(|method| method.parameters.len() == swift_argument_count)
                    .collect();

                let has_name_match = same_arity.iter().any(|method| {
                    method
                        .parameters
                        .iter()
                        .zip(swift_labels.iter())
                        .all(|(parameter, label)| label.as_deref() == Some(parameter.display_name.as_str()))
                });
                if has_name_match {
                    continue;
                }

                let expected_labels = same_arity
                    .iter()
                    .map(|method| {
                        method
                            .parameters
                            .iter()
                            .map(|parameter| parameter.display_name.clone())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .collect::<Vec<_>>()
                    .join(" | ");
                let actual_labels = format_actual_labels_with_mismatches(
                    &same_arity
                        .first()
                        .map(|method| {
                            method
                                .parameters
                                .iter()
                                .map(|parameter| parameter.display_name.clone())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                    &swift_labels,
                );
                diagnostics.push(build_diagnostic_with_locations(
                    "class_method_parameter_name_mismatch",
                    config,
                    Some(kotlin_class),
                    None,
                    Some(method_name.as_str()),
                    Some(
                        kotlin_method_members
                            .first()
                            .map(|member| member.location.clone())
                            .unwrap_or_else(|| kotlin_class.location.clone()),
                    ),
                    Some(offset_to_location(file, receiver.start())),
                    format!(
                        "Swift call `{}` on `{}` uses parameter labels that do not match Kotlin signature",
                        method_name.as_str(),
                        receiver_type
                    ),
                    format!("expected labels: {expected_labels}; actual labels: {actual_labels}"),
                ));
                continue;
            }
            let expected_label_sets = method_signatures
                .iter()
                .map(|method| {
                    method
                        .parameters
                        .iter()
                        .map(|parameter| parameter.display_name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .collect::<Vec<_>>()
                .join(" | ");
            let actual_labels = swift_labels
                .iter()
                .map(|label| label.clone().unwrap_or_else(|| "<unlabeled>".to_string()))
                .collect::<Vec<_>>()
                .join(", ");
            let expected_count_phrase = if expected_counts.len() == 1 {
                format!("{} argument(s)", expected_counts[0])
            } else {
                format!(
                    "one of [{}] argument counts",
                    expected_counts
                        .iter()
                        .map(|count| count.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let kotlin_location = kotlin_method_members
                .first()
                .map(|member| member.location.clone())
                .unwrap_or_else(|| kotlin_class.location.clone());

            diagnostics.push(build_diagnostic_with_locations(
                "class_method_parameter_count_mismatch",
                config,
                Some(kotlin_class),
                None,
                Some(method_name.as_str()),
                Some(kotlin_location),
                Some(offset_to_location(file, receiver.start())),
                format!(
                    "Swift call `{}` on `{}` passes {} argument(s), but Kotlin method expects {}",
                    method_name.as_str(),
                    receiver_type,
                    swift_argument_count,
                    expected_count_phrase
                ),
                format!(
                    "expected overload labels: {}; actual labels: {}",
                    expected_label_sets, actual_labels
                ),
            ));
        }
    }

    diagnostics
}

fn collect_typed_variables(contents: &str, declaration_regex: &Regex) -> HashMap<String, String> {
    let mut variables = HashMap::new();
    for captures in declaration_regex.captures_iter(contents) {
        if let (Some(name), Some(type_name)) = (captures.get(1), captures.get(2)) {
            variables.insert(name.as_str().to_string(), type_name.as_str().to_string());
        }
    }
    variables
}

fn build_kotlin_api_index(analysis: &AnalysisResult, snapshot: &ProjectSnapshot) -> KotlinApiIndex {
    let mut class_constructors = HashMap::<String, Vec<MethodSignature>>::new();
    let mut class_properties = HashMap::<String, HashMap<String, KotlinProperty>>::new();
    let mut enum_or_sealed_cases = HashMap::<String, HashSet<String>>::new();
    let mut typealiases = HashSet::<String>::new();
    let mut declared_types = HashSet::<String>::new();
    let mut companion_members = HashMap::<String, HashSet<String>>::new();
    let mut top_level_members_by_kt_type = HashMap::<String, HashSet<String>>::new();

    let class_header_regex =
        Regex::new(r"(?m)^\s*(?:public\s+)?class\s+([A-Za-z_]\w*)\s*(\(([^)]*)\))?")
            .expect("class regex");
    let enum_regex =
        Regex::new(r"(?s)enum\s+class\s+([A-Za-z_]\w*)\s*\{([^}]*)\}").expect("enum regex");
    let sealed_regex =
        Regex::new(r"(?s)sealed\s+class\s+([A-Za-z_]\w*)\s*\{([^}]*)\}").expect("sealed regex");
    let typealias_regex =
        Regex::new(r"(?m)^\s*(?:public\s+)?typealias\s+([A-Za-z_]\w*)\s*=").expect("alias regex");
    let declared_type_regex = Regex::new(
        r"(?m)^\s*(?:public\s+)?(?:data\s+class|class|interface|object|enum\s+class|sealed\s+class)\s+([A-Za-z_]\w*)",
    )
    .expect("declared type regex");
    let top_level_member_regex =
        Regex::new(r"(?m)^\s*(?:public\s+)?(?:fun|val)\s+([A-Za-z_]\w*)").expect("top-level regex");
    let companion_block_regex = Regex::new(
        r"(?s)class\s+([A-Za-z_]\w*)[^{]*\{.*?companion\s+object[^{]*\{(.*?)\}",
    )
    .expect("companion regex");
    let companion_member_regex =
        Regex::new(r"(?m)^\s*(?:public\s+)?(?:fun|const\s+val|val|var)\s+([A-Za-z_]\w*)")
            .expect("companion member regex");

    for file in &snapshot.kotlin_files {
        for captures in class_header_regex.captures_iter(&file.contents) {
            let class_name = captures[1].to_string();
            declared_types.insert(class_name.clone());
            let raw_params = captures.get(3).map(|m| m.as_str()).unwrap_or("");
            class_constructors
                .entry(class_name.clone())
                .or_default()
                .push(MethodSignature {
                    parameters: parse_kotlin_parameter_list(raw_params),
                    return_type: "Unit".to_string(),
                });
        }

        for captures in enum_regex.captures_iter(&file.contents) {
            let type_name = captures[1].to_string();
            declared_types.insert(type_name.clone());
            let mut cases = HashSet::new();
            for token in captures[2].split(',') {
                let raw = token.trim();
                if raw.is_empty() {
                    continue;
                }
                let name = raw
                    .split_whitespace()
                    .next()
                    .unwrap_or(raw)
                    .trim_matches('{')
                    .trim_matches('(')
                    .to_string();
                if !name.is_empty() {
                    cases.insert(to_swift_case_name(&name));
                }
            }
            enum_or_sealed_cases.insert(type_name, cases);
        }

        for captures in sealed_regex.captures_iter(&file.contents) {
            let type_name = captures[1].to_string();
            declared_types.insert(type_name.clone());
            let mut cases = HashSet::new();
            for line in captures[2].lines() {
                let trimmed = line.trim();
                let prefix = if trimmed.starts_with("object ") {
                    "object "
                } else if trimmed.starts_with("data object ") {
                    "data object "
                } else if trimmed.starts_with("class ") {
                    "class "
                } else if trimmed.starts_with("data class ") {
                    "data class "
                } else {
                    continue;
                };
                let candidate = trimmed
                    .trim_start_matches(prefix)
                    .split(|ch: char| ch == '(' || ch == ':' || ch.is_whitespace())
                    .next()
                    .unwrap_or("");
                if !candidate.is_empty() {
                    cases.insert(to_swift_case_name(candidate));
                }
            }
            if !cases.is_empty() {
                enum_or_sealed_cases.insert(type_name, cases);
            }
        }

        for captures in typealias_regex.captures_iter(&file.contents) {
            typealiases.insert(captures[1].to_string());
            declared_types.insert(captures[1].to_string());
        }

        for captures in declared_type_regex.captures_iter(&file.contents) {
            declared_types.insert(captures[1].to_string());
        }

        let kt_type = format!("{}Kt", kotlin_file_stem_to_type_name(&file.path));
        let members = top_level_members_by_kt_type.entry(kt_type).or_default();
        for captures in top_level_member_regex.captures_iter(&file.contents) {
            members.insert(captures[1].to_string());
        }

        for captures in companion_block_regex.captures_iter(&file.contents) {
            let class_name = captures[1].to_string();
            let members = companion_members.entry(class_name).or_default();
            for member_capture in companion_member_regex.captures_iter(&captures[2]) {
                members.insert(member_capture[1].to_string());
            }
        }
    }

    for contract in analysis
        .kotlin_contracts
        .iter()
        .filter(|contract| contract.kind == ContractKind::KotlinClass)
    {
        for member in &contract.members {
            if let MemberSignature::Property(property) = &member.signature {
                class_properties
                    .entry(contract.name.clone())
                    .or_default()
                    .insert(
                        member.name.clone(),
                        KotlinProperty {
                            property_type: property.property_type.clone(),
                            mutable: property.mutable,
                            nullable: property.property_type.contains('?'),
                        },
                    );
            }
        }
    }

    KotlinApiIndex {
        class_constructors,
        class_properties,
        enum_or_sealed_cases,
        typealiases,
        declared_types,
        companion_members,
        top_level_members_by_kt_type,
    }
}

fn parse_kotlin_parameter_list(raw: &str) -> Vec<Parameter> {
    split_top_level_args(raw)
        .into_iter()
        .filter_map(|raw_param| {
            let before_colon = split_top_level_once(raw_param.trim(), ':')?.0.trim();
            let parameter_type = split_top_level_once(raw_param.trim(), ':')
                .map(|(_, t)| t.trim().to_string())
                .unwrap_or_else(|| "Any".to_string());
            let name = before_colon
                .split_whitespace()
                .last()
                .unwrap_or(before_colon)
                .to_string();
            if name.is_empty() {
                return None;
            }
            Some(Parameter {
                display_name: name.clone(),
                internal_name: name,
                parameter_type: parameter_type.clone(),
                optional: parameter_type.contains('?'),
                has_default: raw_param.contains('='),
            })
        })
        .collect()
}

fn kotlin_file_stem_to_type_name(path: &str) -> String {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    let stem = file_name.strip_suffix(".kt").unwrap_or(file_name);
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.collect::<String>()),
        None => stem.to_string(),
    }
}

fn to_swift_case_name(raw: &str) -> String {
    if raw
        .chars()
        .all(|character| character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_')
    {
        return raw.to_ascii_lowercase().replace('_', "");
    }
    let mut chars = raw.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_lowercase(), chars.collect::<String>()),
        None => raw.to_string(),
    }
}

fn check_constructor_calls(
    snapshot: &ProjectSnapshot,
    index: &KotlinApiIndex,
    config: &Config,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let constructor_call_regex =
        Regex::new(r"\b([A-Z][A-Za-z_0-9]*)\s*\(([^)]*)\)").expect("ctor call regex");

    for file in &snapshot.ios_files {
        for captures in constructor_call_regex.captures_iter(&file.contents) {
            let type_name = captures[1].to_string();
            let Some(constructors) = index.class_constructors.get(&type_name) else {
                continue;
            };
            let args = captures.get(2).map(|m| m.as_str()).unwrap_or("");
            let labels = extract_swift_argument_labels(args);
            let count = labels.len();
            let arities: Vec<usize> = constructors.iter().map(|c| c.parameters.len()).collect();
            if arities.contains(&count) {
                continue;
            }
            diagnostics.push(Diagnostic {
                code: "class_constructor_parameter_count_mismatch".to_string(),
                severity: config.severity_for("class_constructor_parameter_count_mismatch"),
                message: format!(
                    "Swift constructor call `{}` passes {} argument(s), but Kotlin constructor expects {}",
                    type_name,
                    count,
                    arities
                        .iter()
                        .map(|v| v.to_string())
                        .collect::<Vec<_>>()
                        .join(" or ")
                ),
                hint: format!(
                    "expected labels by overload: {}; actual labels: {}",
                    constructors
                        .iter()
                        .map(|c| c
                            .parameters
                            .iter()
                            .map(|p| p.display_name.clone())
                            .collect::<Vec<_>>()
                            .join(", "))
                        .collect::<Vec<_>>()
                        .join(" | "),
                    labels
                        .iter()
                        .map(|label| label.clone().unwrap_or_else(|| "<unlabeled>".to_string()))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                kotlin_symbol: Some(type_name.clone()),
                ios_symbol: None,
                member: Some("constructor".to_string()),
                kotlin_location: None,
                ios_location: captures
                    .get(1)
                    .map(|m| offset_to_location(file, m.start())),
                base_ref: None,
                head_ref: None,
                evidence: vec![
                    "kotlin_change:constructor_signature".to_string(),
                    "swift_usage_found:constructor_call".to_string(),
                ],
            });
        }
    }

    diagnostics
}

fn check_property_usages(
    snapshot: &ProjectSnapshot,
    index: &KotlinApiIndex,
    config: &Config,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let declaration_regex = Regex::new(r"\b(?:let|var)\s+([A-Za-z_]\w*)\s*:\s*([A-Za-z_]\w*)")
        .expect("declaration regex");
    let property_access_regex =
        Regex::new(r"\b([A-Za-z_]\w*)\.([A-Za-z_]\w*)\b").expect("property access regex");
    let assignment_regex = Regex::new(r"\b([A-Za-z_]\w*)\.([A-Za-z_]\w*)\s*=\s*(.+)")
        .expect("assignment regex");
    let typed_read_regex =
        Regex::new(r"\b(?:let|var)\s+\w+\s*:\s*([A-Za-z_][\w<>?]*)\s*=\s*([A-Za-z_]\w*)\.([A-Za-z_]\w*)")
            .expect("typed read regex");

    for file in &snapshot.ios_files {
        let variable_types = collect_typed_variables(&file.contents, &declaration_regex);

        for captures in property_access_regex.captures_iter(&file.contents) {
            let Some(full_match) = captures.get(0) else {
                continue;
            };
            let tail = &file.contents[full_match.end()..];
            if tail.trim_start().starts_with('(') {
                continue;
            }
            let receiver = captures[1].to_string();
            let property = captures[2].to_string();
            let Some(class_name) = variable_types.get(&receiver) else {
                continue;
            };
            let Some(properties) = index.class_properties.get(class_name) else {
                continue;
            };
            if properties.contains_key(&property) {
                continue;
            }
            diagnostics.push(Diagnostic {
                code: "class_property_missing".to_string(),
                severity: config.severity_for("class_property_missing"),
                message: format!(
                    "Swift access `{}.{}` refers to a missing Kotlin property",
                    receiver, property
                ),
                hint: format!("expected one of: {}", properties.keys().cloned().collect::<Vec<_>>().join(", ")),
                kotlin_symbol: Some(class_name.clone()),
                ios_symbol: None,
                member: Some(property),
                kotlin_location: None,
                ios_location: captures.get(1).map(|m| offset_to_location(file, m.start())),
                base_ref: None,
                head_ref: None,
                evidence: vec![
                    "kotlin_change:property_removed_or_renamed".to_string(),
                    "swift_usage_found:property_access".to_string(),
                ],
            });
        }

        for captures in assignment_regex.captures_iter(&file.contents) {
            let receiver = captures[1].to_string();
            let property = captures[2].to_string();
            let value_expr = captures[3].trim();
            let Some(class_name) = variable_types.get(&receiver) else {
                continue;
            };
            let Some(property_info) = index
                .class_properties
                .get(class_name)
                .and_then(|props| props.get(&property))
            else {
                continue;
            };
            if !property_info.mutable {
                diagnostics.push(Diagnostic {
                    code: "class_property_mutability_mismatch".to_string(),
                    severity: config.severity_for("class_property_mutability_mismatch"),
                    message: format!(
                        "Swift assignment to `{}.{}` targets immutable Kotlin property",
                        receiver, property
                    ),
                    hint: "expected mutable=true; actual mutable=false".to_string(),
                    kotlin_symbol: Some(class_name.clone()),
                    ios_symbol: None,
                    member: Some(property.clone()),
                    kotlin_location: None,
                    ios_location: captures.get(1).map(|m| offset_to_location(file, m.start())),
                    base_ref: None,
                    head_ref: None,
                    evidence: vec![
                        "kotlin_change:property_mutability".to_string(),
                        "swift_usage_found:property_assignment".to_string(),
                    ],
                });
            }
            if value_expr == "nil" && !property_info.nullable {
                diagnostics.push(Diagnostic {
                    code: "class_property_nullability_mismatch".to_string(),
                    severity: config.severity_for("class_property_nullability_mismatch"),
                    message: format!(
                        "Swift assigns `nil` to non-null Kotlin property `{}.{}`",
                        receiver, property
                    ),
                    hint: format!(
                        "expected nullable=true; actual nullable=false for type `{}`",
                        property_info.property_type
                    ),
                    kotlin_symbol: Some(class_name.clone()),
                    ios_symbol: None,
                    member: Some(property),
                    kotlin_location: None,
                    ios_location: captures.get(1).map(|m| offset_to_location(file, m.start())),
                    base_ref: None,
                    head_ref: None,
                    evidence: vec![
                        "kotlin_change:property_nullability".to_string(),
                        "swift_usage_found:nil_assignment".to_string(),
                    ],
                });
            }
        }

        for captures in typed_read_regex.captures_iter(&file.contents) {
            let expected_type = captures[1].to_string();
            let receiver = captures[2].to_string();
            let property = captures[3].to_string();
            let Some(class_name) = variable_types.get(&receiver) else {
                continue;
            };
            let Some(property_info) = index
                .class_properties
                .get(class_name)
                .and_then(|props| props.get(&property))
            else {
                continue;
            };
            if canonical_type(&expected_type) != canonical_type(&property_info.property_type) {
                diagnostics.push(Diagnostic {
                    code: "class_property_type_mismatch".to_string(),
                    severity: config.severity_for("class_property_type_mismatch"),
                    message: format!(
                        "Swift typed read for `{}.{}` expects `{}`, but Kotlin property type is `{}`",
                        receiver, property, expected_type, property_info.property_type
                    ),
                    hint: format!(
                        "expected `{}`; actual `{}`",
                        property_info.property_type, expected_type
                    ),
                    kotlin_symbol: Some(class_name.clone()),
                    ios_symbol: None,
                    member: Some(property),
                    kotlin_location: None,
                    ios_location: captures.get(2).map(|m| offset_to_location(file, m.start())),
                    base_ref: None,
                    head_ref: None,
                    evidence: vec![
                        "kotlin_change:property_type".to_string(),
                        "swift_usage_found:typed_property_read".to_string(),
                    ],
                });
            }
        }
    }

    diagnostics
}

fn check_enum_and_sealed_switches(
    snapshot: &ProjectSnapshot,
    index: &KotlinApiIndex,
    config: &Config,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let declaration_regex = Regex::new(r"\b(?:let|var)\s+([A-Za-z_]\w*)\s*:\s*([A-Za-z_]\w*)")
        .expect("declaration regex");
    let switch_on_enum_regex =
        Regex::new(r"(?s)switch\s+onEnum\s*\(\s*of:\s*([A-Za-z_]\w*)\s*\)\s*\{(.*?)\}")
        .expect("switch regex");
    let switch_plain_regex =
        Regex::new(r"(?s)switch\s+([A-Za-z_]\w*)\s*\{(.*?)\}").expect("plain switch regex");
    let case_regex = Regex::new(r"(?m)case\s+\.([A-Za-z_]\w*)").expect("case regex");
    let if_is_regex = Regex::new(r"\bif\s+([A-Za-z_]\w*)\s+is\s+([A-Za-z_]\w*)")
        .expect("if-is regex");

    for file in &snapshot.ios_files {
        let variable_types = collect_typed_variables(&file.contents, &declaration_regex);
        for captures in switch_on_enum_regex.captures_iter(&file.contents) {
            let variable = captures[1].to_string();
            let Some(type_name) = variable_types.get(&variable) else {
                continue;
            };
            let Some(expected_cases) = index.enum_or_sealed_cases.get(type_name).cloned() else {
                continue;
            };
            let mut actual_cases = HashSet::new();
            for case_capture in case_regex.captures_iter(&captures[2]) {
                actual_cases.insert(case_capture[1].to_string());
            }
            push_missing_case_diagnostic(
                &mut diagnostics,
                config,
                file,
                captures.get(1).map(|m| m.start()),
                type_name,
                &expected_cases,
                actual_cases,
                "swift_usage_found:switch_on_enum",
            );
        }

        for captures in switch_plain_regex.captures_iter(&file.contents) {
            let variable = captures[1].to_string();
            let Some(type_name) = variable_types.get(&variable) else {
                continue;
            };
            let Some(expected_cases) = index.enum_or_sealed_cases.get(type_name).cloned() else {
                continue;
            };
            let mut actual_cases = HashSet::new();
            for case_capture in case_regex.captures_iter(&captures[2]) {
                actual_cases.insert(case_capture[1].to_string());
            }
            push_missing_case_diagnostic(
                &mut diagnostics,
                config,
                file,
                captures.get(1).map(|m| m.start()),
                type_name,
                &expected_cases,
                actual_cases,
                "swift_usage_found:plain_switch",
            );
        }

        let mut sealed_case_hits: HashMap<(String, String), HashSet<String>> = HashMap::new();
        for captures in if_is_regex.captures_iter(&file.contents) {
            let variable = captures[1].to_string();
            let swift_type_name = captures[2].to_string();
            let Some(type_name) = variable_types.get(&variable) else {
                continue;
            };
            let Some(expected_cases) = index.enum_or_sealed_cases.get(type_name) else {
                continue;
            };
            let inferred_case = infer_case_name_from_swift_type(type_name, &swift_type_name);
            let Some(case_name) = inferred_case else {
                continue;
            };
            if expected_cases.contains(&case_name) {
                sealed_case_hits
                    .entry((variable.clone(), type_name.clone()))
                    .or_default()
                    .insert(case_name);
            }
        }

        for ((variable, type_name), actual_cases) in sealed_case_hits {
            let Some(expected_cases) = index.enum_or_sealed_cases.get(&type_name).cloned() else {
                continue;
            };
            let location_offset = file.contents.find(&variable);
            push_missing_case_diagnostic(
                &mut diagnostics,
                config,
                file,
                location_offset,
                &type_name,
                &expected_cases,
                actual_cases,
                "swift_usage_found:if_is_chain",
            );
        }
    }
    diagnostics
}

fn infer_case_name_from_swift_type(base_type: &str, swift_type_name: &str) -> Option<String> {
    if let Some(stripped) = swift_type_name.strip_prefix(base_type) {
        if !stripped.is_empty() {
            return Some(to_swift_case_name(stripped));
        }
    }
    None
}

fn push_missing_case_diagnostic(
    diagnostics: &mut Vec<Diagnostic>,
    config: &Config,
    file: &SourceFile,
    location_offset: Option<usize>,
    type_name: &str,
    expected_cases: &HashSet<String>,
    actual_cases: HashSet<String>,
    evidence_usage: &str,
) {
    let missing: Vec<String> = expected_cases
        .iter()
        .filter(|expected| !actual_cases.contains(*expected))
        .cloned()
        .collect();
    if missing.is_empty() {
        return;
    }

    diagnostics.push(Diagnostic {
        code: "enum_or_sealed_switch_missing_cases".to_string(),
        severity: config.severity_for("enum_or_sealed_switch_missing_cases"),
        message: format!(
            "Swift switch for `{}` misses {} Kotlin case(s)",
            type_name,
            missing.len()
        ),
        hint: format!(
            "missing cases: {}; covered cases: {}",
            missing.join(", "),
            actual_cases.into_iter().collect::<Vec<_>>().join(", ")
        ),
        kotlin_symbol: Some(type_name.to_string()),
        ios_symbol: None,
        member: None,
        kotlin_location: None,
        ios_location: location_offset.map(|offset| offset_to_location(file, offset)),
        base_ref: None,
        head_ref: None,
        evidence: vec![
            "kotlin_change:enum_or_sealed_cases".to_string(),
            evidence_usage.to_string(),
        ],
    });
}

fn check_typealias_and_nested_type_usages(
    snapshot: &ProjectSnapshot,
    index: &KotlinApiIndex,
    config: &Config,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let typed_decl_regex = Regex::new(r"\b(?:let|var)\s+\w+\s*:\s*([A-Z][A-Za-z_0-9]*)")
        .expect("typed decl regex");
    let swift_type_decl_regex =
        Regex::new(r"(?m)^\s*(?:struct|class|protocol|enum)\s+[A-Za-z_]\w*(?:\s*<([^>]+)>)?")
            .expect("swift decl regex");
    let swift_common_types: HashSet<&str> = [
        "String", "Int", "Bool", "Double", "Float", "Void", "Any", "Data", "URL",
    ]
    .into_iter()
    .collect();
    let mut local_swift_types = HashSet::<String>::new();
    let mut local_generic_names = HashSet::<String>::new();
    for file in &snapshot.ios_files {
        for captures in swift_type_decl_regex.captures_iter(&file.contents) {
            if let Some(generics) = captures.get(1) {
                for generic_name in generics.as_str().split(',') {
                    local_generic_names.insert(generic_name.trim().to_string());
                }
            }
            let line = captures.get(0).map(|m| m.as_str()).unwrap_or_default();
            if let Some(name) = line
                .split_whitespace()
                .nth(1)
                .map(|value| value.split('<').next().unwrap_or(value))
            {
                local_swift_types.insert(name.to_string());
            }
        }
    }

    for file in &snapshot.ios_files {
        if !file.contents.contains("import shared") {
            continue;
        }
        for captures in typed_decl_regex.captures_iter(&file.contents) {
            let type_name = captures[1].to_string();
            if swift_common_types.contains(type_name.as_str()) {
                continue;
            }
            if local_swift_types.contains(&type_name) || local_generic_names.contains(&type_name) {
                continue;
            }
            if index.declared_types.contains(&type_name) || index.typealiases.contains(&type_name) {
                continue;
            }
            diagnostics.push(Diagnostic {
                code: "kotlin_type_usage_missing".to_string(),
                severity: config.severity_for("kotlin_type_usage_missing"),
                message: format!(
                    "Swift uses type `{}` that is not found among Kotlin declared types/typealiases",
                    type_name
                ),
                hint: "check renamed or removed typealias/nested type".to_string(),
                kotlin_symbol: Some(type_name),
                ios_symbol: None,
                member: None,
                kotlin_location: None,
                ios_location: captures.get(1).map(|m| offset_to_location(file, m.start())),
                base_ref: None,
                head_ref: None,
                evidence: vec![
                    "kotlin_change:type_removed_or_renamed".to_string(),
                    "swift_usage_found:type_annotation".to_string(),
                ],
            });
        }
    }

    diagnostics
}

fn check_top_level_usages(
    snapshot: &ProjectSnapshot,
    index: &KotlinApiIndex,
    config: &Config,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let top_level_usage_regex =
        Regex::new(r"\b([A-Z][A-Za-z_0-9]*Kt)\.([A-Za-z_]\w*)").expect("top-level usage regex");

    for file in &snapshot.ios_files {
        for captures in top_level_usage_regex.captures_iter(&file.contents) {
            let container = captures[1].to_string();
            let member = captures[2].to_string();
            let Some(known_members) = index.top_level_members_by_kt_type.get(&container) else {
                continue;
            };
            if known_members.contains(&member) {
                continue;
            }
            diagnostics.push(Diagnostic {
                code: "top_level_member_missing".to_string(),
                severity: config.severity_for("top_level_member_missing"),
                message: format!("Swift uses missing top-level Kotlin member `{}.{}`", container, member),
                hint: format!(
                    "expected one of: {}",
                    known_members.iter().cloned().collect::<Vec<_>>().join(", ")
                ),
                kotlin_symbol: Some(container),
                ios_symbol: None,
                member: Some(member),
                kotlin_location: None,
                ios_location: captures.get(1).map(|m| offset_to_location(file, m.start())),
                base_ref: None,
                head_ref: None,
                evidence: vec![
                    "kotlin_change:top_level_member_removed_or_renamed".to_string(),
                    "swift_usage_found:top_level_access".to_string(),
                ],
            });
        }
    }
    diagnostics
}

fn check_companion_member_usages(
    snapshot: &ProjectSnapshot,
    index: &KotlinApiIndex,
    config: &Config,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let companion_usage_regex =
        Regex::new(r"\b([A-Z][A-Za-z_0-9]*)\.companion\.([A-Za-z_]\w*)")
            .expect("companion usage regex");

    for file in &snapshot.ios_files {
        for captures in companion_usage_regex.captures_iter(&file.contents) {
            let class_name = captures[1].to_string();
            let member = captures[2].to_string();
            let Some(members) = index.companion_members.get(&class_name) else {
                diagnostics.push(Diagnostic {
                    code: "companion_object_missing".to_string(),
                    severity: config.severity_for("companion_object_missing"),
                    message: format!(
                        "Swift uses `{}.companion.{}`, but companion object is missing in Kotlin",
                        class_name, member
                    ),
                    hint: "add companion object or update Swift usage".to_string(),
                    kotlin_symbol: Some(class_name),
                    ios_symbol: None,
                    member: Some(member),
                    kotlin_location: None,
                    ios_location: captures.get(1).map(|m| offset_to_location(file, m.start())),
                    base_ref: None,
                    head_ref: None,
                    evidence: vec![
                        "kotlin_change:companion_object_removed".to_string(),
                        "swift_usage_found:companion_access".to_string(),
                    ],
                });
                continue;
            };
            if members.contains(&member) {
                continue;
            }
            diagnostics.push(Diagnostic {
                code: "companion_member_missing".to_string(),
                severity: config.severity_for("companion_member_missing"),
                message: format!(
                    "Swift uses missing companion member `{}.companion.{}`",
                    class_name, member
                ),
                hint: format!(
                    "expected one of: {}",
                    members.iter().cloned().collect::<Vec<_>>().join(", ")
                ),
                kotlin_symbol: Some(class_name),
                ios_symbol: None,
                member: Some(member),
                kotlin_location: None,
                ios_location: captures.get(1).map(|m| offset_to_location(file, m.start())),
                base_ref: None,
                head_ref: None,
                evidence: vec![
                    "kotlin_change:companion_member_removed_or_renamed".to_string(),
                    "swift_usage_found:companion_access".to_string(),
                ],
            });
        }
    }
    diagnostics
}

fn count_swift_arguments(raw_arguments: &str) -> usize {
    if raw_arguments.trim().is_empty() {
        return 0;
    }

    let mut depth = 0isize;
    let mut count = 1usize;
    for character in raw_arguments.chars() {
        match character {
            '(' | '[' | '<' => depth += 1,
            ')' | ']' | '>' => depth -= 1,
            ',' if depth == 0 => count += 1,
            _ => {}
        }
    }

    count
}

fn extract_swift_argument_labels(raw_arguments: &str) -> Vec<Option<String>> {
    if raw_arguments.trim().is_empty() {
        return Vec::new();
    }

    split_top_level_args(raw_arguments)
        .into_iter()
        .map(|argument| {
            let trimmed = argument.trim();
            split_top_level_once(trimmed, ':')
                .map(|(label, _)| label.trim().to_string())
                .filter(|label| !label.is_empty())
        })
        .collect()
}

fn split_top_level_args(raw_arguments: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut depth = 0isize;
    let mut start = 0usize;

    for (index, character) in raw_arguments.char_indices() {
        match character {
            '(' | '[' | '<' => depth += 1,
            ')' | ']' | '>' => depth -= 1,
            ',' if depth == 0 => {
                args.push(raw_arguments[start..index].to_string());
                start = index + 1;
            }
            _ => {}
        }
    }
    args.push(raw_arguments[start..].to_string());
    args
}

fn format_actual_labels_with_mismatches(
    expected_labels: &[String],
    actual_labels: &[Option<String>],
) -> String {
    let max_len = expected_labels.len().max(actual_labels.len());
    let mut formatted = Vec::with_capacity(max_len);

    for index in 0..max_len {
        let expected = expected_labels.get(index).map(|label| label.as_str());
        let actual = actual_labels.get(index).and_then(|label| label.as_deref());
        let actual_rendered = actual.unwrap_or("<unlabeled>");
        let matches = expected == Some(actual_rendered);

        if matches {
            formatted.push(actual_rendered.to_string());
        } else {
            formatted.push(format!("!{actual_rendered}!"));
        }
    }

    formatted.join(", ")
}

fn offset_to_location(file: &SourceFile, byte_offset: usize) -> SourceLocation {
    let mut line = 1usize;
    let mut column = 1usize;

    for character in file.contents[..byte_offset.min(file.contents.len())].chars() {
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }

    SourceLocation {
        path: file.path.clone(),
        line,
        column,
        snapshot: file.snapshot.clone(),
    }
}

fn compare_member_sets(
    kotlin_contract: &Contract,
    swift_protocol: &Contract,
    kotlin_members: &BTreeMap<String, Member>,
    swift_members: &BTreeMap<String, Member>,
    conformance_mode: bool,
    config: &Config,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (name, kotlin_member) in kotlin_members {
        match swift_members.get(name) {
            Some(swift_member) => compare_member_signature(
                kotlin_contract,
                swift_protocol,
                kotlin_member,
                swift_member,
                conformance_mode,
                config,
                diagnostics,
            ),
            None => diagnostics.push(build_diagnostic(
                "missing_member",
                config,
                Some(kotlin_contract),
                Some(swift_protocol),
                Some(name),
                format!(
                    "Member `{}` from Kotlin contract `{}` is missing on Swift protocol `{}`",
                    name, kotlin_contract.fq_name, swift_protocol.name
                ),
                format!("add `{name}` to Swift protocol `{}`", swift_protocol.name),
            )),
        }
    }

    for (name, swift_member) in swift_members {
        if kotlin_members.contains_key(name) {
            continue;
        }

        diagnostics.push(build_diagnostic_with_locations(
            "extra_member",
            config,
            Some(kotlin_contract),
            Some(swift_protocol),
            Some(name),
            Some(kotlin_contract.location.clone()),
            Some(swift_member.location.clone()),
            format!(
                "Swift protocol `{}` still exposes stale member `{}` not present in Kotlin contract `{}`",
                swift_protocol.name, name, kotlin_contract.fq_name
            ),
            format!("remove stale member `{name}` from `{}`", swift_protocol.name),
        ));
    }
}

fn compare_conformance(
    kotlin_contract: &Contract,
    swift_protocol: &Contract,
    swift_type: &Contract,
    required_members: &BTreeMap<String, Member>,
    swift_types: &HashMap<String, &Contract>,
    config: &Config,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let implemented_members = flattened_members(swift_type, swift_types, swift_types, false);

    for (name, required_member) in required_members {
        match implemented_members.get(name) {
            Some(implementation) => compare_member_signature(
                kotlin_contract,
                swift_type,
                required_member,
                implementation,
                true,
                config,
                diagnostics,
            ),
            None => diagnostics.push(build_diagnostic_with_locations(
                "missing_conformance_impl",
                config,
                Some(kotlin_contract),
                Some(swift_type),
                Some(name),
                Some(kotlin_contract.location.clone()),
                Some(swift_type.location.clone()),
                format!(
                    "Swift type `{}` no longer satisfies protocol `{}` because `{}` is missing",
                    swift_type.name, swift_protocol.name, name
                ),
                format!("implement `{name}` on `{}`", swift_type.name),
            )),
        }
    }
}

fn compare_member_signature(
    kotlin_contract: &Contract,
    swift_contract: &Contract,
    kotlin_member: &Member,
    swift_member: &Member,
    conformance_mode: bool,
    config: &Config,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match (&kotlin_member.signature, &swift_member.signature) {
        (MemberSignature::Method(kotlin_method), MemberSignature::Method(swift_method)) => {
            compare_methods(
                kotlin_contract,
                swift_contract,
                kotlin_member,
                kotlin_method,
                swift_member,
                swift_method,
                conformance_mode,
                config,
                diagnostics,
            );
        }
        (MemberSignature::Property(kotlin_property), MemberSignature::Property(swift_property)) => {
            if canonical_type(&kotlin_property.property_type)
                != canonical_type(&swift_property.property_type)
            {
                diagnostics.push(build_diagnostic_with_locations(
                    "parameter_type_mismatch",
                    config,
                    Some(kotlin_contract),
                    Some(swift_contract),
                    Some(kotlin_member.name.as_str()),
                    Some(kotlin_member.location.clone()),
                    Some(swift_member.location.clone()),
                    format!(
                        "Property `{}` uses type `{}` in Kotlin and `{}` in Swift",
                        kotlin_member.name,
                        kotlin_property.property_type,
                        swift_property.property_type
                    ),
                    format!(
                        "expected `{}`; actual `{}`",
                        kotlin_property.property_type, swift_property.property_type
                    ),
                ));
            }

            if kotlin_property.mutable != swift_property.mutable {
                diagnostics.push(build_diagnostic_with_locations(
                    "property_mutability_mismatch",
                    config,
                    Some(kotlin_contract),
                    Some(swift_contract),
                    Some(kotlin_member.name.as_str()),
                    Some(kotlin_member.location.clone()),
                    Some(swift_member.location.clone()),
                    format!(
                        "Property `{}` mutability differs between Kotlin and Swift",
                        kotlin_member.name
                    ),
                    format!(
                        "expected mutable={}; actual mutable={}",
                        kotlin_property.mutable, swift_property.mutable
                    ),
                ));
            }
        }
        _ => {
            diagnostics.push(build_diagnostic_with_locations(
                if conformance_mode {
                    "missing_conformance_impl"
                } else {
                    "missing_member"
                },
                config,
                Some(kotlin_contract),
                Some(swift_contract),
                Some(kotlin_member.name.as_str()),
                Some(kotlin_member.location.clone()),
                Some(swift_member.location.clone()),
                format!(
                    "Member `{}` changes kind between Kotlin and Swift",
                    kotlin_member.name
                ),
                format!(
                    "make `{}` a matching member kind on both sides",
                    kotlin_member.name
                ),
            ));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn compare_methods(
    kotlin_contract: &Contract,
    swift_contract: &Contract,
    kotlin_member: &Member,
    kotlin_method: &MethodSignature,
    swift_member: &Member,
    swift_method: &MethodSignature,
    conformance_mode: bool,
    config: &Config,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if kotlin_method.parameters.len() != swift_method.parameters.len() {
        diagnostics.push(build_diagnostic_with_locations(
            "parameter_count_mismatch",
            config,
            Some(kotlin_contract),
            Some(swift_contract),
            Some(kotlin_member.name.as_str()),
            Some(kotlin_member.location.clone()),
            Some(swift_member.location.clone()),
            format!(
                "Method `{}` has {} parameter(s) in Kotlin and {} in Swift",
                kotlin_member.name,
                kotlin_method.parameters.len(),
                swift_method.parameters.len()
            ),
            format!(
                "expected {} parameter(s); actual {}",
                kotlin_method.parameters.len(),
                swift_method.parameters.len()
            ),
        ));
        return;
    }

    for (index, (kotlin_parameter, swift_parameter)) in kotlin_method
        .parameters
        .iter()
        .zip(swift_method.parameters.iter())
        .enumerate()
    {
        let name_matches = kotlin_parameter.display_name == swift_parameter.display_name
            || kotlin_parameter.display_name == swift_parameter.internal_name;

        if !name_matches {
            diagnostics.push(build_diagnostic_with_locations(
                "parameter_name_mismatch",
                config,
                Some(kotlin_contract),
                Some(swift_contract),
                Some(kotlin_member.name.as_str()),
                Some(kotlin_member.location.clone()),
                Some(swift_member.location.clone()),
                format!(
                    "Method `{}` parameter #{} is named `{}` in Kotlin and `{}` in Swift",
                    kotlin_member.name,
                    index + 1,
                    kotlin_parameter.display_name,
                    swift_parameter.display_name
                ),
                format!(
                    "expected name `{}`; actual name `{}`",
                    kotlin_parameter.display_name,
                    swift_parameter.display_name
                ),
            ));
        }

        if canonical_type(&kotlin_parameter.parameter_type)
            != canonical_type(&swift_parameter.parameter_type)
        {
            diagnostics.push(build_diagnostic_with_locations(
                "parameter_type_mismatch",
                config,
                Some(kotlin_contract),
                Some(swift_contract),
                Some(kotlin_member.name.as_str()),
                Some(kotlin_member.location.clone()),
                Some(swift_member.location.clone()),
                format!(
                    "Method `{}` parameter `{}` uses `{}` in Kotlin and `{}` in Swift",
                    kotlin_member.name,
                    kotlin_parameter.display_name,
                    kotlin_parameter.parameter_type,
                    swift_parameter.parameter_type
                ),
                format!(
                    "expected type `{}`; actual type `{}`",
                    kotlin_parameter.parameter_type,
                    swift_parameter.parameter_type
                ),
            ));
        }
    }

    if canonical_type(&kotlin_method.return_type) != canonical_type(&swift_method.return_type) {
        diagnostics.push(build_diagnostic_with_locations(
            "return_type_mismatch",
            config,
            Some(kotlin_contract),
            Some(swift_contract),
            Some(kotlin_member.name.as_str()),
            Some(kotlin_member.location.clone()),
            Some(swift_member.location.clone()),
            format!(
                "Method `{}` returns `{}` in Kotlin and `{}` in Swift",
                kotlin_member.name, kotlin_method.return_type, swift_method.return_type
            ),
            format!(
                "expected return `{}`; actual return `{}`",
                kotlin_method.return_type, swift_method.return_type
            ),
        ));
    }

    if conformance_mode && swift_contract.kind == ContractKind::SwiftType {
        let protocol_like_parameter_names: BTreeSet<&str> = swift_method
            .parameters
            .iter()
            .map(|parameter| parameter.display_name.as_str())
            .collect();
        if protocol_like_parameter_names.is_empty() {
            let _ = protocol_like_parameter_names;
        }
    }
}

fn find_matching_protocol<'a>(
    kotlin_contract: &Contract,
    swift_protocols: &'a HashMap<String, &'a Contract>,
    config: &Config,
) -> Option<&'a Contract> {
    if let Some(mapped_name) = config
        .mapped_ios_name(&kotlin_contract.fq_name)
        .or_else(|| config.mapped_ios_name(&kotlin_contract.name))
    {
        return swift_protocols.get(mapped_name).copied();
    }

    if let Some(protocol) = swift_protocols.get(&kotlin_contract.name) {
        return Some(protocol);
    }

    let kotlin_normalized = config.normalize_contract_name(&kotlin_contract.name);
    swift_protocols
        .values()
        .copied()
        .find(|contract| config.normalize_contract_name(&contract.name) == kotlin_normalized)
}

fn flattened_members<'a>(
    contract: &'a Contract,
    by_name: &HashMap<String, &'a Contract>,
    by_fq_name: &HashMap<String, &'a Contract>,
    include_defaults: bool,
) -> BTreeMap<String, Member> {
    let mut members = BTreeMap::new();
    let mut visited = HashSet::new();
    collect_flattened_members(
        contract,
        by_name,
        by_fq_name,
        include_defaults,
        &mut visited,
        &mut members,
    );
    members
}

fn collect_flattened_members<'a>(
    contract: &'a Contract,
    by_name: &HashMap<String, &'a Contract>,
    by_fq_name: &HashMap<String, &'a Contract>,
    include_defaults: bool,
    visited: &mut HashSet<String>,
    members: &mut BTreeMap<String, Member>,
) {
    if !visited.insert(contract.id.clone()) {
        return;
    }

    for supertype in &contract.supertypes {
        if let Some(parent) = by_name
            .get(supertype)
            .copied()
            .or_else(|| by_fq_name.get(supertype).copied())
        {
            collect_flattened_members(
                parent,
                by_name,
                by_fq_name,
                include_defaults,
                visited,
                members,
            );
        }
    }

    for member in &contract.members {
        members.insert(member.name.clone(), member.clone());
    }

    if include_defaults {
        for member in &contract.default_members {
            members.insert(member.name.clone(), member.clone());
        }
    }
}

fn merge_member_maps(
    required: &BTreeMap<String, Member>,
    defaults: &BTreeMap<String, Member>,
) -> BTreeMap<String, Member> {
    let mut merged = required.clone();
    for (name, member) in defaults {
        merged.entry(name.clone()).or_insert_with(|| member.clone());
    }
    merged
}

fn index_by_name(contracts: &[Contract]) -> HashMap<String, &Contract> {
    contracts
        .iter()
        .map(|contract| (contract.name.clone(), contract))
        .collect()
}

fn index_by_fq_name(contracts: &[Contract]) -> HashMap<String, &Contract> {
    contracts
        .iter()
        .map(|contract| (contract.fq_name.clone(), contract))
        .collect()
}

fn build_diagnostic(
    code: &str,
    config: &Config,
    kotlin_contract: Option<&Contract>,
    ios_contract: Option<&Contract>,
    member: Option<&str>,
    message: String,
    hint: String,
) -> Diagnostic {
    build_diagnostic_with_locations(
        code,
        config,
        kotlin_contract,
        ios_contract,
        member,
        kotlin_contract.map(|contract| contract.location.clone()),
        ios_contract.map(|contract| contract.location.clone()),
        message,
        hint,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_diagnostic_with_locations(
    code: &str,
    config: &Config,
    kotlin_contract: Option<&Contract>,
    ios_contract: Option<&Contract>,
    member: Option<&str>,
    kotlin_location: Option<crate::model::SourceLocation>,
    ios_location: Option<crate::model::SourceLocation>,
    message: String,
    hint: String,
) -> Diagnostic {
    Diagnostic {
        code: code.to_string(),
        severity: config.severity_for(code),
        message,
        hint,
        kotlin_symbol: kotlin_contract.map(|contract| contract.fq_name.clone()),
        ios_symbol: ios_contract.map(|contract| contract.name.clone()),
        member: member.map(str::to_string),
        kotlin_location,
        ios_location,
        base_ref: None,
        head_ref: None,
        evidence: Vec::new(),
    }
}

fn canonical_type(raw: &str) -> String {
    let compact = raw.trim().replace(' ', "");
    let optional_stripped = compact.trim_end_matches('?');

    if optional_stripped.starts_with('[') && optional_stripped.ends_with(']') {
        let inner = &optional_stripped[1..optional_stripped.len() - 1];
        if let Some((key, value)) = split_top_level_once(inner, ':') {
            return format!("map<{},{}>", canonical_type(key), canonical_type(value));
        }

        return format!("list<{}>", canonical_type(inner));
    }

    if let Some(inner) = optional_stripped
        .strip_prefix("List<")
        .and_then(|value| value.strip_suffix('>'))
        .or_else(|| {
            optional_stripped
                .strip_prefix("Array<")
                .and_then(|value| value.strip_suffix('>'))
        })
        .or_else(|| {
            optional_stripped
                .strip_prefix("Set<")
                .and_then(|value| value.strip_suffix('>'))
        })
    {
        let prefix = if optional_stripped.starts_with("Set<") {
            "set"
        } else {
            "list"
        };
        return format!("{prefix}<{}>", canonical_type(inner));
    }

    if let Some(inner) = optional_stripped
        .strip_prefix("Map<")
        .and_then(|value| value.strip_suffix('>'))
        .or_else(|| {
            optional_stripped
                .strip_prefix("Dictionary<")
                .and_then(|value| value.strip_suffix('>'))
        })
    {
        if let Some((key, value)) = split_top_level_once(inner, ',') {
            return format!("map<{},{}>", canonical_type(key), canonical_type(value));
        }
    }

    match optional_stripped {
        "Boolean" | "Bool" => "bool".to_string(),
        "String" => "string".to_string(),
        "Unit" | "Void" | "()" => "void".to_string(),
        "Int" | "Int32" => "int".to_string(),
        "Long" | "Int64" => "long".to_string(),
        "Short" | "Int16" => "short".to_string(),
        "Byte" | "Int8" => "byte".to_string(),
        "Float" => "float".to_string(),
        "Double" => "double".to_string(),
        "Any" | "AnyObject" => "any".to_string(),
        other => other.rsplit('.').next().unwrap_or(other).to_string(),
    }
}

fn split_top_level_once(input: &str, needle: char) -> Option<(&str, &str)> {
    let mut depth = 0isize;

    for (index, character) in input.char_indices() {
        match character {
            '<' | '[' | '(' => depth += 1,
            '>' | ']' | ')' => depth -= 1,
            _ if character == needle && depth == 0 => {
                let left = input[..index].trim();
                let right = input[index + character.len_utf8()..].trim();
                return Some((left, right));
            }
            _ => {}
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::model::{ProjectSnapshot, SourceFile};

    use super::{canonical_type, compare_project, introduced_diagnostics};

    #[test]
    fn detects_missing_member_and_conformance_issue() {
        let snapshot = ProjectSnapshot {
            label: "workspace".to_string(),
            kotlin_files: vec![SourceFile {
                path: "Shared.kt".to_string(),
                contents: r#"
                    interface UserGateway {
                        fun loadUser(id: String, includePosts: Boolean): User
                    }
                "#
                .to_string(),
                snapshot: None,
            }],
            ios_files: vec![SourceFile {
                path: "UserGateway.swift".to_string(),
                contents: r#"
                    protocol UserGateway {
                        func loadUser(id: String, includePosts: Bool) -> User
                    }

                    struct UserGatewayImpl: UserGateway {
                        var isEnabled: Bool { true }
                    }
                "#
                .to_string(),
                snapshot: None,
            }],
        };

        let diagnostics =
            compare_project(&snapshot, &Config::default()).expect("analysis should succeed");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "missing_conformance_impl")
        );
    }

    #[test]
    fn canonicalizes_basic_cross_platform_types() {
        assert_eq!(canonical_type("Boolean"), "bool");
        assert_eq!(canonical_type("Bool"), "bool");
        assert_eq!(
            canonical_type("Map<String, List<Int>>"),
            "map<string,list<int>>"
        );
        assert_eq!(canonical_type("[String: [Int32]]"), "map<string,list<int>>");
    }

    #[test]
    fn filters_only_introduced_diagnostics() {
        let base = vec![crate::model::Diagnostic {
            code: "missing_member".to_string(),
            severity: crate::config::Severity::Error,
            message: "base".to_string(),
            hint: "base".to_string(),
            kotlin_symbol: Some("A".to_string()),
            ios_symbol: Some("A".to_string()),
            member: Some("foo".to_string()),
            kotlin_location: None,
            ios_location: None,
            base_ref: None,
            head_ref: None,
            evidence: Vec::new(),
        }];
        let head = vec![
            base[0].clone(),
            crate::model::Diagnostic {
                code: "missing_member".to_string(),
                severity: crate::config::Severity::Error,
                message: "head".to_string(),
                hint: "head".to_string(),
                kotlin_symbol: Some("A".to_string()),
                ios_symbol: Some("A".to_string()),
                member: Some("bar".to_string()),
                kotlin_location: None,
                ios_location: None,
                base_ref: None,
                head_ref: None,
                evidence: Vec::new(),
            },
        ];

        let introduced = introduced_diagnostics(base, head);
        assert_eq!(introduced.len(), 1);
        assert_eq!(introduced[0].member.as_deref(), Some("bar"));
    }

    #[test]
    fn does_not_report_missing_protocol_for_unmapped_kotlin_interface() {
        let snapshot = ProjectSnapshot {
            label: "workspace".to_string(),
            kotlin_files: vec![SourceFile {
                path: "Shared.kt".to_string(),
                contents: r#"
                    interface InternalGateway {
                        fun ping(): String
                    }
                "#
                .to_string(),
                snapshot: None,
            }],
            ios_files: vec![],
        };

        let diagnostics =
            compare_project(&snapshot, &Config::default()).expect("analysis should succeed");
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn reports_missing_protocol_for_explicit_mapping_without_swift_protocol() {
        let snapshot = ProjectSnapshot {
            label: "workspace".to_string(),
            kotlin_files: vec![SourceFile {
                path: "Shared.kt".to_string(),
                contents: r#"
                    package com.example
                    interface UserGateway {
                        fun ping(): String
                    }
                "#
                .to_string(),
                snapshot: None,
            }],
            ios_files: vec![],
        };

        let config: Config = toml::from_str(
            r#"
            [[mappings]]
            kotlin = "com.example.UserGateway"
            ios = "UserGatewayProtocol"
            "#,
        )
        .expect("config should parse");

        let diagnostics = compare_project(&snapshot, &config).expect("analysis should succeed");
        assert!(diagnostics.iter().any(|diagnostic| diagnostic.code == "missing_protocol"));
    }

    #[test]
    fn reports_kotlin_class_method_argument_count_mismatch_in_swift_call() {
        let snapshot = ProjectSnapshot {
            label: "workspace".to_string(),
            kotlin_files: vec![SourceFile {
                path: "MainViewModel.kt".to_string(),
                contents: r#"
                    class MainViewModel {
                        fun addItemToCart(fruittie: Fruittie, name: String) {}
                    }
                    class Fruittie
                "#
                .to_string(),
                snapshot: None,
            }],
            ios_files: vec![SourceFile {
                path: "ContentView.swift".to_string(),
                contents: r#"
                    import shared
                    func testCall() {
                        let mainViewModel: MainViewModel = make()
                        let value: Fruittie = makeFruittie()
                        mainViewModel.addItemToCart(fruittie: value)
                    }
                "#
                .to_string(),
                snapshot: None,
            }],
        };

        let diagnostics =
            compare_project(&snapshot, &Config::default()).expect("analysis should succeed");
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "class_method_parameter_count_mismatch"));
    }

    #[test]
    fn does_not_report_when_any_overload_matches_argument_count() {
        let snapshot = ProjectSnapshot {
            label: "workspace".to_string(),
            kotlin_files: vec![SourceFile {
                path: "MainViewModel.kt".to_string(),
                contents: r#"
                    class MainViewModel {
                        fun addItemToCart(fruittie: Fruittie) {}
                        fun addItemToCart(fruittie: Fruittie, name: String) {}
                    }
                    class Fruittie
                "#
                .to_string(),
                snapshot: None,
            }],
            ios_files: vec![SourceFile {
                path: "ContentView.swift".to_string(),
                contents: r#"
                    import shared
                    func testCall() {
                        let mainViewModel: MainViewModel = make()
                        let value: Fruittie = makeFruittie()
                        mainViewModel.addItemToCart(fruittie: value)
                    }
                "#
                .to_string(),
                snapshot: None,
            }],
        };

        let diagnostics =
            compare_project(&snapshot, &Config::default()).expect("analysis should succeed");
        assert!(!diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "class_method_parameter_count_mismatch"));
    }

    #[test]
    fn reports_kotlin_class_method_parameter_name_mismatch_in_swift_call() {
        let snapshot = ProjectSnapshot {
            label: "workspace".to_string(),
            kotlin_files: vec![SourceFile {
                path: "MainViewModel.kt".to_string(),
                contents: r#"
                    class MainViewModel {
                        fun addItemToCart(fruittie: Fruittie, name: String) {}
                    }
                    class Fruittie
                "#
                .to_string(),
                snapshot: None,
            }],
            ios_files: vec![SourceFile {
                path: "ContentView.swift".to_string(),
                contents: r#"
                    import shared
                    func testCall() {
                        let mainViewModel: MainViewModel = make()
                        let value: Fruittie = makeFruittie()
                        mainViewModel.addItemToCart(item: value, title: "x")
                    }
                "#
                .to_string(),
                snapshot: None,
            }],
        };

        let diagnostics =
            compare_project(&snapshot, &Config::default()).expect("analysis should succeed");
        let diagnostic = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "class_method_parameter_name_mismatch")
            .expect("name mismatch diagnostic expected");
        assert!(diagnostic
            .hint
            .contains("expected labels: fruittie, name; actual labels: !item!, !title!"));
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "class_method_parameter_name_mismatch"));
    }

    #[test]
    fn reports_constructor_parameter_count_mismatch() {
        let snapshot = ProjectSnapshot {
            label: "workspace".to_string(),
            kotlin_files: vec![SourceFile {
                path: "User.kt".to_string(),
                contents: r#"
                    class User(id: String)
                "#
                .to_string(),
                snapshot: None,
            }],
            ios_files: vec![SourceFile {
                path: "UserView.swift".to_string(),
                contents: r#"
                    import shared
                    func makeUser() {
                        let _ = User(id: "1", name: "x")
                    }
                "#
                .to_string(),
                snapshot: None,
            }],
        };

        let diagnostics = compare_project(&snapshot, &Config::default()).expect("analysis should succeed");
        assert!(diagnostics.iter().any(|d| d.code == "class_constructor_parameter_count_mismatch"));
    }

    #[test]
    fn reports_property_mismatch_variants() {
        let snapshot = ProjectSnapshot {
            label: "workspace".to_string(),
            kotlin_files: vec![SourceFile {
                path: "Profile.kt".to_string(),
                contents: r#"
                    class Profile {
                        val id: String = ""
                        var nickname: String? = null
                        var score: Int = 0
                    }
                "#
                .to_string(),
                snapshot: None,
            }],
            ios_files: vec![SourceFile {
                path: "ProfileView.swift".to_string(),
                contents: r#"
                    import shared
                    func test() {
                        let p: Profile = make()
                        p.unknown
                        p.id = "x"
                        p.score = nil
                        let value: String = p.score
                    }
                "#
                .to_string(),
                snapshot: None,
            }],
        };

        let diagnostics = compare_project(&snapshot, &Config::default()).expect("analysis should succeed");
        assert!(diagnostics.iter().any(|d| d.code == "class_property_missing"));
        assert!(diagnostics
            .iter()
            .any(|d| d.code == "class_property_mutability_mismatch"));
        assert!(diagnostics
            .iter()
            .any(|d| d.code == "class_property_nullability_mismatch"));
        assert!(diagnostics.iter().any(|d| d.code == "class_property_type_mismatch"));
    }

    #[test]
    fn reports_missing_cases_for_enum_switch() {
        let snapshot = ProjectSnapshot {
            label: "workspace".to_string(),
            kotlin_files: vec![SourceFile {
                path: "Status.kt".to_string(),
                contents: r#"
                    enum class Status { IDLE, LOADING, DONE }
                "#
                .to_string(),
                snapshot: None,
            }],
            ios_files: vec![SourceFile {
                path: "StatusView.swift".to_string(),
                contents: r#"
                    import shared
                    func render() {
                        let status: Status = make()
                        switch onEnum(of: status) {
                        case .idle:
                            break
                        case .loading:
                            break
                        }
                    }
                "#
                .to_string(),
                snapshot: None,
            }],
        };

        let diagnostics = compare_project(&snapshot, &Config::default()).expect("analysis should succeed");
        assert!(diagnostics
            .iter()
            .any(|d| d.code == "enum_or_sealed_switch_missing_cases"));
    }

    #[test]
    fn reports_missing_cases_for_plain_swift_switch() {
        let snapshot = ProjectSnapshot {
            label: "workspace".to_string(),
            kotlin_files: vec![SourceFile {
                path: "Status.kt".to_string(),
                contents: r#"
                    enum class Status { IDLE, LOADING, DONE }
                "#
                .to_string(),
                snapshot: None,
            }],
            ios_files: vec![SourceFile {
                path: "StatusView.swift".to_string(),
                contents: r#"
                    import shared
                    func render() {
                        let status: Status = make()
                        switch status {
                        case .idle:
                            break
                        case .loading:
                            break
                        @unknown default:
                            break
                        }
                    }
                "#
                .to_string(),
                snapshot: None,
            }],
        };

        let diagnostics = compare_project(&snapshot, &Config::default()).expect("analysis should succeed");
        assert!(diagnostics
            .iter()
            .any(|d| d.code == "enum_or_sealed_switch_missing_cases"));
    }

    #[test]
    fn reports_missing_cases_for_sealed_if_is_chain() {
        let snapshot = ProjectSnapshot {
            label: "workspace".to_string(),
            kotlin_files: vec![SourceFile {
                path: "Result.kt".to_string(),
                contents: r#"
                    sealed class ProbeResult {
                        object Success : ProbeResult()
                        object Failure : ProbeResult()
                        data class Pending(val eta: Int) : ProbeResult()
                    }
                "#
                .to_string(),
                snapshot: None,
            }],
            ios_files: vec![SourceFile {
                path: "ResultView.swift".to_string(),
                contents: r#"
                    import shared
                    func render() {
                        let result: ProbeResult = make()
                        if result is ProbeResultSuccess {
                            return
                        }
                        if result is ProbeResultFailure {
                            return
                        }
                    }
                "#
                .to_string(),
                snapshot: None,
            }],
        };

        let diagnostics = compare_project(&snapshot, &Config::default()).expect("analysis should succeed");
        assert!(diagnostics
            .iter()
            .any(|d| d.code == "enum_or_sealed_switch_missing_cases"));
    }

    #[test]
    fn reports_missing_kotlin_type_usage() {
        let snapshot = ProjectSnapshot {
            label: "workspace".to_string(),
            kotlin_files: vec![SourceFile {
                path: "Known.kt".to_string(),
                contents: r#"
                    class Known
                "#
                .to_string(),
                snapshot: None,
            }],
            ios_files: vec![SourceFile {
                path: "TypeUse.swift".to_string(),
                contents: r#"
                    import shared
                    func f() {
                        let _: MissingType = make()
                    }
                "#
                .to_string(),
                snapshot: None,
            }],
        };

        let diagnostics = compare_project(&snapshot, &Config::default()).expect("analysis should succeed");
        assert!(diagnostics.iter().any(|d| d.code == "kotlin_type_usage_missing"));
    }

    #[test]
    fn reports_missing_top_level_and_companion_members() {
        let snapshot = ProjectSnapshot {
            label: "workspace".to_string(),
            kotlin_files: vec![SourceFile {
                path: "Utils.kt".to_string(),
                contents: r#"
                    fun ping() = Unit
                    class Keys {
                        companion object {
                            const val TOKEN = "x"
                        }
                    }
                "#
                .to_string(),
                snapshot: None,
            }],
            ios_files: vec![SourceFile {
                path: "Use.swift".to_string(),
                contents: r#"
                    import shared
                    func test() {
                        _ = UtilsKt.missingCall
                        _ = Keys.companion.MISSING
                    }
                "#
                .to_string(),
                snapshot: None,
            }],
        };

        let diagnostics = compare_project(&snapshot, &Config::default()).expect("analysis should succeed");
        assert!(diagnostics.iter().any(|d| d.code == "top_level_member_missing"));
        assert!(diagnostics.iter().any(|d| d.code == "companion_member_missing"));
    }
}
