//! `callus init`: idempotent, non-destructive hook/plugin and MCP installer
//! for Claude Code, `OpenAI` Codex CLI, Antigravity (`agy`), and `OpenCode`.

use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use callus_core::CallusError;
use serde_json::{Map, Value, json};
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

#[derive(Debug, clap::Args)]
#[expect(clippy::struct_excessive_bools, reason = "independent CLI flags")]
pub(crate) struct Args {
    #[arg(long)]
    claude: bool,
    #[arg(long)]
    codex: bool,
    #[arg(long)]
    antigravity: bool,
    #[arg(long)]
    opencode: bool,
    /// Install into project-local config directories instead of user home
    #[arg(long)]
    project: bool,
    /// Show planned changes without writing any files
    #[arg(long)]
    dry_run: bool,
    /// Remove all Callus hooks, plugins and MCP entries cleanly
    #[arg(long)]
    uninstall: bool,
    /// Override base home directory (useful for isolated testing)
    #[arg(long, value_name = "PATH")]
    home: Option<PathBuf>,
}

const CALLUS_SKILL_MD: &str = include_str!("../../../skills/callus/SKILL.md");

const OPENCODE_PLUGIN_TS: &str = r#"// Managed by Callus (`callus init --opencode`). Safe to remove via `--uninstall`.
import { spawnSync } from "node:child_process";

