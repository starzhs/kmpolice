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
use config::Config;
use mr::{render_ios_usage_report, render_verbose_changes, run_mock_progress, run_mr};
use report::{render_json, render_text};

pub fn run() -> Result<i32> {
    let cli = Cli::parse();
    let config = Config::load(cli.config_path())?;
    let result = if cli.mock_progress {
        run_mock_progress(cli.mock_kotlin_files, cli.mock_ios_files)
    } else {
        run_mr(&cli.repo, &cli.target, &config, cli.verbose_changes)?
    };
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
    }

    println!("{output}");

    Ok(if diagnostics.is_empty() { 0 } else { 1 })
}
