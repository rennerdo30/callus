//! `callus pack`: import, export, and inspect versioned, human-reviewable MIT community rule packs.

use std::path::PathBuf;

use callus_core::{
    CallusError, EdgeType, EmbeddingStatus, ErrorPattern, Fix, FixSource, FixStatus, FixType,
    GraphNode, NodeKind, Scope, Scrubber, ScrubberConfig, compute_signature_hash,
};
use callus_store::{KnowledgeStore, SqliteStore};
use clap::Subcommand;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    adapters::now_iso_utc,
    export::{ExportScope, collect_candidates},
    hook::{configured_dims, resolve_db_path},
};

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    #[command(subcommand)]
    command: PackCommand,
}

#[derive(Debug, Subcommand)]
enum PackCommand {
    /// Import a curated JSON rule pack into the local Callus database
    Import {
        /// Path to the `.json` rule pack file
        path: PathBuf,
        /// Mark imported rules as `verified` immediately (default preserves pack status)
        #[arg(long)]
        verify: bool,
        /// Override local SQLite database path
        #[arg(long, value_name = "PATH")]
        db: Option<PathBuf>,
    },
    /// Export verified non-personal learned rules as a shareable JSON rule pack
    Export {
        /// Human-readable pack slug (e.g. `my-team-rust`)
        #[arg(long, default_value = "custom-pack")]
        name: String,
        /// Optional description
        #[arg(long, default_value = "Exported Callus community rule pack")]
        description: String,
        /// Output file path (defaults to stdout)
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,
        /// Override local SQLite database path
        #[arg(long, value_name = "PATH")]
        db: Option<PathBuf>,
    },
    /// List installed `public_pack` rules in the local database
    List {
        /// Override local SQLite database path
        #[arg(long, value_name = "PATH")]
        db: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RulePack {
    pub schema_version: u32,
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub shell: Option<String>,
    pub rules: Vec<PackRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PackRule {
    pub id: String,
    pub title: String,
    pub error_template: String,
    #[serde(default = "default_tool")]
    pub tool_name: String,
    pub fix_summary: String,
    #[serde(default = "default_fix_type")]
    pub fix_type: FixType,
    #[serde(default)]
    pub pattern_before: Option<String>,
    #[serde(default)]
    pub pattern_after: Option<String>,
    #[serde(default)]
    pub guard_regex: Option<String>,
    #[serde(default = "default_status")]
    pub status: FixStatus,
}

fn default_tool() -> String {
    "shell".to_owned()
}

const fn default_fix_type() -> FixType {
    FixType::Hint
}

const fn default_status() -> FixStatus {
    FixStatus::Verified
}

pub(crate) fn run(args: Args) -> Result<(), CallusError> {
    match args.command {
        PackCommand::Import { path, verify, db } => import_pack(&path, verify, db),
        PackCommand::Export {
            name,
            description,
            out,
            db,
        } => export_pack(&name, &description, out.as_deref(), db),
        PackCommand::List { db } => list_packs(db),
    }
}

async fn import_single_rule(
    store: &SqliteStore,
    scrubber: &Scrubber,
    pack: &RulePack,
    rule: &PackRule,
    env_node_id: Option<&str>,
    force_verify: bool,
    now: &str,
) -> Result<bool, callus_store::StoreError> {
    if let Some(regex) = rule.guard_regex.as_deref()
        && !regex.trim().is_empty()
        && Regex::new(regex).is_err()
    {
        return Err(callus_store::StoreError::InvalidData {
            context: "guard_regex",
            detail: format!("invalid regex in rule {}: {regex}", rule.id),
        });
    }

    let title = scrubber.scrub_text(&rule.title).scrubbed;
    let template = scrubber.scrub_text(&rule.error_template).scrubbed;
    let summary = scrubber.scrub_text(&rule.fix_summary).scrubbed;
    let before = rule
        .pattern_before
        .as_deref()
        .map(|s| scrubber.scrub_text(s).scrubbed);
    let after = rule
        .pattern_after
        .as_deref()
        .map(|s| scrubber.scrub_text(s).scrubbed);
    let sig = compute_signature_hash(&rule.tool_name, &template);
    let status = if force_verify {
        FixStatus::Verified
    } else {
        rule.status
    };

    let pat = store
        .upsert_pattern(&ErrorPattern {
            id: format!("pack-{}-{}", pack.name, rule.id),
            signature_hash: sig,
            title: title.clone(),
            error_template: template,
            tool_name: rule.tool_name.clone(),
            scope: Scope::PublicPack,
            repo_hash: None,
            occurrence_count: 1,
            first_seen_at: now.to_owned(),
            last_seen_at: now.to_owned(),
            embedding_status: EmbeddingStatus::Pending,
            updated_at: now.to_owned(),
        })
        .await?;

    let fix = store
        .upsert_fix(&Fix {
            id: format!("fix-pack-{}-{}", pack.name, rule.id),
            summary: summary.clone(),
            fix_type: rule.fix_type,
            pattern_before: before,
            pattern_after: after,
            guard_regex: rule.guard_regex.clone(),
            status,
            scope: Scope::PublicPack,
            source: FixSource::Pack,
            created_at: now.to_owned(),
            updated_at: now.to_owned(),
        })
        .await?;

    let err_node = format!("err:{}", pat.id);
    let fix_node = format!("fix:{}", fix.id);
    store
        .upsert_node(&GraphNode {
            id: err_node.clone(),
            kind: NodeKind::ErrorPattern,
            label: title,
            props_json: json!({ "pack": pack.name, "version": pack.version }),
        })
        .await?;
    store
        .upsert_node(&GraphNode {
            id: fix_node.clone(),
            kind: NodeKind::Fix,
            label: summary,
            props_json: json!({ "status": fix.status.as_str(), "pack": pack.name }),
        })
        .await?;
    store
        .record_edge_feedback(&err_node, EdgeType::ResolvedBy, &fix_node, 3, 0, 3, now)
        .await?;

    if let Some(env_id) = env_node_id {
        store
            .record_edge_feedback(&err_node, EdgeType::OccursIn, env_id, 0, 0, 1, now)
            .await?;
    }

    Ok(fix.fix_type == FixType::PreToolGuard && fix.guard_regex.is_some())
}

fn import_pack(
    path: &std::path::Path,
    force_verify: bool,
    db: Option<PathBuf>,
) -> Result<(), CallusError> {
    let raw = std::fs::read_to_string(path).map_err(|source| CallusError::Io {
        context: format!("cannot read pack {}", path.display()),
        source,
    })?;
    let pack: RulePack = serde_json::from_str(&raw)?;
    if pack.schema_version != 1 {
        return Err(CallusError::Io {
            context: format!("unsupported pack schema_version {}", pack.schema_version),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "expected schema_version 1",
            ),
        });
    }

    let db_path = resolve_db_path(db);
    if let Some(parent) = db_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|source| CallusError::Io {
            context: format!("cannot create {}", parent.display()),
            source,
        })?;
    }
    let store = SqliteStore::open(&db_path, configured_dims()).map_err(|e| store_err(&e))?;
    let scrubber = Scrubber::new(&ScrubberConfig::default())?;
    let rt = single_rt()?;
    let now = now_iso_utc();

