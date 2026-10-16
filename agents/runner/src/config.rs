//! Runner configuration.
//!
//! The runner's settings live in one JSON file, `data/config.json` by default,
//! created with default values the first time the runner starts. The file is
//! shared with the chat server, so the runner owns a single namespaced key,
//! `agents`, and leaves every other key byte-for-byte untouched: reading a
//! server configuration and writing it back never drops `bind`, `origins` or
//! anything else the server put there.
//!
//! Precedence for the data directory, highest first: the `--data` flag, the
//! `AGENTS_DATA` environment variable, the parent directory of an explicitly
//! selected config file, then `data/` relative to the working directory.
//! `AGENTS_CONFIG` and `--config` select the file itself.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::errors::{Error, Result};

/// Namespaced key the runner owns inside the shared configuration file.
pub const CONFIG_NAMESPACE: &str = "agents";

/// Default config file path, relative to the working directory.
pub const DEFAULT_CONFIG_FILE: &str = "data/config.json";

/// Default data directory, relative to the working directory.
pub const DEFAULT_DATA_DIR: &str = "data";

/// Default chat base URL. Matches the chat server's own default bind address.
pub const DEFAULT_CHAT_URL: &str = "http://127.0.0.1:3000/";

/// Log levels accepted from configuration or `AGENTS_LOG`.
pub const LOG_LEVELS: [&str; 5] = ["trace", "debug", "info", "warn", "error"];

/// Everything the runner needs to serve its agents.
///
/// Nested rather than flat so the file stays readable next to the server's own
/// keys, and so a later version can add a sibling namespace without a rewrite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerConfig {
    /// Chat server base URL, including any hosting base path, without the
    /// `/api/bot/v1` suffix.
    #[serde(default = "default_chat_url")]
    pub chat_url: String,

    /// Environment variable holding the runner bot's provisioning credential.
    #[serde(default = "default_bot_token_env")]
    pub bot_token_env: String,

    /// Environment variable holding the operator credential used to mint
    /// account-wide worker credentials for new agents via
    /// `POST /api/bot/v1/credentials`.
    #[serde(default = "default_admin_token_env")]
    pub admin_token_env: String,

    /// Model provider base URL, OpenAI-compatible, used by the host LLM op.
    #[serde(default)]
    pub model_base_url: String,

    /// Model id sent with every completion request.
    #[serde(default = "default_model_name")]
    pub model_name: String,

    /// Environment variable holding the model provider key. The key is read by
    /// the runner and is never serialized into a workspace or handed to guest
    /// code as text.
    #[serde(default = "default_model_key_env")]
    pub model_key_env: String,

    /// Directory holding one workspace per agent.
    #[serde(default = "default_workspace_root")]
    pub workspace_root: String,

    /// Maximum number of agents the runner drives at once.
    #[serde(default = "default_max_agents")]
    pub max_agents: usize,

    /// Operator usernames allowed to use the runner's control commands.
    #[serde(default)]
    pub operators: Vec<String>,

    /// Also accept a requester the chat server reports as a member of its admin
    /// group, resolved through `/message-context`.
    #[serde(default = "default_true")]
    pub trust_server_admin_group: bool,

    /// Chat accounts the runner answers without a mention, for its own control
    /// surface.
    #[serde(default)]
    pub always_answer: Vec<String>,

    /// Tracing level when `AGENTS_LOG` and `RUST_LOG` are both unset.
    #[serde(default = "default_log_level")]
    pub log_level: String,

    /// Whether the runner prints and answers its stdin console.
    #[serde(default = "default_true")]
    pub console: bool,

    /// Whether the runner connects to the chat server at all.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            chat_url: default_chat_url(),
            bot_token_env: default_bot_token_env(),
            admin_token_env: default_admin_token_env(),
            model_base_url: String::new(),
            model_name: default_model_name(),
            model_key_env: default_model_key_env(),
            workspace_root: default_workspace_root(),
            max_agents: default_max_agents(),
            operators: Vec::new(),
            trust_server_admin_group: true,
            always_answer: Vec::new(),
            log_level: default_log_level(),
            console: true,
            enabled: true,
        }
    }
}

fn default_chat_url() -> String {
    DEFAULT_CHAT_URL.to_string()
}

fn default_bot_token_env() -> String {
    "RUNNER_BOT_TOKEN".to_string()
}

fn default_admin_token_env() -> String {
    "RUNNER_ADMIN_TOKEN".to_string()
}

fn default_model_name() -> String {
    "deepseek-flash".to_string()
}

fn default_model_key_env() -> String {
    "DEEPSEEK_API_KEY".to_string()
}

fn default_workspace_root() -> String {
    "workspaces".to_string()
}

