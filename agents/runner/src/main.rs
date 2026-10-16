//! CommonRoom agent runner.
//!
//! One process, one V8 isolate per agent: a Cloudflare-Worker-shaped platform
//! where the runner is a chat bot that owns models, tools and isolates, and each
//! agent is a workspace under `data/`.
//!
//! Layout, all under `data/`:
//!
//! ```text
//! data/config.json          shared with the chat server; the runner owns the `agents` key
//! data/workspace<Name>/     one agent: agent.json, agent.token, state.json, agent.log, files/
//! ```
//!
//! The binary is a CLI. `serve` is the real mode; the others exist so an
//! operator can inspect state without a chat credential.
//!
//! ```text
//! runner serve [--config PATH] [--data DIR]
//! runner list  [--config PATH]
//! runner config [--config PATH]
//! ```

mod cli;

use runner_lib::config::LoadedConfig;
use runner_lib::errors::{Error, Result};
use runner_lib::workspace;

use cli::ParsedArgs;

const USAGE: &str = "\
CommonRoom agent runner

Usage: runner <command> [options]

Commands:
  serve           run the agents, the control commands and the stdin console
  list            list agent workspaces and their state
  config          print the resolved configuration and where it was read from

Options:
  --config PATH   configuration file (default data/config.json, env AGENTS_CONFIG)
  --data DIR      data directory (default: the config file's directory, env AGENTS_DATA)
  -h, --help      show this help

Environment:
  AGENTS_CONFIG   configuration file path
  AGENTS_DATA     data directory
  AGENTS_LOG      tracing filter, overrides `log_level`
  RUST_LOG        tracing filter, used when AGENTS_LOG is unset

Each agent is a directory under the data root, `data/workspace<Name>/`, holding
its own descriptor, credential, durable state, log and files. One V8 isolate per
agent runs inside this process.
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = match ParsedArgs::parse(&args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    if parsed.help {
        println!("{USAGE}");
        return;
    }

    let config_path = parsed.config.clone();
    let data_dir = parsed.data.clone();

    // `config` creates the file when it is missing, because "show me the
    // settings" on a fresh install should show the defaults the runner would
    // use, not an error. `list` is genuinely read-only: an installation with no
    // configuration yet has no agents, which is a fact worth printing rather
    // than a failure.
    let loaded = match parsed.command.as_str() {
        "list" => match resolve_read_only(config_path.as_deref(), data_dir.as_deref()) {
            Ok(Some(loaded)) => Ok(loaded),
            Ok(None) => {
                println!("no configuration yet; run `runner serve` to create one with defaults");
                return;
            }
            Err(error) => Err(error),
        },
        _ => {
            LoadedConfig::load(config_path.as_deref(), data_dir.as_deref()).map(|(loaded, note)| {
                if parsed.command != "config" {
                    println!("{}", note.describe());
                }
                loaded
            })
        }
    };

    let loaded = match loaded {
        Ok(loaded) => loaded,
        Err(error) => fail(&error),
    };

    match parsed.command.as_str() {
        "config" => print_config(&loaded),
        "list" => match print_list(&loaded) {
            Ok(()) => {}
            Err(error) => fail(&error),
        },
        "serve" => {
            // Implemented in a later layer: the isolate pool, the chat client and
            // the console.
            eprintln!(
                "serve is not wired up yet; configuration loaded from {}",
                loaded.config_path.display()
            );
            std::process::exit(1);
        }
        other => {
            eprintln!("unknown command `{other}`\n\n{USAGE}");
            std::process::exit(2);
        }
    }
}

fn fail(error: &Error) -> ! {
    eprintln!("runner: {error}");
    std::process::exit(1);
}

/// Load configuration without creating anything.
///
/// Returns `Ok(None)` when the file simply is not there yet, which is the normal
/// state of a fresh checkout. An explicit `--config` path that does not exist is
/// still reported: the operator named that file, so silence would hide a typo.
fn resolve_read_only(
    config: Option<&std::path::Path>,
    data: Option<&std::path::Path>,
) -> Result<Option<LoadedConfig>> {
    let (path, _) = runner_lib::config::resolve_paths(config, data);
    if !path.exists() {
        if config.is_some() {
            return Err(Error::config(
                path,
                "file not found; run `runner serve` with this path to create it with defaults",
            ));
        }
        return Ok(None);
    }
    LoadedConfig::load_read_only(&path).map(Some)
}

fn print_config(loaded: &LoadedConfig) {
    println!("config file:    {}", loaded.config_path.display());
    println!("data directory: {}", loaded.data_dir.display());
    println!("workspaces:     {}", loaded.workspace_root.display());
    match serde_json::to_string_pretty(&loaded.config) {
        Ok(text) => println!("{text}"),
        Err(error) => eprintln!("runner: cannot render configuration: {error}"),
    }
}

fn print_list(loaded: &LoadedConfig) -> Result<()> {
    let workspaces = workspace::list(loaded)?;
    if workspaces.is_empty() {
        println!("no agents under {}", loaded.workspace_root.display());
        return Ok(());
    }
    println!("{:<16} {:<10} {:<8} MODULE", "AGENT", "DIRECTORY", "STATE");
    for workspace in workspaces {
        let state = if workspace.record.enabled {
            "enabled"
        } else {
            "disabled"
        };
        let token = if workspace.token()?.is_some() {
            "token set"
        } else {
            "no token"
        };
        let module = match &workspace.record.module {
            workspace::AgentModule::Esm { path } => path.clone(),
        };
        println!(
            "{:<16} {:<10} {:<8} {module}  ({token})",
            workspace.name, workspace.directory, state
        );
    }
    Ok(())
}
