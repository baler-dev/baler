use anyhow::Result;
use baler_core::baler_toml::BalerToml;
use clap::{Parser, Subcommand};
use std::fs;
use std::path::Path;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Codegen {
        #[arg(long)]
        check: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Codegen { check } => codegen(check),
    }
}

fn codegen(check: bool) -> Result<()> {
    let schema = schemars::schema_for!(BalerToml);
    let rendered = serde_json::to_string_pretty(&schema)?;
    write_or_check("artifacts/baler.schema.json", &rendered, check)
}

fn write_or_check(path: &str, content: &str, check: bool) -> Result<()> {
    let path = Path::new(path);
    if check {
        let existing = fs::read_to_string(path).unwrap_or_default();
        if existing.trim() != content.trim() {
            anyhow::bail!("{} is stale, run `cargo xtask codegen`", path.display());
        }
        return Ok(());
    }
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(path, content)?;
    Ok(())
}