fn default_max_agents() -> usize {
    32
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_true() -> bool {
    true
}

/// A loaded configuration plus the paths it was resolved against.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    /// Absolute path of the configuration file.
    pub config_path: PathBuf,
    /// Absolute data directory: the parent of the config file unless overridden.
    pub data_dir: PathBuf,
    /// Absolute directory holding one workspace per agent.
    pub workspace_root: PathBuf,
    /// Validated runner settings.
    pub config: RunnerConfig,
}

/// The JSON document as it sits on disk.
///
/// Deliberately tolerant of unknown top-level keys so a shared `data/config.json`
/// written by the chat server loads unchanged. The `agents` object itself is
/// strict, so a typo inside the runner's own settings is reported instead of
/// silently ignored.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConfigDocument {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agents: Option<RunnerConfig>,
}

/// Resolve where the configuration file and data directory live.
///
/// `explicit_config` is the `--config` value, if any. `explicit_data` is the
/// `--data` value, if any. Environment variables are consulted in between, as
/// documented on the module.
pub fn resolve_paths(
    explicit_config: Option<&Path>,
    explicit_data: Option<&Path>,
) -> (PathBuf, PathBuf) {
    let config_path = explicit_config
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("AGENTS_CONFIG").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_FILE));

    let data_dir = explicit_data
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("AGENTS_DATA").map(PathBuf::from))
        .or_else(|| {
            config_path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(Path::to_path_buf)
        })
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DATA_DIR));

    (config_path, data_dir)
}

impl LoadedConfig {
    /// Load configuration from disk, creating the file with defaults when it is
    /// absent and filling in any missing key when it exists.
    ///
    /// Returns the loaded configuration together with a note describing what was
    /// written, so the caller can log it without inspecting the filesystem.
    pub fn load(
        explicit_config: Option<&Path>,
        explicit_data: Option<&Path>,
    ) -> Result<(Self, LoadNote)> {
        let (config_path, data_dir) = resolve_paths(explicit_config, explicit_data);

        if let Some(parent) = config_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|error| {
                Error::io(
                    "create configuration directory",
                    parent.to_path_buf(),
                    error,
                )
            })?;
        }

        let note = if config_path.exists() {
            let text = std::fs::read_to_string(&config_path)
                .map_err(|error| Error::io("read configuration", config_path.clone(), error))?;
            let document: ConfigDocument = serde_json::from_str(&text)
                .map_err(|error| Error::config(config_path.clone(), error.to_string()))?;
            match document.agents {
                Some(config) => {
                    config.shape(&config_path)?;
                    LoadNote::Existing
                }
                None => {
                    let config = RunnerConfig::default();
                    config.shape(&config_path)?;
                    write_namespace(&config_path, &text, &config)?;
                    LoadNote::AddedNamespace
                }
            }
        } else {
            let config = RunnerConfig::default();
            config.shape(&config_path)?;
            let document = ConfigDocument {
                agents: Some(config.clone()),
            };
            let text = serde_json::to_string_pretty(&document)
                .map_err(|error| Error::json("encode", config_path.display().to_string(), error))?;
            // Owner-only from the first write: the agent block will hold
            // operator identity and, through the file's neighbours, is the
            // natural place credentials end up.
            write_secret_file(&config_path, format!("{text}\n").as_bytes())?;
            LoadNote::Created
        };

        let config = read_namespace(&config_path)?.unwrap_or_default();

        let workspace_root = {
            let candidate = PathBuf::from(&config.workspace_root);
            if candidate.is_absolute() {
                candidate
            } else {
                data_dir.join(candidate)
            }
        };

        Ok((
            Self {
                config_path,
                data_dir,
                workspace_root,
                config,
            },
            note,
        ))
    }

    /// Load without touching the filesystem beyond reading, for tests and for
    /// the `config` subcommand.
    pub fn load_read_only(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| Error::io("read configuration", path.to_path_buf(), error))?;
        let document: ConfigDocument = serde_json::from_str(&text)
            .map_err(|error| Error::config(path.to_path_buf(), error.to_string()))?;
        let config = document.agents.unwrap_or_default();
        config.shape(path)?;
        let data_dir = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_DATA_DIR));
        let root = PathBuf::from(&config.workspace_root);
        Ok(Self {
            config_path: path.to_path_buf(),
            data_dir: data_dir.clone(),
            workspace_root: if root.is_absolute() {
                root
            } else {
                data_dir.join(root)
            },
            config,
        })
    }

    /// Absolute path of one agent's workspace directory.
    ///
    /// This is the `data/workspace<Name>` layout: the workspace root comes from
    /// configuration and each agent appends its own directory name.
    pub fn workspace_dir(&self, directory: &str) -> PathBuf {
        self.workspace_root.join(directory)
    }

    /// Names of every workspace directory present on disk, sorted.
    pub fn workspace_directories(&self) -> Result<Vec<String>> {
        let root = &self.workspace_root;
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut names = Vec::new();
        let entries = std::fs::read_dir(root)
            .map_err(|error| Error::io("list workspace root", root.clone(), error))?;
        for entry in entries {
            let entry =
                entry.map_err(|error| Error::io("read workspace entry", root.clone(), error))?;
            let file_type = entry
                .file_type()
                .map_err(|error| Error::io("stat workspace entry", entry.path(), error))?;
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            names.push(name);
        }
        names.sort();
        Ok(names)
    }
}

