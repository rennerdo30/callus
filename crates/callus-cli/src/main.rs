//! The `callus` binary.

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
    /// Install hooks/plugins and MCP config for the installed CLIs (planned)
    Init(InitArgs),
    /// Run one hook event from stdin (planned)
    Hook {
        /// Calling CLI
        #[arg(long)]
        cli: Option<String>,
        /// pre, post, failure or stop
        #[arg(long)]
        event: Option<String>,
    },
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

#[derive(Debug, clap::Args)]
#[expect(clippy::struct_excessive_bools, reason = "independent CLI flags")]
struct InitArgs {
    #[arg(long)]
    claude: bool,
    #[arg(long)]
    codex: bool,
    #[arg(long)]
    antigravity: bool,
    #[arg(long)]
    opencode: bool,
    /// Show the changes without applying them
    #[arg(long)]
    dry_run: bool,
    /// Remove everything Callus installed
    #[arg(long)]
    uninstall: bool,
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
        // Hooks fail open: never disturb the calling agent.
        Command::Hook { .. } => {
            eprintln!("callus: {}", CallusError::NotImplemented("hook"));
            return Ok(());
        }
        Command::Init(_) => "init",
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
