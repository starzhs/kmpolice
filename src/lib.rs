pub mod analyzer;
pub mod cli;
pub mod config;
pub mod git;
pub mod ios_usage;
pub mod model;
pub mod mr;
pub mod parser;
pub mod report;
pub mod source;

use anyhow::Result;
use clap::Parser;

use cli::{Cli, OutputFormat};
use config::{Config, Severity};
use model::Diagnostic;
use mr::{render_ios_usage_report, render_mr_debug, render_verbose_changes, run_mr};
use report::{render_json, render_text};

pub fn run() -> Result<i32> {
    let cli = Cli::parse();
    let config = Config::load(cli.config_path())?;
    let shared_sdk_name = cli
        .shared_sdk_name
        .clone()
        .or_else(|| config.shared_sdk_name.clone())
        .unwrap_or_else(|| "shared".to_string());
    let result = run_mr(
        &cli.repo,
        &cli.target,
        &config,
        cli.verbose_changes,
        &shared_sdk_name,
    )?;
    let diagnostics = result.diagnostics;

    let mut output = match cli.output_format() {
        OutputFormat::Text => render_text(&diagnostics),
        OutputFormat::Json => render_json(&diagnostics)?,
    };
    if cli.verbose_changes && matches!(cli.output_format(), OutputFormat::Text) {
        output.push_str("\n\n");
        output.push_str(&render_verbose_changes(&result.api_changes));
        output.push_str("\n\n");
        output.push_str(&render_ios_usage_report(&result.ios_usage));
        output.push_str("\n\n");
        output.push_str(&render_mr_debug(&result.debug, &result.api_changes));
    }

    println!("{output}");

    Ok(exit_code_for_diagnostics(&diagnostics))
}

fn exit_code_for_diagnostics(diagnostics: &[Diagnostic]) -> i32 {
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::exit_code_for_diagnostics;
    use crate::config::Severity;
    use crate::model::Diagnostic;

    fn diagnostic_with_severity(severity: Severity) -> Diagnostic {
        Diagnostic {
            code: "test".to_string(),
            severity,
            message: String::new(),
            hint: String::new(),
            kotlin_symbol: None,
            ios_symbol: None,
            member: None,
            kotlin_location: None,
            ios_location: None,
            base_ref: None,
            head_ref: None,
            evidence: Vec::new(),
        }
    }

    #[test]
    fn exit_code_is_zero_when_no_diagnostics() {
        assert_eq!(exit_code_for_diagnostics(&[]), 0);
    }

    #[test]
    fn exit_code_is_zero_for_info_and_warning_only() {
        let diagnostics = vec![
            diagnostic_with_severity(Severity::Info),
            diagnostic_with_severity(Severity::Warning),
        ];
        assert_eq!(exit_code_for_diagnostics(&diagnostics), 0);
    }

    #[test]
    fn exit_code_is_one_when_any_error_present() {
        let diagnostics = vec![
            diagnostic_with_severity(Severity::Info),
            diagnostic_with_severity(Severity::Error),
        ];
        assert_eq!(exit_code_for_diagnostics(&diagnostics), 1);
    }
}