/// What loading did, so the caller can report it once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadNote {
    /// The file was absent and has been created with default values.
    Created,
    /// The file existed and the `agents` namespace was appended.
    AddedNamespace,
    /// The file existed and already carried the namespace.
    Existing,
}

impl LoadNote {
    /// A sentence for the log.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Created => "created data/config.json with default agent settings",
            Self::AddedNamespace => "added the `agents` settings block to data/config.json",
            Self::Existing => "loaded agent settings from data/config.json",
        }
    }
}

fn read_namespace(path: &Path) -> Result<Option<RunnerConfig>> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| Error::io("read configuration", path.to_path_buf(), error))?;
    let document: ConfigDocument = serde_json::from_str(&text)
        .map_err(|error| Error::config(path.to_path_buf(), error.to_string()))?;
    Ok(document.agents)
}

/// Rewrite only the `agents` key of an existing document.
///
/// The file is parsed as a generic JSON value so every unrelated key survives
/// exactly as the server wrote it.
fn write_namespace(path: &Path, existing_text: &str, config: &RunnerConfig) -> Result<()> {
    let mut value: serde_json::Value = serde_json::from_str(existing_text)
        .map_err(|error| Error::config(path.to_path_buf(), error.to_string()))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| Error::config(path.to_path_buf(), "top level must be a JSON object"))?;
    let encoded = serde_json::to_value(config)
        .map_err(|error| Error::json("encode", path.display().to_string(), error))?;
    object.insert(CONFIG_NAMESPACE.to_string(), encoded);
    let text = serde_json::to_string_pretty(&value)
        .map_err(|error| Error::json("encode", path.display().to_string(), error))?;
    write_secret_file(path, format!("{text}\n").as_bytes())
}

/// Write a file with owner-only permissions.
///
/// Used for anything that can hold a credential: configuration, agent records,
/// checkpoints and logs.
pub fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| Error::io("create directory", parent.to_path_buf(), error))?;
    }
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes)
        .map_err(|error| Error::io("write temporary file", temporary.clone(), error))?;
    set_owner_only(&temporary)?;
    std::fs::rename(&temporary, path)
        .map_err(|error| Error::io("replace file", path.to_path_buf(), error))?;
    Ok(())
}

