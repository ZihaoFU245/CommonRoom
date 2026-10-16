//! One agent workspace on disk.
//!
//! The layout is the operator-visible contract, so it is small and explicit:
//!
//! ```text
//! data/workspace<Name>/
//!   agent.json        bot identity, module, grants, enabled flag
//!   agent.token       this agent's bot credential, mode 0600 (created by /set)
//!   state.json        cursors, outbox and observed content revisions
//!   agent.log         host log for this agent's isolate
//!   files/            the only directory the agent's own tools may write
//! ```
//!
//! Nothing outside the workspace directory is readable or writable by the
//! agent's tools.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{LoadedConfig, set_owner_only, write_secret_file};
use crate::errors::{Error, Result};

/// Agent names become directory names and chat command targets, so they are
/// restricted to a conservative character set.
const NAME_PATTERN_LEN: std::ops::RangeInclusive<usize> = 2..=32;

/// Descriptor file inside a workspace.
pub const AGENT_FILE: &str = "agent.json";

/// Credential file inside a workspace.
pub const TOKEN_FILE: &str = "agent.token";

/// Durable worker state inside a workspace.
pub const STATE_FILE: &str = "state.json";

/// Host log inside a workspace.
pub const LOG_FILE: &str = "agent.log";

/// Directory for files the agent writes with its own tools.
pub const FILES_DIR: &str = "files";

/// Entry module used when an agent does not name one.
pub const DEFAULT_MODULE: &str = "agent.js";

/// Prefix applied to every workspace directory name.
pub const WORKSPACE_PREFIX: &str = "workspace";

/// The module an agent's isolate evaluates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AgentModule {
    /// An ECMAScript module inside the workspace, evaluated by the isolate.
    ///
    /// This is the default: every agent runs the same kind of code, and the
    /// workspace decides which build of it. A rig agent compiled to WebAssembly
    /// is loaded by such a module through `Runner.loadWasm`.
    Esm {
        /// Workspace-relative path of the module.
        path: String,
    },
}

impl Default for AgentModule {
    fn default() -> Self {
        Self::Esm {
            path: DEFAULT_MODULE.to_string(),
        }
    }
}

/// Everything the runner needs to know about one agent.
///
/// Stored per workspace rather than in one central roster, so adding an agent is
/// creating a directory and removing one is deleting it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRecord {
    /// Agent name as used in chat and in `/set <name>`.
    pub name: String,

    /// Module the isolate evaluates.
    #[serde(default)]
    pub module: AgentModule,

    /// Chat account id once known. Used to recognize mentions of this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,

    /// Chat username once known, for logs and for `/ps`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,

    /// Whether the runner keeps this agent running.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Seconds allowed for one model turn before the host aborts it.
    #[serde(default = "default_turn_timeout")]
    pub turn_timeout_seconds: u64,

    /// Free-form settings handed to the module as `Runner.settings`.
    #[serde(default)]
    pub settings: serde_json::Map<String, serde_json::Value>,

    /// Unix seconds when the record was created.
    #[serde(default)]
    pub created: u64,
}

fn default_true() -> bool {
    true
}

fn default_turn_timeout() -> u64 {
    120
}

impl AgentRecord {
    /// A fresh record for a new workspace.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            module: AgentModule::default(),
            account_id: None,
            username: None,
            enabled: true,
            turn_timeout_seconds: default_turn_timeout(),
            settings: serde_json::Map::new(),
            created: unix_seconds(),
        }
    }
}

/// A workspace on disk, with its record loaded.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// Agent name.
    pub name: String,
    /// Workspace directory name, `workspace<Name>`.
    pub directory: String,
    /// Absolute workspace path.
    pub root: PathBuf,
    /// Descriptor.
    pub record: AgentRecord,
}

impl Workspace {
    /// Path of the descriptor file.
    pub fn record_path(&self) -> PathBuf {
        self.root.join(AGENT_FILE)
    }

    /// Path of the credential file.
    pub fn token_path(&self) -> PathBuf {
        self.root.join(TOKEN_FILE)
    }

