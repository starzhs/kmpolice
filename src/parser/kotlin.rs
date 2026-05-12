use anyhow::{Context, Result};
use tree_sitter::{Node, Parser, Point};

use crate::model::{
    Contract, ContractKind, Member, MemberSignature, MethodSignature, Parameter, SourceFile,
    SourceLocation,
};

pub fn parse_kotlin_files(files: &[SourceFile]) -> Result<Vec<Contract>> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_kotlin::language())
        .context("failed to initialize Kotlin parser")?;

    let mut contracts = Vec::new();

    for file in files {
        let tree = parser
            .parse(&file.contents, None)
            .with_context(|| format!("failed to parse Kotlin file {}", file.path))?;
        let root = tree.root_node();
        let package_name = find_package_name(root, &file.contents);
        collect_contracts(root, &file.contents, file, &package_name, &mut contracts);
    }

    Ok(contracts)
}

fn collect_contracts(
    node: Node<'_>,
    source: &str,
    file: &SourceFile,
    package_name: &str,
    contracts: &mut Vec<Contract>,
) {
    if node.kind() == "class_declaration" {
        if is_interface_declaration(node, source) {
            if let Some(contract) = parse_interface(node, source, file, package_name) {
                contracts.push(contract);
            }
            return;
        }
        if is_class_declaration(node, source) {
            if let Some(contract) = parse_class(node, source, file, package_name) {
                contracts.push(contract);
            }
            return;
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_contracts(child, source, file, package_name, contracts);
    }
}

fn parse_interface(
    node: Node<'_>,
    source: &str,
    file: &SourceFile,
    package_name: &str,
) -> Option<Contract> {
    let name_node = first_named_child_of_kind(node, "type_identifier")?;
    let name = node_text(name_node, source)?.to_string();
    let fq_name = if package_name.is_empty() {
        name.clone()
    } else {
        format!("{package_name}.{name}")
    };

    let location = to_location(file, name_node.start_position());
    let supertypes = interface_supertypes(node, source);
    let body = first_named_child_of_kind(node, "class_body")?;
    let members = collect_interface_members(body, source, file);

    Some(Contract {
        id: fq_name.clone(),
        name,
        fq_name,
        kind: ContractKind::KotlinInterface,
        supertypes,
        conformances: Vec::new(),
        members,
        default_members: Vec::new(),
        location,
    })
}

fn find_package_name(root: Node<'_>, source: &str) -> String {
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        if child.kind() == "package_header" {
            return node_text(child, source)
                .unwrap_or_default()
                .trim()
                .strip_prefix("package")
                .unwrap_or_else(|| node_text(child, source).unwrap_or_default())
                .trim()
                .to_string();
        }
    }

    String::new()
}

fn is_interface_declaration(node: Node<'_>, source: &str) -> bool {
    let Some(name_node) = first_named_child_of_kind(node, "type_identifier") else {
        return false;
    };

    source
        .get(node.start_byte()..name_node.start_byte())
        .is_some_and(|prefix| prefix.contains("interface"))
}

fn is_class_declaration(node: Node<'_>, source: &str) -> bool {
    let Some(name_node) = first_named_child_of_kind(node, "type_identifier") else {
        return false;
    };

    source
        .get(node.start_byte()..name_node.start_byte())
        .is_some_and(|prefix| prefix.contains("class"))
}

fn parse_class(
    node: Node<'_>,
    source: &str,
    file: &SourceFile,
    package_name: &str,
) -> Option<Contract> {
    if !is_public_declaration(node, source) {
        return None;
    }

    let name_node = first_named_child_of_kind(node, "type_identifier")?;
    let name = node_text(name_node, source)?.to_string();
    let fq_name = if package_name.is_empty() {
        name.clone()
    } else {
        format!("{package_name}.{name}")
    };

    let location = to_location(file, name_node.start_position());
    let supertypes = interface_supertypes(node, source);
    let body = first_named_child_of_kind(node, "class_body")?;
    let members = collect_class_members(body, source, file);

    Some(Contract {
        id: fq_name.clone(),
        name,
        fq_name,
        kind: ContractKind::KotlinClass,
        supertypes,
        conformances: Vec::new(),
        members,
        default_members: Vec::new(),
        location,
    })
}

fn interface_supertypes(node: Node<'_>, source: &str) -> Vec<String> {
    let mut supertypes = Vec::new();
    let mut cursor = node.walk();

    for child in node.named_children(&mut cursor) {
        if child.kind() != "delegation_specifier" {
            continue;
        }

        if let Some(type_node) = first_type_like_child(child) {
            if let Some(text) = node_text(type_node, source) {
                supertypes.push(last_type_segment(text));
            }
        }
    }

    supertypes
}

fn collect_interface_members(body: Node<'_>, source: &str, file: &SourceFile) -> Vec<Member> {
    let mut members = Vec::new();
    collect_members_recursive(body, source, file, &mut members);
    members
}

fn collect_class_members(body: Node<'_>, source: &str, file: &SourceFile) -> Vec<Member> {
    let mut members = Vec::new();
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        match child.kind() {
            "function_declaration" => {
                if is_public_declaration(child, source)
                    && let Some(member) = parse_method(child, source, file)
                {
                    members.push(member);
                }
            }
            "property_declaration" => {
                if is_public_declaration(child, source)
                    && let Some(member) = parse_property(child, source, file)
                {
                    members.push(member);
                }
            }
            _ => {}
        }
    }
    members
}

