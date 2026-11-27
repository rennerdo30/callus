//! Interactive cross-platform installer (`callus install`, `callus update`, `callus uninstall`).
//!
//! Shows the exact per-tool plan (hooks, plugins, MCP configs, timestamped backups),
//! asks `[y/N]` before touching each tool (unless `--yes`), safely defaults to `no` on
//! non-TTY stdin, and starts with an empty database unless `--packs` is passed explicitly.

use std::{
    io::{IsTerminal as _, Write as _},
    path::{Path, PathBuf},
};

use callus_core::CallusError;

use crate::{init, pack};

const ALL_TOOLS: &[&str] = &["claude", "codex", "opencode", "antigravity"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Install,
    Update,
    Uninstall,
}

impl Action {
    const fn verb(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Update => "update",
            Self::Uninstall => "uninstall",
        }
    }
}

#[derive(Debug, clap::Args)]
#[expect(clippy::struct_excessive_bools, reason = "independent CLI flags")]
pub(crate) struct Args {
    /// Limit to specific CLIs (`claude`, `codex`, `opencode`, `antigravity`); default: all detected
    #[arg(value_name = "TOOL")]
    tools: Vec<String>,
    /// Skip interactive confirmation prompts
    #[arg(short = 'y', long)]
    yes: bool,
    /// Show the exact plan per tool without modifying any files
    #[arg(long)]
    dry_run: bool,
    /// Configure project-local directories instead of user home
    #[arg(long)]
    project: bool,
    /// Import optional OS starter rule packs (opt-in; fresh installs start empty by default)
    #[arg(long, conflicts_with = "no_packs")]
    packs: bool,
    /// Deprecated no-op (fresh installs no longer import starter packs by default)
    #[arg(long, conflicts_with = "packs")]
    no_packs: bool,
    /// Override base home directory (useful for isolated testing)
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,
    /// Override SQLite database path used when importing starter packs
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,
}

pub(crate) fn run(action: Action, args: &Args) -> Result<(), CallusError> {
    let mut unknown = Vec::new();
    for raw in &args.tools {
        let lower = raw.to_ascii_lowercase();
        if !ALL_TOOLS.contains(&lower.as_str()) {
            unknown.push(raw.clone());
        }
    }
    if !unknown.is_empty() {
        return Err(CallusError::Io {
            context: format!(
                "unknown tool: {} (choose from {})",
                unknown.join(", "),
                ALL_TOOLS.join(", ")
            ),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "unknown CLI tool"),
        });
    }

    let base = resolve_base(args.project, args.home.as_deref())?;
    let explicit: Vec<String> = args.tools.iter().map(|s| s.to_ascii_lowercase()).collect();
    let targets: Vec<&str> = if explicit.is_empty() {
        ALL_TOOLS.to_vec()
    } else {
        ALL_TOOLS
            .iter()
            .copied()
            .filter(|t| explicit.iter().any(|e| e == t))
            .collect()
    };

    if args.dry_run {
        println!("Dry run: nothing will be changed.");
    }

    let is_uninstall = action == Action::Uninstall;
    let mut ran_any = false;

    for tool in targets {
        let explicitly_requested = !explicit.is_empty();
        if !explicitly_requested && !is_tool_detected(tool, &base) {
            let bin = binary_name_for(tool);
            if bin == tool {
                println!("{tool} is not installed (not found on PATH); skipping it.");
            } else {
                println!("{tool} is not installed ({bin} not found on PATH); skipping it.");
            }
            continue;
        }

        let steps = init::describe_tool_plan(tool, &base, args.project, is_uninstall);
        println!("\n{tool}: these steps will run ({}):", action.verb());
        for line in &steps {
            println!("  - {line}");
        }
        if !is_uninstall {
            println!("  Note: {}", post_install_note(tool));
        }

        if !args.dry_run && !args.yes && !confirm(&format!("Run them for {tool}? [y/N] ")) {
            println!("Skipped {tool}.");
            continue;
        }

        ran_any = true;
        init::configure_tool(tool, &base, args.project, args.dry_run, is_uninstall)?;
    }

    if action == Action::Install && args.packs && !args.no_packs {
        offer_starter_packs(args)?;
    }

    println!();
    if args.dry_run {
        println!("Dry run done: nothing was changed.");
    } else if !ran_any {
        println!("No agent integrations were changed.");
    } else if is_uninstall {
        println!("Done. Restart your agent sessions. Your ~/.callus database was left untouched.");
    } else {
        println!("Done. Restart your agent sessions to load Callus.");
        println!("Open the local Web UI anytime:  callus ui --port 8080");
    }
    Ok(())
}