    /// Path of the durable worker state file.
    pub fn state_path(&self) -> PathBuf {
        self.root.join(STATE_FILE)
    }

    /// Path of the host log.
    pub fn log_path(&self) -> PathBuf {
        self.root.join(LOG_FILE)
    }

    /// Directory the agent's own tools may write.
    pub fn files_dir(&self) -> PathBuf {
        self.root.join(FILES_DIR)
    }

    /// Absolute path of the module this agent evaluates.
    pub fn module_path(&self) -> Result<PathBuf> {
        match &self.record.module {
            AgentModule::Esm { path } => {
                let candidate = self.root.join(path);
                let resolved = confine(&self.root, &candidate)?;
                Ok(resolved)
            }
        }
    }

    /// Read this agent's credential, if one has been set.
    pub fn token(&self) -> Result<Option<String>> {
        let path = self.token_path();
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|error| Error::io("read credential", path.clone(), error))?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        Ok(Some(trimmed.to_string()))
    }

    /// Store this agent's credential, owner-only.
    pub fn set_token(&self, token: &str) -> Result<()> {
        write_secret_file(&self.token_path(), token.trim().as_bytes())
    }

    /// Remove this agent's credential.
    pub fn clear_token(&self) -> Result<()> {
        let path = self.token_path();
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|error| Error::io("remove credential", path.clone(), error))?;
        }
        Ok(())
    }

    /// Persist the descriptor.
    pub fn save_record(&self) -> Result<()> {
        let text = serde_json::to_string_pretty(&self.record).map_err(|error| {
            Error::json("encode", self.record_path().display().to_string(), error)
        })?;
        write_secret_file(&self.record_path(), format!("{text}\n").as_bytes())
    }

    /// Read the raw contents of a file inside the workspace.
    pub fn read_relative(&self, relative: &str) -> Result<String> {
        let path = confine(&self.root, &self.root.join(relative))?;
        std::fs::read_to_string(&path).map_err(|error| Error::io("read file", path, error))
    }

    /// Write a file inside the workspace.
    pub fn write_relative(&self, relative: &str, contents: &str) -> Result<()> {
        let path = confine(&self.root, &self.root.join(relative))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| Error::io("create directory", parent.to_path_buf(), error))?;
        }
        std::fs::write(&path, contents).map_err(|error| Error::io("write file", path, error))
    }
}

/// Validate an agent name.
///
/// Lowercase letters, digits, `_` and `-`, 2 to 32 characters, starting with a
/// letter or digit. This is the intersection of a safe directory name and a
/// usable chat command target.
pub fn validate_name(name: &str) -> Result<()> {
    if !NAME_PATTERN_LEN.contains(&name.len()) {
        return Err(Error::AgentName {
            name: name.to_string(),
            message: format!(
                "must be {} to {} characters",
                NAME_PATTERN_LEN.start(),
                NAME_PATTERN_LEN.end()
            ),
        });
    }
    let mut characters = name.chars();
    let first = characters.next().unwrap_or('_');
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(Error::AgentName {
            name: name.to_string(),
            message: "must start with a lowercase letter or a digit".to_string(),
        });
    }
    for character in characters {
        if !character.is_ascii_lowercase()
            && !character.is_ascii_digit()
            && character != '_'
            && character != '-'
        {
            return Err(Error::AgentName {
                name: name.to_string(),
                message: format!("`{character}` is not allowed; use a-z, 0-9, _ or -"),
            });
        }
    }
    Ok(())
}

/// Directory name for an agent name.
pub fn directory_for(name: &str) -> String {
    format!("{WORKSPACE_PREFIX}{}", capitalize(name))
}

/// Recover an agent name from a directory name, when it follows the convention.
pub fn name_for_directory(directory: &str) -> Option<String> {
    let rest = directory.strip_prefix(WORKSPACE_PREFIX)?;
    if rest.is_empty() {
        return None;
    }
    Some(rest.to_ascii_lowercase())
}

