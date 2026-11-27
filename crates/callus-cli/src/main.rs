//! The `callus` binary.

mod adapters;
mod hook;
mod init;
mod scrub_test;

use std::process::ExitCode;

use callus_core::CallusError;
use clap::{Parser, Subcommand, ValueEnum};

/// Local-first mistake memory, guard and rule compiler for AI coding CLIs.
#[derive(Debug, Parser)]
#[command(name = "callus", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Install hooks/plugins and MCP config for supported CLIs
    Init(init::Args),
    /// Run one hook event from stdin (fail-open, <10ms local hot path)
    Hook(hook::Args),
    /// Run the MCP server over stdio (planned)
    Mcp,
    /// Run the HTTP API, MCP endpoint, sync and web UI (planned)
    Serve,
    /// Sync scoped knowledge with a central server (planned)
    Sync,
    /// Export rule diffs, guard hooks or rule packs (planned)
    Export {
        #[arg(long, value_enum)]
        format: Option<ExportFormat>,
    },
    /// Show what the privacy scrubber redacts in a text
    ScrubTest(scrub_test::Args),
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ExportFormat {
    AgentsMd,
    ClaudeMd,
    Hooks,
    Pack,
}

fn run(command: Command) -> Result<(), CallusError> {
    let planned = match command {
        Command::ScrubTest(args) => return scrub_test::run(args),
        Command::Init(args) => return init::run(args),
        Command::Hook(args) => {
            hook::run(args);
            return Ok(());
        }
        Command::Mcp => "mcp",
        Command::Serve => "serve",
        Command::Sync => "sync",
        Command::Export { .. } => "export",
    };
    Err(CallusError::NotImplemented(planned))
}

fn main() -> ExitCode {
    match run(Cli::parse().command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("callus: {error}");
            ExitCode::FAILURE
        }
    }
}
