use serde::Serialize;

use crate::config::Severity;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ContractKind {
    KotlinInterface,
    KotlinClass,
    SwiftProtocol,
    SwiftType,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MemberSignature {
    Method(MethodSignature),
    Property(PropertySignature),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MethodSignature {
    pub parameters: Vec<Parameter>,
    pub return_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PropertySignature {
    pub property_type: String,
    pub mutable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Parameter {
    pub display_name: String,
    pub internal_name: String,
    pub parameter_type: String,
    pub optional: bool,
    pub has_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Member {
    pub name: String,
    pub signature: MemberSignature,
    pub location: SourceLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct SourceLocation {
    pub path: String,
    pub line: usize,
    pub column: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Contract {
    pub id: String,
    pub name: String,
    pub fq_name: String,
    pub kind: ContractKind,
    pub supertypes: Vec<String>,
    pub conformances: Vec<String>,
    pub members: Vec<Member>,
    pub default_members: Vec<Member>,
    pub location: SourceLocation,
}

#[derive(Debug, Clone)]
pub struct AnalysisResult {
    pub kotlin_contracts: Vec<Contract>,
    pub swift_protocols: Vec<Contract>,
    pub swift_types: Vec<Contract>,
}

#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: String,
    pub contents: String,
    pub snapshot: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProjectSnapshot {
    pub label: String,
    pub kotlin_files: Vec<SourceFile>,
    pub ios_files: Vec<SourceFile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub hint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kotlin_symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ios_symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kotlin_location: Option<SourceLocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ios_location: Option<SourceLocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
}

impl Diagnostic {
    pub fn fingerprint(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.code,
            self.kotlin_symbol.as_deref().unwrap_or("-"),
            self.ios_symbol.as_deref().unwrap_or("-"),
            self.member.as_deref().unwrap_or("-"),
        )
    }
}