function runHook(event: "pre" | "post", payload: unknown): Record<string, unknown> | null {
  try {
    const res = spawnSync("callus", ["hook", "--cli", "opencode", "--event", event], {
      input: JSON.stringify(payload),
      encoding: "utf8",
      timeout: 500,
    });
    const out = (res.stdout ?? "").trim();
    return out ? (JSON.parse(out) as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

export const callusPlugin = () => ({
  "tool.execute.before": async (input: Record<string, unknown>) => {
    const decision = runHook("pre", input);
    if (decision?.decision === "block" && typeof decision.reason === "string") {
      throw new Error(decision.reason);
    }
  },
  "tool.execute.after": async (input: Record<string, unknown>, output: Record<string, unknown>) => {
    const res = runHook("post", { ...input, output });
    if (typeof res?.hint === "string" && typeof output.output === "string") {
      output.output = `${output.output}\n${res.hint}`;
    }
  },
});
"#;

pub(crate) fn run(args: Args) -> Result<(), CallusError> {
    let base = if args.project {
        std::env::current_dir().map_err(io_err("cannot read current directory"))?
    } else if let Some(h) = args.home {
        h
    } else {
        std::env::home_dir().ok_or_else(|| CallusError::Io {
            context: "cannot determine home directory".to_owned(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "home dir unset"),
        })?
    };

    let any_explicit = args.claude || args.codex || args.antigravity || args.opencode;
    let targets = [
        (
            "claude",
            args.claude || (!any_explicit && base.join(".claude").exists()),
        ),
        (
            "codex",
            args.codex || (!any_explicit && base.join(".codex").exists()),
        ),
        (
            "antigravity",
            args.antigravity
                || (!any_explicit
                    && (base.join(".gemini").exists()
                        || !discover_t3_antigravity_dirs(&base).is_empty())),
        ),
        (
            "opencode",
            args.opencode || (!any_explicit && base.join(".config").join("opencode").exists()),
        ),
    ];

    let selected: Vec<&str> = targets
        .into_iter()
        .filter_map(|(name, enabled)| enabled.then_some(name))
        .collect();

    if selected.is_empty() {
        println!(
            "No supported CLIs detected in {}. Pass --claude, --codex, --antigravity, or --opencode explicitly.",
            base.display()
        );
        return Ok(());
    }

    for cli in selected {
        configure_tool(cli, &base, args.project, args.dry_run, args.uninstall)?;
    }
    Ok(())
}

/// Applies or removes Callus hooks, plugins, and MCP entries for one CLI tool.
pub(crate) fn configure_tool(
    cli: &str,
    base: &Path,
    project: bool,
    dry_run: bool,
    uninstall: bool,
) -> Result<(), CallusError> {
    match cli {
        "claude" => configure_claude(base, project, dry_run, uninstall),
        "codex" => configure_codex(base, project, dry_run, uninstall),
        "antigravity" => configure_antigravity(base, project, dry_run, uninstall),
        "opencode" => configure_opencode(base, project, dry_run, uninstall),
        _ => Ok(()),
    }
}

/// Returns human-readable plan lines describing the exact files touched for `cli`.
pub(crate) fn describe_tool_plan(
    cli: &str,
    base: &Path,
    project: bool,
    uninstall: bool,
) -> Vec<String> {
    let verb = if uninstall {
        "remove Callus from"
    } else {
        "configure Callus in"
    };
    match cli {
        "claude" => {
            let settings = base.join(".claude").join("settings.json");
            let mcp = if project {
                base.join(".mcp.json")
            } else {
                base.join(".claude.json")
            };
            vec![
                format!(
                    "{verb} {} (PreToolUse, PostToolUse, PostToolUseFailure, Stop hooks)",
                    settings.display()
                ),
                format!("{verb} {} (mcpServers.callus)", mcp.display()),
            ]
        }
        "codex" => {
            let cfg = base.join(".codex").join("config.toml");
            let mut lines = vec![format!(
                "{verb} {} (features.hooks, PreToolUse/PostToolUse hooks, mcp_servers.callus)",
                cfg.display()
            )];
            if !project {
                let skill = base
                    .join(".agents")
                    .join("skills")
                    .join("callus")
                    .join("SKILL.md");
                lines.push(if uninstall {
                    format!("remove {}", skill.display())
                } else {
                    format!("install skill {}", skill.display())
                });
            }
            lines
        }
        "antigravity" => {
            if project {
                vec![format!(
                    "{verb} {} (PreToolUse, PostToolUse, Stop hooks)",
                    base.join(".agents").join("hooks.json").display()
                )]
            } else {
                let mut lines = Vec::new();
                for dir in discover_antigravity_plugin_dirs(base) {
                    lines.push(if uninstall {
                        format!("remove owned Antigravity plugin {}", dir.display())
                    } else {
                        format!(
                            "install Antigravity plugin {} (plugin.json, mcp_config.json, hooks.json, SKILL.md)",
                            dir.display()
                        )
                    });
                }
                lines
            }
        }
        "opencode" => {
            let dir = if project {
                base.join(".opencode")
            } else {
                base.join(".config").join("opencode")
            };
            let plugin = dir.join("plugins").join("callus.ts");
            let cfg = dir.join("opencode.json");
            let mut lines = vec![
                if uninstall {
                    format!("remove {}", plugin.display())
                } else {
                    format!("write OpenCode plugin {}", plugin.display())
                },
                format!("{verb} {} (mcp.callus + plugin entry)", cfg.display()),
            ];
            if !project {
                let skill = base
                    .join(".agents")
                    .join("skills")
                    .join("callus")
                    .join("SKILL.md");
                lines.push(if uninstall {
                    format!("remove {}", skill.display())
                } else {
                    format!("install skill {}", skill.display())
                });
            }
            lines
        }
        _ => Vec::new(),
    }
}

fn configure_claude(
    base: &Path,
    project: bool,
    dry_run: bool,
    uninstall: bool,
) -> Result<(), CallusError> {
    let settings_path = base.join(".claude").join("settings.json");
    let mut settings = read_json_object(&settings_path)?;
    let hooks = ensure_object(&mut settings, "hooks");

    for (event_name, cmd) in [
        ("PreToolUse", "callus hook --cli claude --event pre"),
        ("PostToolUse", "callus hook --cli claude --event post"),
        (
            "PostToolUseFailure",
            "callus hook --cli claude --event failure",
        ),
        ("Stop", "callus hook --cli claude --event stop"),
    ] {
        let entry = json!({
            "matcher": "*",
            "hooks": [{ "type": "command", "command": cmd }]
        });
        update_json_hook_array(hooks, event_name, cmd, &entry, uninstall);
    }
    write_json_if_changed(&settings_path, &Value::Object(settings), dry_run)?;

    let mcp_path = if project {
        base.join(".mcp.json")
    } else {
        base.join(".claude.json")
    };
    let mut mcp_doc = read_json_object(&mcp_path)?;
    let servers = ensure_object(&mut mcp_doc, "mcpServers");
    if uninstall {
        servers.remove("callus");
    } else {
        servers.insert(
            "callus".to_owned(),
            json!({ "command": "callus", "args": ["mcp"] }),
        );
    }
    write_json_if_changed(&mcp_path, &Value::Object(mcp_doc), dry_run)?;
    Ok(())
}

fn configure_codex(
    base: &Path,
    project: bool,
    dry_run: bool,
    uninstall: bool,
) -> Result<(), CallusError> {
    let config_path = base.join(".codex").join("config.toml");
    let existing = if config_path.exists() {
        std::fs::read_to_string(&config_path)
            .map_err(io_err(format!("cannot read {}", config_path.display())))?
    } else {
        String::new()
    };
    let mut doc: DocumentMut = existing.parse().map_err(|e| CallusError::Io {
        context: format!("invalid TOML in {}", config_path.display()),
        source: std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{e}")),
    })?;

    if !uninstall {
        let features = doc
            .entry("features")
            .or_insert_with(|| Item::Table(Table::new()));
        if let Some(tbl) = features.as_table_mut() {
            tbl["hooks"] = value(true);
        }
    }

    let hooks_item = doc
        .entry("hooks")
        .or_insert_with(|| Item::Table(Table::new()));
    if let Some(hooks_tbl) = hooks_item.as_table_mut() {
        for (event_name, cmd) in [
            ("PreToolUse", "callus hook --cli codex --event pre"),
            ("PostToolUse", "callus hook --cli codex --event post"),
        ] {
            update_codex_hook_array(hooks_tbl, event_name, cmd, uninstall);
        }
    }

    let mcp_item = doc
        .entry("mcp_servers")
        .or_insert_with(|| Item::Table(Table::new()));
    if let Some(mcp_tbl) = mcp_item.as_table_mut() {
        if uninstall {
            mcp_tbl.remove("callus");
        } else {
            let mut srv = Table::new();
            srv["command"] = value("callus");
            let mut arr = toml_edit::Array::new();
            arr.push("mcp");
            srv["args"] = value(arr);
            mcp_tbl["callus"] = Item::Table(srv);
        }
    }

    let rendered = doc.to_string();
    write_text_if_changed(&config_path, &rendered, dry_run)?;
    if !project {
        sync_agents_skill(base, dry_run, uninstall)?;
    }
    if project && !uninstall {
        println!(
            "Note: approve project hooks in Codex CLI with `codex /hooks` for {}.",
            config_path.display()
        );
    }
    Ok(())
}

/// Discovers initialized T3 Code Antigravity profile plugin folders under
/// `<home>/.t3/userdata/providers/antigravity/*/config/plugins/callus`.
pub(crate) fn discover_t3_antigravity_dirs(base: &Path) -> Vec<PathBuf> {
    let root = base
        .join(".t3")
        .join("userdata")
        .join("providers")
        .join("antigravity");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("config").is_dir() {
            dirs.push(path.join("config").join("plugins").join("callus"));
        }
    }
    dirs.sort();
    dirs
}

fn discover_antigravity_plugin_dirs(base: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![
        base.join(".gemini")
            .join("config")
            .join("plugins")
            .join("callus"),
    ];
    dirs.extend(discover_t3_antigravity_dirs(base));
    dirs
}

fn configure_antigravity(
    base: &Path,
    project: bool,
    dry_run: bool,
    uninstall: bool,
) -> Result<(), CallusError> {
    if project {
        let hooks_path = base.join(".agents").join("hooks.json");
        let mut hooks_doc = read_json_object(&hooks_path)?;
        if uninstall {
            hooks_doc.remove("callus");
        } else {
            hooks_doc.insert("callus".to_owned(), antigravity_hooks_value());
        }
        write_json_if_changed(&hooks_path, &Value::Object(hooks_doc), dry_run)?;
        return Ok(());
    }

    // Clean up legacy v0.1.0 top-level entries in ~/.gemini/config/{hooks,mcp_config}.json if present.
    cleanup_legacy_antigravity_top_level(base, dry_run)?;

    let plugin_json = json!({
        "$schema": "https://antigravity.google/schemas/v1/plugin.json",
        "name": "callus",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Callus mistake memory, PreToolUse guards, and MCP tools"
    });
    let mcp_json = json!({
        "mcpServers": {
            "callus": {
                "command": "callus",
                "args": ["mcp"]
            }
        }
    });
    let hooks_json = json!({
        "callus": antigravity_hooks_value()
    });
    let marker_json = json!({
        "managed_by": "callus-installer",
        "tool": "antigravity",
        "version": env!("CARGO_PKG_VERSION")
    });

    for plugin_dir in discover_antigravity_plugin_dirs(base) {
        let marker_path = plugin_dir.join(".callus-plugin.json");
        if uninstall {
            if marker_path.is_file() {
                if dry_run {
                    println!("[dry-run] would remove {}", plugin_dir.display());
                } else {
                    std::fs::remove_dir_all(&plugin_dir)
                        .map_err(io_err(format!("cannot remove {}", plugin_dir.display())))?;
                    println!("removed {}", plugin_dir.display());
                }
            }
            continue;
        }

        write_json_if_changed(&plugin_dir.join("plugin.json"), &plugin_json, dry_run)?;
        write_json_if_changed(&plugin_dir.join("mcp_config.json"), &mcp_json, dry_run)?;
        write_json_if_changed(&plugin_dir.join("hooks.json"), &hooks_json, dry_run)?;
        write_text_if_changed(
            &plugin_dir.join("skills").join("callus").join("SKILL.md"),
            CALLUS_SKILL_MD,
            dry_run,
        )?;
        write_json_if_changed(&marker_path, &marker_json, dry_run)?;
    }
    Ok(())
}

fn antigravity_hooks_value() -> Value {
    json!({
        "PreToolUse": [{
            "matcher": "*",
            "type": "command",
            "command": "callus hook --cli antigravity --event pre",
            "timeout": 10
        }],
        "PostToolUse": [{
            "matcher": "*",
            "type": "command",
            "command": "callus hook --cli antigravity --event post",
            "timeout": 10
        }],
        "Stop": [{
            "type": "command",
            "command": "callus hook --cli antigravity --event stop",
            "timeout": 10
        }]
    })
}

fn cleanup_legacy_antigravity_top_level(base: &Path, dry_run: bool) -> Result<(), CallusError> {
    let legacy_hooks = base.join(".gemini").join("config").join("hooks.json");
    if legacy_hooks.is_file() {
        let mut doc = read_json_object(&legacy_hooks)?;
        if doc.remove("callus").is_some() {
            write_json_if_changed(&legacy_hooks, &Value::Object(doc), dry_run)?;
        }
    }
    let legacy_mcp = base.join(".gemini").join("config").join("mcp_config.json");
    if legacy_mcp.is_file() {
        let mut doc = read_json_object(&legacy_mcp)?;
        if let Some(servers) = doc.get_mut("mcpServers").and_then(Value::as_object_mut)
            && servers.remove("callus").is_some()
        {
            write_json_if_changed(&legacy_mcp, &Value::Object(doc), dry_run)?;
        }
    }
    Ok(())
}

fn sync_agents_skill(base: &Path, dry_run: bool, uninstall: bool) -> Result<(), CallusError> {
    let skill_dir = base.join(".agents").join("skills").join("callus");
    let skill_file = skill_dir.join("SKILL.md");
    if uninstall {
        if skill_file.is_file() {
            if dry_run {
                println!("[dry-run] would remove {}", skill_file.display());
            } else {
                let _ = std::fs::remove_file(&skill_file);
                let _ = std::fs::remove_dir(&skill_dir);
                println!("removed {}", skill_file.display());
            }
        }
        return Ok(());
    }
    write_text_if_changed(&skill_file, CALLUS_SKILL_MD, dry_run)
}

fn configure_opencode(
    base: &Path,
    project: bool,
    dry_run: bool,
    uninstall: bool,
) -> Result<(), CallusError> {
    let config_dir = if project {
        base.join(".opencode")
    } else {
        base.join(".config").join("opencode")
    };
    let plugin_path = config_dir.join("plugins").join("callus.ts");
    if uninstall {
        if plugin_path.exists() {
            if dry_run {
                println!("[dry-run] would remove {}", plugin_path.display());
            } else {
                backup_file(&plugin_path)?;
                std::fs::remove_file(&plugin_path)
                    .map_err(io_err(format!("cannot remove {}", plugin_path.display())))?;
                println!("removed {}", plugin_path.display());
            }
        }
    } else {
        write_text_if_changed(&plugin_path, OPENCODE_PLUGIN_TS, dry_run)?;
    }

    let opencode_json = config_dir.join("opencode.json");
    let mut doc = read_json_object(&opencode_json)?;
    let mcp = ensure_object(&mut doc, "mcp");
    if uninstall {
        mcp.remove("callus");
    } else {
        mcp.insert(
            "callus".to_owned(),
            json!({ "type": "local", "command": ["callus", "mcp"], "enabled": true }),
        );
    }

    let plugin_rel = "./plugins/callus.ts";
    let plugins = doc
        .entry("plugin")
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(arr) = plugins.as_array_mut() {
        arr.retain(|v| v.as_str() != Some(plugin_rel));
        if !uninstall {
            arr.push(Value::String(plugin_rel.to_owned()));
        }
    }
    write_json_if_changed(&opencode_json, &Value::Object(doc), dry_run)?;
    if !project {
        sync_agents_skill(base, dry_run, uninstall)?;
    }
    Ok(())
}

fn update_json_hook_array(
    hooks: &mut Map<String, Value>,
    event_name: &str,
    cmd: &str,
    entry: &Value,
    uninstall: bool,
) {
    let list = hooks
        .entry(event_name.to_owned())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(arr) = list.as_array_mut() else {
        return;
    };
    arr.retain(|item| !json_entry_mentions_callus(item, cmd));
    if !uninstall {
        arr.push(entry.clone());
    }
    if uninstall && arr.is_empty() {
        hooks.remove(event_name);
    }
}

fn json_entry_mentions_callus(item: &Value, cmd: &str) -> bool {
    if item.get("command").and_then(Value::as_str) == Some(cmd) {
        return true;
    }
    item.get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|nested| {
            nested
                .iter()
                .any(|h| h.get("command").and_then(Value::as_str) == Some(cmd))
        })
}

fn codex_table_mentions_callus(tbl: &Table, cmd: &str) -> bool {
    if tbl.get("command").and_then(Item::as_str) == Some(cmd) {
        return true;
    }
    tbl.get("hooks")
        .and_then(Item::as_array_of_tables)
        .is_some_and(|nested| {
            nested
                .iter()
                .any(|h| h.get("command").and_then(Item::as_str) == Some(cmd))
        })
}

fn update_codex_hook_array(hooks_tbl: &mut Table, event_name: &str, cmd: &str, uninstall: bool) {
    let mut kept = ArrayOfTables::new();
    if let Some(existing) = hooks_tbl.get(event_name).and_then(Item::as_array_of_tables) {
        for tbl in existing {
            if !codex_table_mentions_callus(tbl, cmd) {
                kept.push(tbl.clone());
            }
        }
    }
    if !uninstall {
        let mut entry = Table::new();
        entry["matcher"] = value("*");
        let mut inner = ArrayOfTables::new();
        let mut hook_cmd = Table::new();
        hook_cmd["type"] = value("command");
        hook_cmd["command"] = value(cmd);
        hook_cmd["timeout"] = value(10);
        inner.push(hook_cmd);
        entry["hooks"] = Item::ArrayOfTables(inner);
        kept.push(entry);
    }
    if kept.is_empty() {
        hooks_tbl.remove(event_name);
    } else {
        hooks_tbl[event_name] = Item::ArrayOfTables(kept);
    }
}

fn read_json_object(path: &Path) -> Result<Map<String, Value>, CallusError> {
    if !path.exists() {
        return Ok(Map::new());
    }
    let raw =
        std::fs::read_to_string(path).map_err(io_err(format!("cannot read {}", path.display())))?;
    if raw.trim().is_empty() {
        return Ok(Map::new());
    }
    let parsed: Value = serde_json::from_str(&raw)?;
    match parsed {
        Value::Object(map) => Ok(map),
        _ => Err(CallusError::Io {
            context: format!("expected JSON object in {}", path.display()),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, "root is not an object"),
        }),
    }
}