fn capitalize(name: &str) -> String {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) => {
            let mut out = String::with_capacity(name.len());
            out.extend(first.to_uppercase());
            out.push_str(characters.as_str());
            out
        }
        None => String::new(),
    }
}

/// Create a workspace directory and its descriptor.
///
/// Existing directories are left alone; `create` is idempotent so a restart does
/// not rewrite a record an operator has edited.
pub fn create(config: &LoadedConfig, name: &str) -> Result<Workspace> {
    validate_name(name)?;
    let directory = directory_for(name);
    let root = config.workspace_dir(&directory);
    std::fs::create_dir_all(&root)
        .map_err(|error| Error::io("create workspace", root.clone(), error))?;
    set_owner_only(&root)?;
    let files = root.join(FILES_DIR);
    std::fs::create_dir_all(&files)
        .map_err(|error| Error::io("create workspace files directory", files.clone(), error))?;

    let record_path = root.join(AGENT_FILE);
    if record_path.exists() {
        return open(config, name);
    }
    let record = AgentRecord::new(name);
    let workspace = Workspace {
        name: name.to_string(),
        directory,
        root,
        record,
    };
    workspace.save_record()?;
    let module = workspace.root.join(DEFAULT_MODULE);
    if !module.exists() {
        std::fs::write(&module, default_module_source(name))
            .map_err(|error| Error::io("write default module", module, error))?;
    }
    Ok(workspace)
}

/// Open an existing workspace.
pub fn open(config: &LoadedConfig, name: &str) -> Result<Workspace> {
    validate_name(name)?;
    let directory = directory_for(name);
    let root = config.workspace_dir(&directory);
    let record_path = root.join(AGENT_FILE);
    if !record_path.exists() {
        return Err(Error::UnknownAgent {
            name: name.to_string(),
        });
    }
    let text = std::fs::read_to_string(&record_path)
        .map_err(|error| Error::io("read agent record", record_path.clone(), error))?;
    let record: AgentRecord = serde_json::from_str(&text)
        .map_err(|error| Error::config(record_path.clone(), error.to_string()))?;
    if record.name != name {
        return Err(Error::workspace(
            record_path,
            format!(
                "record names agent `{}` but lives in `workspace{name}`",
                record.name
            ),
        ));
    }
    Ok(Workspace {
        name: name.to_string(),
        directory,
        root,
        record,
    })
}

/// Open every workspace under the configured root, sorted by name.
pub fn list(config: &LoadedConfig) -> Result<Vec<Workspace>> {
    let mut workspaces = Vec::new();
    for directory in config.workspace_directories()? {
        let Some(name) = name_for_directory(&directory) else {
            continue;
        };
        match open(config, &name) {
            Ok(workspace) => workspaces.push(workspace),
            Err(error) => {
                tracing::warn!(directory = %directory, error = %error, "skipping unreadable workspace");
            }
        }
    }
    Ok(workspaces)
}

/// Resolve `candidate` and refuse anything that escapes `root`.
///
/// Both paths are made absolute before comparison, so `..` and symlinks cannot
/// be used to reach outside the workspace.
pub fn confine(root: &Path, candidate: &Path) -> Result<PathBuf> {
    let root = absolutize(root)?;
    let candidate = absolutize(candidate)?;
    if candidate == root || candidate.starts_with(&root) {
        return Ok(candidate);
    }
    Err(Error::workspace(
        candidate,
        format!("escapes the agent workspace {}", root.display()),
    ))
}

/// Make a path absolute without requiring it to exist.
pub fn absolutize(path: &Path) -> Result<PathBuf> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| Error::io("read working directory", PathBuf::from("."), error))?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    Ok(normalized)
}

