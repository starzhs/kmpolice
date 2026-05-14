use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use tree_sitter::{Node, Parser, Point};

use crate::model::{
    Contract, ContractKind, Member, MemberSignature, MethodSignature, Parameter, PropertySignature,
    SourceFile, SourceLocation,
};

pub fn parse_swift_files(
    files: &[SourceFile],
) -> Result<(Vec<Contract>, Vec<Contract>, HashSet<String>)> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_swift::language())
        .context("failed to initialize Swift parser")?;

    let mut protocols = HashMap::<String, Contract>::new();
    let mut types = HashMap::<String, Contract>::new();
    let mut extensions = Vec::<SwiftExtension>::new();
    let mut generic_placeholders = HashSet::<String>::new();

    for file in files {
        let tree = parser
            .parse(&file.contents, None)
            .with_context(|| format!("failed to parse Swift file {}", file.path))?;
        let root = tree.root_node();
        collect_swift_generic_placeholders(root, &file.contents, &mut generic_placeholders);
        let mut cursor = root.walk();

        for child in root.named_children(&mut cursor) {
            match child.kind() {
                "protocol_declaration" => {
                    if let Some(contract) = parse_protocol(child, &file.contents, file) {
                        protocols
                            .entry(contract.name.clone())
                            .and_modify(|existing| merge_contract(existing, contract.clone()))
                            .or_insert(contract);
                    }
                }
                "class_declaration" => match class_like_kind(child, &file.contents).as_deref() {
                    Some("extension") => {
                        if let Some(extension) = parse_extension(child, &file.contents, file) {
                            extensions.push(extension);
                        }
                    }
                    Some(_) => {
                        if let Some(contract) = parse_type(child, &file.contents, file) {
                            types
                                .entry(contract.name.clone())
                                .and_modify(|existing| merge_contract(existing, contract.clone()))
                                .or_insert(contract);
                        }
                    }
                    None => {}
                },
                _ => {}
            }
        }
    }

    merge_extensions(&mut protocols, &mut types, extensions);

    Ok((
        protocols.into_values().collect(),
        types.into_values().collect(),
        generic_placeholders,
    ))
}

fn collect_swift_generic_placeholders(node: Node<'_>, source: &str, output: &mut HashSet<String>) {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind().contains("generic_parameter")
            && let Some(name_node) = current.child_by_field_name("name")
            && let Some(name) = node_text(name_node, source)
        {
            let normalized = name.trim();
            if !normalized.is_empty() {
                output.insert(normalized.to_string());
            }
        }
        if (current.kind().contains("type_parameters")
            || current.kind().contains("generic_parameter_clause"))
            && let Some(clause_text) = node_text(current, source)
        {
            for name in parse_generic_names_from_clause_text(clause_text) {
                output.insert(name);
            }
        }
        let mut cursor = current.walk();
        for child in current.named_children(&mut cursor) {
            stack.push(child);
        }
    }
}

fn parse_generic_names_from_clause_text(clause: &str) -> Vec<String> {
    let trimmed = clause.trim();
    let inner = trimmed
        .strip_prefix('<')
        .and_then(|value| value.strip_suffix('>'))
        .unwrap_or(trimmed);
    inner
        .split(',')
        .filter_map(|part| {
            let candidate = part.trim();
            if candidate.is_empty() {
                return None;
            }
            let head = candidate.split(':').next().unwrap_or(candidate).trim();
            let mut name = String::new();
            for character in head.chars() {
                if name.is_empty() {
                    if character.is_ascii_alphabetic() || character == '_' {
                        name.push(character);
                    } else if !character.is_whitespace() {
                        break;
                    }
                } else if character.is_ascii_alphanumeric() || character == '_' {
                    name.push(character);
                } else {
                    break;
                }
            }
            if name.is_empty() { None } else { Some(name) }
        })
        .collect()
}

#[derive(Debug, Clone)]
struct SwiftExtension {
    target_name: String,
    conformances: Vec<String>,
    members: Vec<Member>,
    location: SourceLocation,
}

fn parse_protocol(node: Node<'_>, source: &str, file: &SourceFile) -> Option<Contract> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source)?.to_string();
    let body = node.child_by_field_name("body")?;
    let location = to_location(file, name_node.start_position());

    Some(Contract {
        id: name.clone(),
        name: name.clone(),
        fq_name: name,
        kind: ContractKind::SwiftProtocol,
        supertypes: inheritance_specifiers(node, source),
        conformances: Vec::new(),
        members: collect_protocol_members(body, source, file),
        default_members: Vec::new(),
        location,
    })
}

fn parse_type(node: Node<'_>, source: &str, file: &SourceFile) -> Option<Contract> {
    let name_node = node.child_by_field_name("name")?;
    let name = last_type_segment(node_text(name_node, source)?);
    let body = node.child_by_field_name("body")?;
    let location = to_location(file, name_node.start_position());

    Some(Contract {
        id: name.clone(),
        name: name.clone(),
        fq_name: name,
        kind: ContractKind::SwiftType,
        supertypes: Vec::new(),
        conformances: inheritance_specifiers(node, source),
        members: collect_type_members(body, source, file),
        default_members: Vec::new(),
        location,
    })
}

