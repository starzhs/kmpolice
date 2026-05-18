use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Default for Severity {
    fn default() -> Self {
        Self::Error
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub kotlin_roots: Vec<String>,
    #[serde(default)]
    pub ios_roots: Vec<String>,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub mappings: Vec<MappingRule>,
    #[serde(default)]
    pub naming: NamingConfig,
    #[serde(default)]
    pub ignore: IgnoreConfig,
    #[serde(default)]
    pub severity: BTreeMap<String, Severity>,
    #[serde(default)]
    pub shared_sdk_name: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MappingRule {
    pub kotlin: String,
    pub ios: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NamingConfig {
    #[serde(default)]
    pub strip_prefixes: Vec<String>,
    #[serde(default)]
    pub strip_suffixes: Vec<String>,
    #[serde(default)]
    pub case_insensitive: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct IgnoreConfig {
    #[serde(default)]
    pub diagnostics: Vec<String>,
    #[serde(default)]
    pub kotlin_symbols: Vec<String>,
    #[serde(default)]
    pub ios_symbols: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PathMatcher {
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        match path {
            Some(path) => {
                let contents = fs::read_to_string(path)
                    .with_context(|| format!("failed to read config {}", path.display()))?;
                toml::from_str(&contents)
                    .with_context(|| format!("failed to parse config {}", path.display()))
            }
            None => Ok(Self::default()),
        }
    }

    pub fn severity_for(&self, code: &str) -> Severity {
        self.severity.get(code).copied().unwrap_or_default()
    }

    pub fn path_matcher(&self) -> Result<PathMatcher> {
        Ok(PathMatcher {
            include: compile_glob_set(&self.include)?,
            exclude: compile_glob_set(&self.exclude)?,
        })
    }

    pub fn should_ignore_diagnostic(
        &self,
        code: &str,
        kotlin_symbol: Option<&str>,
        ios_symbol: Option<&str>,
    ) -> bool {
        self.ignore.diagnostics.iter().any(|item| item == code)
            || kotlin_symbol
                .map(|symbol| self.ignore.kotlin_symbols.iter().any(|item| item == symbol))
                .unwrap_or(false)
            || ios_symbol
                .map(|symbol| self.ignore.ios_symbols.iter().any(|item| item == symbol))
                .unwrap_or(false)
    }

    pub fn mapped_ios_name<'a>(&'a self, kotlin_symbol: &str) -> Option<&'a str> {
        self.mappings
            .iter()
            .find(|rule| rule.kotlin == kotlin_symbol)
            .map(|rule| rule.ios.as_str())
    }

    pub fn normalize_contract_name(&self, name: &str) -> String {
        let mut normalized = name.to_string();

        for prefix in &self.naming.strip_prefixes {
            if let Some(stripped) = normalized.strip_prefix(prefix) {
                normalized = stripped.to_string();
            }
        }

        for suffix in &self.naming.strip_suffixes {
            if let Some(stripped) = normalized.strip_suffix(suffix) {
                normalized = stripped.to_string();
            }
        }

        let filtered: String = normalized
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .collect();

        if self.naming.case_insensitive {
            filtered.to_lowercase()
        } else {
            filtered
        }
    }
}

impl PathMatcher {
    pub fn is_included(&self, path: &str) -> bool {
        if self
            .exclude
            .as_ref()
            .is_some_and(|globset| globset.is_match(path))
        {
            return false;
        }

        match &self.include {
            Some(globset) => globset.is_match(path),
            None => true,
        }
    }
}

fn compile_glob_set(patterns: &[String]) -> Result<Option<GlobSet>> {
    if patterns.is_empty() {
        return Ok(None);
    }

    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder
            .add(Glob::new(pattern).with_context(|| format!("invalid glob pattern `{pattern}`"))?);
    }

    Ok(Some(builder.build()?))
}
