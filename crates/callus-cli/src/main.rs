//! The `callus` binary.

mod adapters;
mod export;
mod hook;
mod init;
mod mcp;
mod scrub_test;
mod server;

use std::process::ExitCode;

use callus_core::CallusError;
use clap::{Parser, Subcommand};

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
    /// Run the official Rust MCP server (`rmcp`) over stdio
    Mcp(mcp::Args),
    /// Run the central HTTP REST API and Streamable HTTP MCP server (`/mcp`)
    Serve(server::Args),
    /// Sync scoped knowledge with a central server (planned for CL-12)
    Sync,
    /// Export budgeted rule diffs, deterministic guard hooks, or rule packs
    Export(export::Args),
    /// Show what the privacy scrubber redacts in a text
    ScrubTest(scrub_test::Args),
}

fn run(command: Command) -> Result<(), CallusError> {
    let planned = match command {
        Command::ScrubTest(args) => return scrub_test::run(args),
        Command::Init(args) => return init::run(args),
        Command::Hook(args) => {
            hook::run(args);
            return Ok(());
        }
        Command::Mcp(args) => return mcp::run(args),
        Command::Serve(args) => return server::run(args),
        Command::Export(args) => return export::run(args),
        Command::Sync => "sync",
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