fn parse_extension(node: Node<'_>, source: &str, file: &SourceFile) -> Option<SwiftExtension> {
    let name_node = node.child_by_field_name("name")?;
    let body = node.child_by_field_name("body")?;
    let target_name = last_type_segment(node_text(name_node, source)?);

    Some(SwiftExtension {
        target_name,
        conformances: inheritance_specifiers(node, source),
        members: collect_type_members(body, source, file),
        location: to_location(file, name_node.start_position()),
    })
}

fn class_like_kind(node: Node<'_>, source: &str) -> Option<String> {
    node.child_by_field_name("declaration_kind")
        .and_then(|child| node_text(child, source))
        .map(str::to_string)
}

fn inheritance_specifiers(node: Node<'_>, source: &str) -> Vec<String> {
    let mut inherited = Vec::new();
    let mut cursor = node.walk();

    for child in node.named_children(&mut cursor) {
        if child.kind() != "inheritance_specifier" {
            continue;
        }

        if let Some(text) = node_text(child, source) {
            let raw = text.trim().trim_start_matches(':');
            for item in raw.split(',') {
                let candidate = last_type_segment(item);
                if !candidate.is_empty() {
                    inherited.push(candidate);
                }
            }
        }
    }

    inherited
}

fn collect_protocol_members(body: Node<'_>, source: &str, file: &SourceFile) -> Vec<Member> {
    let mut members = Vec::new();
    let mut cursor = body.walk();

    for child in body.named_children(&mut cursor) {
        match child.kind() {
            "protocol_function_declaration" => {
                if let Some(member) = parse_method(child, source, file) {
                    members.push(member);
                }
            }
            "protocol_property_declaration" => {
                if let Some(member) = parse_protocol_property(child, source, file) {
                    members.push(member);
                }
            }
            _ => {}
        }
    }

    members
}

fn collect_type_members(body: Node<'_>, source: &str, file: &SourceFile) -> Vec<Member> {
    let mut members = Vec::new();
    let mut cursor = body.walk();

    for child in body.named_children(&mut cursor) {
        match child.kind() {
            "function_declaration" => {
                if let Some(member) = parse_method(child, source, file) {
                    members.push(member);
                }
            }
            "property_declaration" => {
                if let Some(member) = parse_property(child, source, file) {
                    members.push(member);
                }
            }
            _ => {}
        }
    }

    members
}

fn parse_method(node: Node<'_>, source: &str, file: &SourceFile) -> Option<Member> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(name_node, source)?.to_string();
    let location = to_location(file, name_node.start_position());
    let return_type = node
        .child_by_field_name("return_type")
        .and_then(|node| node_text(node, source))
        .map(clean_type)
        .unwrap_or_else(|| "Void".to_string());

    let mut parameters = Vec::new();
    let mut cursor = node.walk();

    for child in node.named_children(&mut cursor) {
        if child.kind() != "parameter" {
            continue;
        }

        let Some(name_node) = child.child_by_field_name("name") else {
            continue;
        };
        let Some(type_node) = child.child_by_field_name("type") else {
            continue;
        };

        let internal_name = node_text(name_node, source)?.to_string();
        let external_name = child
            .child_by_field_name("external_name")
            .and_then(|node| node_text(node, source))
            .map(str::to_string);
        let parameter_type = clean_type(node_text(type_node, source)?);
        let display_name = external_name
            .filter(|value| value != "_")
            .unwrap_or_else(|| internal_name.clone());

        parameters.push(Parameter {
            display_name,
            internal_name,
            optional: parameter_type.ends_with('?'),
            has_default: child.child_by_field_name("default_value").is_some(),
            parameter_type,
        });
    }

    Some(Member {
        name,
        signature: MemberSignature::Method(MethodSignature {
            parameters,
            return_type,
        }),
        location,
    })
}

fn parse_property(node: Node<'_>, source: &str, file: &SourceFile) -> Option<Member> {
    let pattern = node.child_by_field_name("name")?;
    let name = property_name(pattern, source)?;
    let location = to_location(file, pattern.start_position());
    let property_type = direct_named_children(node)
        .into_iter()
        .find(|child| child.kind() == "type_annotation")
        .and_then(|annotation| node_text(annotation, source))
        .map(clean_type_annotation)
        .unwrap_or_else(|| "Any".to_string());

    let declaration_prefix = source
        .get(node.start_byte()..pattern.start_byte())
        .unwrap_or_default();
    let computed_value = node.child_by_field_name("computed_value");
    let mutable = declaration_prefix.contains("var")
        && computed_value
            .map(|computed| {
                node_text(computed, source)
                    .unwrap_or_default()
                    .contains("set")
            })
            .unwrap_or(true);

    Some(Member {
        name,
        signature: MemberSignature::Property(PropertySignature {
            property_type,
            mutable,
        }),
        location,
    })
}