fn ensure_object<'a>(map: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let entry = map
        .entry(key.to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    // Safe because we just ensured it is a Value::Object above.
    entry.as_object_mut().expect("ensured JSON object")
}

fn write_json_if_changed(path: &Path, value: &Value, dry_run: bool) -> Result<(), CallusError> {
    let mut rendered = serde_json::to_string_pretty(value)?;
    rendered.push('\n');
    write_text_if_changed(path, &rendered, dry_run)
}

fn write_text_if_changed(path: &Path, content: &str, dry_run: bool) -> Result<(), CallusError> {
    if let Ok(existing) = std::fs::read_to_string(path)
        && existing == content
    {
        println!("unchanged {}", path.display());
        return Ok(());
    }
    if dry_run {
        println!("[dry-run] would update {}", path.display());
        return Ok(());
    }
    if path.exists() {
        backup_file(path)?;
    } else if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(io_err(format!("cannot create {}", parent.display())))?;
    }
    std::fs::write(path, content).map_err(io_err(format!("cannot write {}", path.display())))?;
    println!("updated {}", path.display());
    Ok(())
}

fn backup_file(path: &Path) -> Result<(), CallusError> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut bak = path.as_os_str().to_os_string();
    bak.push(format!(".callus-{ts}.bak"));
    std::fs::copy(path, PathBuf::from(&bak))
        .map_err(io_err(format!("cannot back up {}", path.display())))?;
    Ok(())
}

fn io_err(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> CallusError {
    let context = context.into();
    move |source| CallusError::Io { context, source }
}
