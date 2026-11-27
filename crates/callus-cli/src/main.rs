//! The `callus` binary.

mod adapters;
mod export;
mod hook;
mod init;
mod installer;
mod mcp;
mod pack;
mod scrub_test;
mod server;
mod sync;
mod ui;
mod worker;

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
    /// Interactive installer for Claude Code, Codex, `OpenCode`, and Antigravity
    Install(installer::Args),
    /// Refresh installed Callus hooks, plugins, and MCP entries across CLIs
    Update(installer::Args),
    /// Remove Callus hooks, plugins, and MCP entries (leaves ~/.callus data intact)
    Uninstall(installer::Args),
    /// Low-level non-interactive hook/plugin/MCP setup (alias for scripted installs)
    Init(init::Args),
    /// Run one hook event from stdin (fail-open, <10ms local hot path)
    Hook(hook::Args),
    /// Run the official Rust MCP server (`rmcp`) over stdio
    Mcp(mcp::Args),
    /// Run the central HTTP REST API and Streamable HTTP MCP server (`/mcp`)
    Serve(server::Args),
    /// Launch the embedded local Web UI against the local SQLite database
    Ui(ui::Args),
    /// Bidirectional scoped sync with a central Callus server
    Sync(sync::Args),
    /// Export budgeted rule diffs, deterministic guard hooks, or rule packs
    Export(export::Args),
    /// Import, export, or list curated community rule packs
    Pack(pack::Args),
    /// Show what the privacy scrubber redacts in a text
    ScrubTest(scrub_test::Args),
}

fn run(command: Command) -> Result<(), CallusError> {
    match command {
        Command::Install(args) => installer::run(installer::Action::Install, &args),
        Command::Update(args) => installer::run(installer::Action::Update, &args),
        Command::Uninstall(args) => installer::run(installer::Action::Uninstall, &args),
        Command::ScrubTest(args) => scrub_test::run(args),
        Command::Init(args) => init::run(args),
        Command::Hook(args) => {
            hook::run(args);
            Ok(())
        }
        Command::Mcp(args) => mcp::run(args),
        Command::Serve(args) => server::run(args),
        Command::Ui(args) => ui::run(args),
        Command::Sync(args) => sync::run(args),
        Command::Export(args) => export::run(args),
        Command::Pack(args) => pack::run(args),
    }
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