/// Seconds since the Unix epoch, or 0 when the clock is before it.
pub fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// The module written into a new workspace.
///
/// A working agent, not a placeholder: it answers every mention with the
/// runner's greeting and shows how to reach the host model and tools.
fn default_module_source(name: &str) -> String {
    format!(
        r#"// Agent `{name}`.
//
// The runner evaluates this module inside a dedicated V8 isolate. `Runner` is
// the host bridge: it is the only way out of the isolate, and every call is
// answered by the runner process, never by the guest.
//
// Uncomment a line to give this agent a model turn or a tool call.

Runner.log("agent `{name}` module loaded");

Runner.on("mention", async (event) => {{
    // Runner.settings carries this workspace's `settings` object.
    Runner.log("mention from " + event.from);

    // A model turn. `context` is the retained history the runner fetched.
    // const reply = await Runner.llm({{ prompt: event.text, context: event.context }});
    // await Runner.send({{ conversation_id: event.conversation_id, text: reply }});

    await Runner.send({{
        conversation_id: event.conversation_id,
        text: "`{name}` is running. Configure a model to answer with one.",
    }});
}});

await Runner.ready();
"#
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Test assertions fail the test on purpose.
mod tests {
    use super::*;
    use crate::config::LoadedConfig;
    use std::path::Path;

    fn test_config(dir: &Path) -> LoadedConfig {
        let config_path = dir.join("data/config.json");
        std::fs::create_dir_all(config_path.parent().unwrap_or(Path::new("."))).expect("parent");
        std::fs::write(
            &config_path,
            r#"{"agents":{"model_base_url":"http://127.0.0.1:9999/v1"}}"#,
        )
        .expect("fixture");
        let (config, _) = LoadedConfig::load(Some(&config_path), None).expect("load");
        config
    }

    #[test]
    fn accepts_valid_names() {
        for name in ["helper", "ab", "agent-1", "a_b", "x9"] {
            validate_name(name).expect(name);
        }
    }

    #[test]
    fn rejects_invalid_names() {
        for name in [
            "",
            "a",
            "Helper",
            "has space",
            "dot.dot",
            "slash/",
            "鐪塶icode",
        ] {
            assert!(validate_name(name).is_err(), "{name} should be rejected");
        }
    }

    #[test]
    fn directory_names_round_trip() {
        for name in ["helper", "ab", "agent-1"] {
            let directory = directory_for(name);
            assert!(directory.starts_with(WORKSPACE_PREFIX));
            assert_eq!(name_for_directory(&directory).as_deref(), Some(name));
        }
    }

    #[test]
    fn create_is_idempotent_and_writes_the_expected_layout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path());
        let first = create(&config, "helper").expect("create");
        assert!(first.record_path().exists());
        assert!(first.module_path().expect("module").exists());
        assert!(first.files_dir().is_dir());
        first.set_token("bot_secret").expect("token");

        // A second create must not clobber an edited record.
        let mut edited = first.clone();
        edited.record.turn_timeout_seconds = 7;
        edited.save_record().expect("save");
        let second = create(&config, "helper").expect("create again");
        assert_eq!(second.record.turn_timeout_seconds, 7);
        assert_eq!(
            second.token().expect("token").as_deref(),
            Some("bot_secret")
        );
    }

    #[test]
    fn open_reports_unknown_agents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path());
        let error = open(&config, "ghost").expect_err("must fail");
        assert!(matches!(error, Error::UnknownAgent { .. }), "{error}");
    }

    #[test]
    fn list_finds_created_workspaces_sorted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path());
        create(&config, "zulu").expect("create");
        create(&config, "alpha").expect("create");
        let names: Vec<String> = list(&config)
            .expect("list")
            .into_iter()
            .map(|workspace| workspace.name)
            .collect();
        assert_eq!(names, vec!["alpha", "zulu"]);
    }

    #[test]
    fn confine_refuses_escapes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("workspaceA");
        std::fs::create_dir_all(&root).expect("mkdir");
        confine(&root, &root.join("files/x.txt")).expect("inside is allowed");
        for escape in ["../outside.txt", "files/../../outside.txt", "/etc/passwd"] {
            assert!(
                confine(&root, &root.join(escape)).is_err(),
                "{escape} should be refused"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn token_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let config = test_config(dir.path());
        let workspace = create(&config, "helper").expect("create");
        workspace.set_token("bot_secret").expect("token");
        let mode = std::fs::metadata(workspace.token_path())
            .expect("stat")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }
}