/// Restrict a path to the owning user, mode `0600` for files and `0700` for
/// directories.
pub fn set_owner_only(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(path)
            .map_err(|error| Error::io("stat", path.to_path_buf(), error))?;
        let mode = if metadata.is_dir() { 0o700 } else { 0o600 };
        let permissions = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(path, permissions)
            .map_err(|error| Error::io("restrict permissions", path.to_path_buf(), error))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

impl RunnerConfig {
    /// Checks that hold whenever the file is read.
    ///
    /// A default-constructed configuration must pass this: the runner's first
    /// start legitimately has no model provider configured yet, exactly as the
    /// reference worker starts before an operator sets one.
    pub fn shape(&self, path: &Path) -> Result<()> {
        if self.chat_url.trim().is_empty() {
            return Err(Error::config_value("chat_url", "must not be empty"));
        }
        if self.bot_token_env.trim().is_empty() {
            return Err(Error::config_value(
                "bot_token_env",
                "must name an environment variable",
            ));
        }
        if self.admin_token_env.trim().is_empty() {
            return Err(Error::config_value(
                "admin_token_env",
                "must name an environment variable",
            ));
        }
        if self.model_name.trim().is_empty() {
            return Err(Error::config_value("model_name", "must not be empty"));
        }
        if self.model_key_env.trim().is_empty() {
            return Err(Error::config_value(
                "model_key_env",
                "must name an environment variable",
            ));
        }
        if !LOG_LEVELS.contains(&self.log_level.as_str()) {
            return Err(Error::config_value(
                "log_level",
                format!("must be one of {}", LOG_LEVELS.join(", ")),
            ));
        }
        if self.max_agents == 0 {
            return Err(Error::config_value(
                "max_agents",
                "must be at least 1; the runner cannot serve zero agents",
            ));
        }
        if self.max_agents > 1024 {
            return Err(Error::config_value(
                "max_agents",
                "must be at most 1024; each agent costs an isolate",
            ));
        }
        if self.workspace_root.trim().is_empty() {
            return Err(Error::config_value("workspace_root", "must not be empty"));
        }
        let _ = path;
        Ok(())
    }

    /// Checks that only `serve` needs.
    ///
    /// Kept separate from [`RunnerConfig::shape`] so `runner list` and
    /// `runner config` work on a half-configured installation: an operator
    /// inspecting why the runner will not start should not be blocked by the
    /// same missing setting.
    pub fn validate_serving(&self) -> Result<()> {
        if self.model_base_url.trim().is_empty() {
            return Err(Error::config_value(
                "model_base_url",
                "must be set before serving; point it at the OpenAI-compatible provider",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Test assertions fail the test on purpose.
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new("."))).expect("parent");
        std::fs::write(path, text).expect("write fixture");
    }

    #[test]
    fn creates_config_with_defaults_when_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("data/config.json");
        let (loaded, note) =
            LoadedConfig::load(Some(&config_path), None).expect("load creates the file");
        assert_eq!(note, LoadNote::Created);
        assert!(config_path.exists());
        assert_eq!(loaded.config.chat_url, DEFAULT_CHAT_URL);
        assert_eq!(loaded.config.max_agents, 32);
        assert_eq!(loaded.data_dir, dir.path().join("data"));
        assert_eq!(loaded.workspace_root, dir.path().join("data/workspaces"));
    }

    #[test]
    fn create_then_load_again_is_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("data/config.json");
        let (first, _) = LoadedConfig::load(Some(&config_path), None).expect("first load");
        let before = std::fs::read_to_string(&config_path).expect("read");
        let (second, note) = LoadedConfig::load(Some(&config_path), None).expect("second load");
        let after = std::fs::read_to_string(&config_path).expect("read");
        assert_eq!(note, LoadNote::Existing);
        assert_eq!(before, after, "a second load must not rewrite the file");
        assert_eq!(first.config, second.config);
    }

    #[test]
    fn preserves_server_keys_when_adding_the_namespace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("data/config.json");
        write(
            &config_path,
            r#"{"bind":"127.0.0.1:3000","origins":["http://localhost:5173"],"production":false}"#,
        );
        let (loaded, note) = LoadedConfig::load(Some(&config_path), None).expect("load");
        assert_eq!(note, LoadNote::AddedNamespace);
        let text = std::fs::read_to_string(&config_path).expect("read");
        let value: serde_json::Value = serde_json::from_str(&text).expect("parse");
        assert_eq!(value["bind"], "127.0.0.1:3000");
        assert_eq!(value["production"], false);
        assert!(value["agents"].is_object());
        assert_eq!(loaded.config.chat_url, DEFAULT_CHAT_URL);
    }

    #[test]
    fn rejects_a_typo_inside_the_namespace() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("data/config.json");
        write(&config_path, r#"{"agents":{"chat_urll":"http://x/"}}"#);
        let error = LoadedConfig::load(Some(&config_path), None).expect_err("must fail");
        let message = error.to_string();
        assert!(
            message.contains("chat_urll") || message.contains("unknown field"),
            "unexpected message: {message}"
        );
    }

    #[test]
    fn rejects_zero_max_agents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("data/config.json");
        write(&config_path, r#"{"agents":{"max_agents":0}}"#);
        let error = LoadedConfig::load(Some(&config_path), None).expect_err("must fail");
        assert!(error.to_string().contains("max_agents"), "{error}");
    }

    #[test]
    fn explicit_data_directory_wins_for_the_workspace_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("elsewhere");
        let config_path = dir.path().join("cfg/runner.json");
        let (loaded, _) = LoadedConfig::load(Some(&config_path), Some(&data)).expect("load");
        assert_eq!(loaded.data_dir, data);
        assert_eq!(loaded.workspace_root, data.join("workspaces"));
        assert_eq!(
            loaded.workspace_dir("workspaceA"),
            data.join("workspaces/workspaceA")
        );
    }

    #[test]
    fn lists_workspace_directories_and_skips_hidden_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("data/config.json");
        let (loaded, _) = LoadedConfig::load(Some(&config_path), None).expect("load");
        std::fs::create_dir_all(loaded.workspace_dir("workspaceB")).expect("mkdir");
        std::fs::create_dir_all(loaded.workspace_dir("workspaceA")).expect("mkdir");
        std::fs::create_dir_all(loaded.workspace_root.join(".hidden")).expect("mkdir");
        std::fs::write(loaded.workspace_root.join("stray.txt"), b"x").expect("write");
        let names = loaded.workspace_directories().expect("list");
        assert_eq!(names, vec!["workspaceA", "workspaceB"]);
    }

    #[cfg(unix)]
    #[test]
    fn configuration_is_written_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let config_path = dir.path().join("data/config.json");
        let _ = LoadedConfig::load(Some(&config_path), None).expect("load");
        let mode = std::fs::metadata(&config_path)
            .expect("stat")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "config holds no secrets but the agent block is private"
        );
    }
}
