use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Parser)]
#[command(name = "kmpolice")]
#[command(about = "Checks Kotlin KMP interfaces against iOS Swift contracts")]
pub struct Cli {
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,
    #[command(subcommand)]
    pub command: CheckCommand,
}

impl Cli {
    pub fn config_path(&self) -> Option<&Path> {
        self.config.as_deref()
    }

    pub fn output_format(&self) -> OutputFormat {
        self.format
    }
}

#[derive(Debug, Subcommand)]
pub enum CheckCommand {
    Check(CheckGroup),
    #[command(
        about = "Best-effort direct path check without git diff context (use at your own risk)"
    )]
    Paths(PathsArgs),
    Git(GitArgs),
    Mr(MrArgs),
}

#[derive(Debug, Args)]
pub struct CheckGroup {
    #[command(subcommand)]
    pub command: NestedCheckCommand,
}

#[derive(Debug, Subcommand)]
pub enum NestedCheckCommand {
    #[command(
        about = "Best-effort direct path check without git diff context (use at your own risk)"
    )]
    Paths(PathsArgs),
    Git(GitArgs),
    Mr(MrArgs),
}

impl From<NestedCheckCommand> for CheckCommand {
    fn from(value: NestedCheckCommand) -> Self {
        match value {
            NestedCheckCommand::Paths(args) => CheckCommand::Paths(args),
            NestedCheckCommand::Git(args) => CheckCommand::Git(args),
            NestedCheckCommand::Mr(args) => CheckCommand::Mr(args),
        }
    }
}

impl Cli {
    pub fn normalized_command(self) -> Self {
        match self.command {
            CheckCommand::Check(group) => Self {
                config: self.config,
                format: self.format,
                command: group.command.into(),
            },
            _ => self,
        }
    }
}

#[derive(Debug, Args)]
pub struct PathsArgs {
    #[arg(long)]
    pub kotlin: PathBuf,
    #[arg(long)]
    pub ios: PathBuf,
}

#[derive(Debug, Args)]
pub struct GitArgs {
    #[arg(long)]
    pub repo: PathBuf,
    #[arg(long)]
    pub base_ref: String,
    #[arg(long)]
    pub head_ref: String,
    #[arg(long)]
    pub introduced_only: bool,
}

#[derive(Debug, Args)]
pub struct MrArgs {
    #[arg(long)]
    pub repo: PathBuf,
    #[arg(long, default_value = "develop")]
    pub target: String,
}