    let mut imported = 0_usize;
    let mut guards = 0_usize;

    rt.block_on(async {
        let env_node_id = match (pack.os.as_deref(), pack.shell.as_deref()) {
            (Some(os), Some(shell)) => {
                let id = format!("env:{os}/{shell}");
                store
                    .upsert_node(&GraphNode {
                        id: id.clone(),
                        kind: NodeKind::Environment,
                        label: format!("{os}/{shell}"),
                        props_json: json!({ "os": os, "shell": shell }),
                    })
                    .await?;
                Some(id)
            }
            _ => None,
        };

        for rule in &pack.rules {
            if import_single_rule(
                &store,
                &scrubber,
                &pack,
                rule,
                env_node_id.as_deref(),
                force_verify,
                &now,
            )
            .await?
            {
                guards += 1;
            }
            imported += 1;
        }
        Ok::<(), callus_store::StoreError>(())
    })
    .map_err(|e| store_err(&e))?;

    println!(
        "Imported pack '{}' v{}: {} rule(s) ({} pre-tool guard(s)).",
        pack.name, pack.version, imported, guards
    );
    Ok(())
}

fn export_pack(
    name: &str,
    description: &str,
    out: Option<&std::path::Path>,
    db: Option<PathBuf>,
) -> Result<(), CallusError> {
    let db_path = resolve_db_path(db);
    let store = SqliteStore::open(&db_path, configured_dims()).map_err(|e| store_err(&e))?;
    let scrubber = Scrubber::new(&ScrubberConfig::default())?;
    let rt = single_rt()?;
    let now = now_iso_utc();

    let candidates = rt
        .block_on(collect_candidates(
            &store,
            ExportScope::Global,
            None,
            90,
            &now,
        ))
        .map_err(|e| store_err(&e))?;

    let mut rules = Vec::new();
    for cand in candidates
        .into_iter()
        .filter(|c| !c.stale && c.scope != Scope::Personal)
    {
        let pat = rt
            .block_on(store.get_pattern(&cand.pattern_id))
            .map_err(|e| store_err(&e))?;
        let fix = rt
            .block_on(store.get_fix(&cand.fix_id))
            .map_err(|e| store_err(&e))?;
        if let (Some(pat), Some(fix)) = (pat, fix) {
            rules.push(PackRule {
                id: pat.id,
                title: scrubber.scrub_text(&pat.title).scrubbed,
                error_template: scrubber.scrub_text(&pat.error_template).scrubbed,
                tool_name: pat.tool_name,
                fix_summary: scrubber.scrub_text(&fix.summary).scrubbed,
                fix_type: fix.fix_type,
                pattern_before: fix.pattern_before.map(|s| scrubber.scrub_text(&s).scrubbed),
                pattern_after: fix.pattern_after.map(|s| scrubber.scrub_text(&s).scrubbed),
                guard_regex: fix.guard_regex,
                status: fix.status,
            });
        }
    }

    let pack = RulePack {
        schema_version: 1,
        name: name.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        description: description.to_owned(),
        os: None,
        shell: None,
        rules,
    };
    let mut rendered = serde_json::to_string_pretty(&pack)?;
    rendered.push('\n');

    if let Some(path) = out {
        std::fs::write(path, rendered).map_err(|source| CallusError::Io {
            context: format!("cannot write pack {}", path.display()),
            source,
        })?;
        println!(
            "Exported {} rule(s) to {}",
            pack.rules.len(),
            path.display()
        );
    } else {
        print!("{rendered}");
    }
    Ok(())
}

fn list_packs(db: Option<PathBuf>) -> Result<(), CallusError> {
    let db_path = resolve_db_path(db);
    let store = SqliteStore::open(&db_path, configured_dims()).map_err(|e| store_err(&e))?;
    let rt = single_rt()?;
    let patterns = rt
        .block_on(store.list_patterns(Some(Scope::PublicPack), None, 200))
        .map_err(|e| store_err(&e))?;

    if patterns.is_empty() {
        println!(
            "No public_pack rules imported yet. Run `callus pack import packs/windows-pwsh.json`."
        );
        return Ok(());
    }
    for pat in patterns {
        println!("{}  [{}]  {}", pat.id, pat.tool_name, pat.title);
    }
    Ok(())
}

fn single_rt() -> Result<tokio::runtime::Runtime, CallusError> {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|source| CallusError::Io {
            context: "cannot start tokio runtime".to_owned(),
            source,
        })
}

fn store_err(err: &callus_store::StoreError) -> CallusError {
    CallusError::Io {
        context: "storage error".to_owned(),
        source: std::io::Error::other(err.to_string()),
    }
}