fn collect_members_recursive(
    node: Node<'_>,
    source: &str,
    file: &SourceFile,
    members: &mut Vec<Member>,
) {
    match node.kind() {
        "function_declaration" => {
            if let Some(member) = parse_method(node, source, file) {
                members.push(member);
            }
            return;
        }
        "property_declaration" => {
            if let Some(member) = parse_property(node, source, file) {
                members.push(member);
            }
            return;
        }
        "class_declaration" => return,
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_members_recursive(child, source, file, members);
    }
}

fn parse_method(node: Node<'_>, source: &str, file: &SourceFile) -> Option<Member> {
    let name_node = first_named_child_of_kind(node, "simple_identifier")?;
    let name = node_text(name_node, source)?.to_string();
    let location = to_location(file, name_node.start_position());

    let parameters = first_named_child_of_kind(node, "function_value_parameters")
        .map(|parameters_node| parse_parameters(parameters_node, source))
        .unwrap_or_default();

    let return_type = kotlin_return_type(node, source).unwrap_or_else(|| "Unit".to_string());

    Some(Member {
        name,
        signature: MemberSignature::Method(MethodSignature {
            parameters,
            return_type,
        }),
        location,
    })
}

fn parse_parameters(node: Node<'_>, source: &str) -> Vec<Parameter> {
    let mut parameters = Vec::new();
    let mut cursor = node.walk();

    for child in node.named_children(&mut cursor) {
        if child.kind() != "parameter" && child.kind() != "parameter_with_optional_type" {
            continue;
        }

        let Some(name_node) = first_named_child_of_kind(child, "simple_identifier") else {
            continue;
        };
        let name = node_text(name_node, source).unwrap_or_default().to_string();
        let parameter_type = first_type_like_child(child)
            .and_then(|type_node| node_text(type_node, source))
            .map(clean_type)
            .unwrap_or_else(|| "Any".to_string());

        parameters.push(Parameter {
            display_name: name.clone(),
            internal_name: name.clone(),
            optional: parameter_type.ends_with('?'),
            has_default: node_text(child, source).is_some_and(|text| text.contains('=')),
            parameter_type,
        });
    }

    parameters
}

fn parse_property(node: Node<'_>, source: &str, file: &SourceFile) -> Option<Member> {
    let name_node = find_named_descendant_of_kind(node, "simple_identifier")?;
    let name = node_text(name_node, source)?.to_string();
    let location = to_location(file, name_node.start_position());
    let declaration_prefix = source
        .get(node.start_byte()..name_node.start_byte())
        .unwrap_or_default();
    let mutable = declaration_prefix.contains("var");

    let property_type = find_type_like_descendant(node)
        .and_then(|type_node| node_text(type_node, source))
        .map(clean_type)
        .unwrap_or_else(|| "Any".to_string());

    Some(Member {
        name,
        signature: MemberSignature::Property(crate::model::PropertySignature {
            property_type,
            mutable,
        }),
        location,
    })
}

fn kotlin_return_type(node: Node<'_>, source: &str) -> Option<String> {
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

fn first_named_child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn first_type_like_child<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| is_type_like(child.kind()))
}

fn find_named_descendant_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    if node.kind() == kind {
        return Some(node);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(found) = find_named_descendant_of_kind(child, kind) {
            return Some(found);
        }
    }

    None
}

fn find_type_like_descendant<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    if is_type_like(node.kind()) {
        return Some(node);
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(found) = find_type_like_descendant(child) {
            return Some(found);
        }
    }

    None
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

fn is_public_declaration(node: Node<'_>, source: &str) -> bool {
    let Some(name_node) = first_named_child_of_kind(node, "simple_identifier")
        .or_else(|| first_named_child_of_kind(node, "type_identifier"))
    else {
        return true;
    };
    let prefix = source
        .get(node.start_byte()..name_node.start_byte())
        .unwrap_or_default();
    !prefix.contains("private ") && !prefix.contains("internal ") && !prefix.contains("protected ")
}

fn last_type_segment(value: &str) -> String {
    let cleaned = clean_type(value);
    cleaned
        .rsplit('.')
        .next()
        .unwrap_or(cleaned.as_str())
        .split('<')
        .next()
        .unwrap_or(cleaned.as_str())
        .to_string()
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
    use super::parse_kotlin_files;
    use crate::model::{MemberSignature, SourceFile};

    #[test]
    fn parses_interface_members() {
        let files = vec![SourceFile {
            path: "Sample.kt".to_string(),
            contents: r#"
                package sample.api

                interface UserGateway : BaseGateway {
                    fun loadUser(id: String, includePosts: Boolean = false): User
                    val isEnabled: Boolean
                }
            "#
            .to_string(),
            snapshot: None,
        }];

        let contracts = parse_kotlin_files(&files).expect("kotlin parsing should succeed");
        let contract = contracts.first().expect("one interface expected");

        assert_eq!(contract.name, "UserGateway");
        assert_eq!(contract.fq_name, "sample.api.UserGateway");
        assert_eq!(contract.supertypes, vec!["BaseGateway"]);
        assert_eq!(contract.members.len(), 2);

        match &contract.members[0].signature {
            MemberSignature::Method(method) => {
                assert_eq!(method.parameters.len(), 2);
                assert_eq!(method.return_type, "User");
            }
            _ => panic!("expected method"),
        }
    }
}