fn resolve_base(project: bool, home: Option<&Path>) -> Result<PathBuf, CallusError> {
    if project {
        std::env::current_dir().map_err(|source| CallusError::Io {
            context: "cannot read current directory".to_owned(),
            source,
        })
    } else if let Some(h) = home {
        Ok(h.to_path_buf())
    } else {
        std::env::home_dir().ok_or_else(|| CallusError::Io {
            context: "cannot determine home directory".to_owned(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "home dir unset"),
        })
    }
}

fn binary_name_for(tool: &str) -> &'static str {
    match tool {
        "antigravity" => "agy",
        "claude" => "claude",
        "codex" => "codex",
        "opencode" => "opencode",
        _ => "",
    }
}

fn post_install_note(tool: &str) -> &'static str {
    match tool {
        "claude" => "Restart Claude Code afterwards to load the Callus hooks and MCP server.",
        "codex" => "Start codex, run `/hooks`, and trust the Callus hooks.",
        "opencode" => "Restart OpenCode afterwards to load `./plugins/callus.ts` and MCP.",
        "antigravity" => {
            "Restart agy (or start a new T3 Code thread) afterwards; review `/mcp` and `/hooks`."
        }
        _ => "",
    }
}

fn is_tool_detected(tool: &str, base: &Path) -> bool {
    let dir_present = match tool {
        "claude" => base.join(".claude").exists(),
        "codex" => base.join(".codex").exists(),
        "antigravity" => {
            base.join(".gemini").exists() || !init::discover_t3_antigravity_dirs(base).is_empty()
        }
        "opencode" => base.join(".config").join("opencode").exists(),
        _ => false,
    };
    dir_present || command_on_path(binary_name_for(tool))
}

fn command_on_path(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let Some(path_var) = std::env::var_os("PATH") else {
        return false;
    };
    let exts: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    for dir in std::env::split_paths(&path_var) {
        for ext in exts {
            if dir.join(format!("{name}{ext}")).is_file() {
                return true;
            }
        }
    }
    false
}

fn confirm(prompt: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        println!("{prompt}(no terminal to ask on: no)");
        return false;
    }
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

fn discover_starter_packs() -> Vec<PathBuf> {
    let mut candidate_dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(parent) = exe.parent()
    {
        candidate_dirs.push(parent.join("packs"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidate_dirs.push(cwd.join("packs"));
    }

    let wanted: &[&str] = if cfg!(target_os = "windows") {
        &["windows-pwsh.json", "git-basics.json"]
    } else if cfg!(target_os = "macos") {
        &["macos-zsh.json", "git-basics.json"]
    } else {
        &["git-basics.json"]
    };

    for dir in candidate_dirs {
        let found: Vec<PathBuf> = wanted
            .iter()
            .map(|f| dir.join(f))
            .filter(|p| p.is_file())
            .collect();
        if !found.is_empty() {
            return found;
        }
    }
    Vec::new()
}

fn offer_starter_packs(args: &Args) -> Result<(), CallusError> {
    let packs = discover_starter_packs();
    if packs.is_empty() {
        return Ok(());
    }
    let names: Vec<String> = packs
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    println!("\nStarter MIT rule packs available: {}", names.join(", "));
    if args.dry_run {
        println!("  [dry-run] would import {}", names.join(", "));
        return Ok(());
    }
    let should_import = args.packs
        || args.yes
        || confirm(&format!(
            "Import starter packs ({})? [y/N] ",
            names.join(", ")
        ));
    if !should_import {
        println!("Skipped starter rule packs.");
        return Ok(());
    }
    for path in packs {
        pack::import_pack_file(&path, false, args.db.clone())?;
    }
    Ok(())
}