fn parse_protocol_property(node: Node<'_>, source: &str, file: &SourceFile) -> Option<Member> {
    let pattern = node.child_by_field_name("name")?;
    let name = property_name(pattern, source)?;
    let location = to_location(file, pattern.start_position());
    let property_type = direct_named_children(node)
        .into_iter()
        .find(|child| child.kind() == "type_annotation")
        .and_then(|annotation| node_text(annotation, source))
        .map(clean_type_annotation)
        .unwrap_or_else(|| "Any".to_string());
    let mutable = direct_named_children(node)
        .into_iter()
        .find(|child| child.kind() == "protocol_property_requirements")
        .and_then(|requirements| node_text(requirements, source))
        .is_some_and(|text| text.contains("set"));

    Some(Member {
        name,
        signature: MemberSignature::Property(PropertySignature {
            property_type,
            mutable,
        }),
        location,
    })
}

fn property_name(node: Node<'_>, source: &str) -> Option<String> {
    node.child_by_field_name("bound_identifier")
        .or_else(|| node.child_by_field_name("name"))
        .and_then(|name_node| node_text(name_node, source))
        .map(str::to_string)
}

fn merge_extensions(
    protocols: &mut HashMap<String, Contract>,
    types: &mut HashMap<String, Contract>,
    extensions: Vec<SwiftExtension>,
) {
    for extension in extensions {
        if let Some(protocol) = protocols.get_mut(&extension.target_name) {
            protocol.default_members.extend(extension.members);
            continue;
        }

        if let Some(contract) = types.get_mut(&extension.target_name) {
            contract.members.extend(extension.members);
            contract.conformances.extend(extension.conformances);
            dedup_string_vec(&mut contract.conformances);
            continue;
        }

        types.insert(
            extension.target_name.clone(),
            Contract {
                id: extension.target_name.clone(),
                fq_name: extension.target_name.clone(),
                name: extension.target_name,
                kind: ContractKind::SwiftType,
                supertypes: Vec::new(),
                conformances: extension.conformances,
                members: extension.members,
                default_members: Vec::new(),
                location: extension.location,
            },
        );
    }
}

fn merge_contract(existing: &mut Contract, incoming: Contract) {
    existing.members.extend(incoming.members);
    existing.default_members.extend(incoming.default_members);
    existing.supertypes.extend(incoming.supertypes);
    existing.conformances.extend(incoming.conformances);
    dedup_string_vec(&mut existing.supertypes);
    dedup_string_vec(&mut existing.conformances);
}

fn dedup_string_vec(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}

fn direct_named_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn clean_type(raw: &str) -> String {
    raw.split_whitespace().collect::<String>()
}

fn clean_type_annotation(raw: &str) -> String {
    clean_type(raw.trim().trim_start_matches(':'))
}

fn last_type_segment(raw: &str) -> String {
    let cleaned = clean_type(raw);
    let segment = cleaned.rsplit('.').next().unwrap_or(cleaned.as_str());
    segment.to_string()
}

fn node_text<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
    source.get(node.start_byte()..node.end_byte())
}

fn to_location(file: &SourceFile, point: Point) -> SourceLocation {
    SourceLocation {
        path: file.path.clone(),
        line: point.row + 1,
        column: point.column + 1,
        snapshot: file.snapshot.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_swift_files;
    use crate::model::{MemberSignature, SourceFile};

    #[test]
    fn parses_protocols_and_extensions() {
        let files = vec![SourceFile {
            path: "Sample.swift".to_string(),
            contents: r#"
                protocol UserGateway {
                    func loadUser(id: String, includePosts: Bool) -> User
                    var isEnabled: Bool { get }
                }

                extension UserGateway {
                    func cachedUser() -> User { fatalError() }
                }

                struct UserGatewayImpl: UserGateway {
                    func loadUser(id: String, includePosts: Bool) -> User { User() }
                    var isEnabled: Bool { true }
                }
            "#
            .to_string(),
            snapshot: None,
        }];

        let (protocols, types, _generic_placeholders) =
            parse_swift_files(&files).expect("swift parsing should succeed");
        let protocol_contract = protocols.first().expect("protocol expected");
        let type_contract = types.first().expect("type expected");

        assert_eq!(protocol_contract.members.len(), 2);
        assert_eq!(protocol_contract.default_members.len(), 1);
        assert!(
            type_contract
                .conformances
                .contains(&"UserGateway".to_string())
        );

        match &protocol_contract.members[0].signature {
            MemberSignature::Method(method) => assert_eq!(method.parameters.len(), 2),
            _ => panic!("expected method"),
        }
    }

    #[test]
    fn collects_generic_placeholders() {
        let files = vec![SourceFile {
            path: "Generic.swift".to_string(),
            contents: r#"
                struct Box<T, U: Equatable> {}
            "#
            .to_string(),
            snapshot: None,
        }];

        let (_protocols, _types, generic_placeholders) =
            parse_swift_files(&files).expect("swift parsing should succeed");
        assert!(generic_placeholders.contains("T"));
        assert!(generic_placeholders.contains("U"));
    }
}
