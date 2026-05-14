use std::path::{Path, PathBuf};

use clap::{Parser, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Parser)]
#[command(name = "kmpolice")]
#[command(about = "Checks Kotlin KMP interfaces against iOS Swift contracts")]
pub struct Cli {
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,
    #[arg(long, default_value = ".")]
    pub repo: PathBuf,
    #[arg(long, default_value = "develop")]
    pub target: String,
    #[arg(long = "verbose-changes", default_value_t = false)]
    pub verbose_changes: bool,
    #[arg(long, default_value_t = false)]
    pub mock_progress: bool,
    #[arg(long, default_value_t = 6000)]
    pub mock_kotlin_files: usize,
    #[arg(long, default_value_t = 6000)]
    pub mock_ios_files: usize,
}

impl Cli {
    pub fn config_path(&self) -> Option<&Path> {
        self.config.as_deref()
    }

    pub fn output_format(&self) -> OutputFormat {
        self.format
    }
}
