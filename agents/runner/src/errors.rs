//! The runner's error surface.
//!
//! One enum, `#[non_exhaustive]` at the crate boundary, so an operator sees a
//! sentence they can act on rather than a wrapped `Box<dyn Error>`. The
//! workspace forbids `unwrap`, `expect` and `panic`, so every fallible step in
//! this crate returns one of these.

use std::path::PathBuf;

/// Result alias for the whole crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong while loading configuration, provisioning an
/// agent workspace, or driving an isolate.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The configuration file exists but is not valid JSON, or holds a field
    /// with the wrong type.
    #[error("invalid configuration at {path}: {message}")]
    Config {
        /// File that failed to load.
        path: PathBuf,
        /// Why it failed.
        message: String,
    },

    /// A configuration value is present and well-typed but unusable, such as a
    /// concurrency ceiling of zero or an empty chat URL.
    #[error("invalid configuration value `{field}`: {message}")]
    ConfigValue {
        /// Dotted field name, for example `agents.chat.url`.
        field: String,
        /// Why the value is rejected.
        message: String,
    },

    /// An agent name is not usable as a directory name or as a chat command
    /// target.
    #[error("invalid agent name `{name}`: {message}")]
    AgentName {
        /// The rejected name.
        name: String,
        /// The rule it broke.
        message: String,
    },

    /// No agent exists under the requested name.
    #[error("no agent named `{name}`")]
    UnknownAgent {
        /// The requested name.
        name: String,
    },

    /// An agent already exists under the requested name.
    #[error("agent `{name}` already exists")]
    DuplicateAgent {
        /// The requested name.
        name: String,
    },

    /// The filesystem layout of a workspace is wrong: a missing manifest, a
    /// missing entry module, or a path that escapes the workspace.
    #[error("workspace problem at {path}: {message}")]
    Workspace {
        /// The offending path.
        path: PathBuf,
        /// What is wrong with it.
        message: String,
    },

    /// Reading, writing or creating a file failed.
    #[error("{operation} failed at {path}: {source}")]
    Io {
        /// Verb describing the attempted operation, for example `create`.
        operation: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The underlying failure.
        #[source]
        source: std::io::Error,
    },

    /// The isolated runtime rejected a script, a module or a host call.
    #[error("isolate `{agent}` failed: {message}")]
    Isolate {
        /// Agent whose isolate failed.
        agent: String,
        /// The V8 or module-loader message.
        message: String,
    },

    /// The agent's isolate ended while the runner still expected work from it.
    #[error("isolate `{agent}` stopped unexpectedly{detail}")]
    IsolateStopped {
        /// Agent whose isolate stopped.
        agent: String,
        /// Optional trailing detail, already prefixed with a colon and space.
        detail: String,
    },

    /// A transport-level failure talking to the chat server.
    #[error("chat request {method} {url} failed: {message}")]
    Chat {
        /// HTTP method.
        method: &'static str,
        /// Absolute URL.
        url: String,
        /// Transport or decoding failure.
        message: String,
    },

    /// The chat server answered with an error status or error code.
    #[error("chat server rejected {method} {url}: {status} {code}{detail}")]
    ChatStatus {
        /// HTTP method.
        method: &'static str,
        /// Absolute URL.
        url: String,
        /// HTTP status.
        status: u16,
        /// Machine-readable error code when the body carried one.
        code: String,
        /// Optional trailing detail, already prefixed with a colon and space.
        detail: String,
    },

    /// JSON encoding or decoding failed at a named boundary.
    #[error("cannot {operation} JSON for {context}: {source}")]
    Json {
        /// `encode` or `decode`.
        operation: &'static str,
        /// Where the payload came from or goes.
        context: String,
        /// The underlying failure.
        #[source]
        source: serde_json::Error,
    },
}

impl Error {
    /// Attach an I/O failure to a path with a verb.
    pub fn io(operation: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            operation,
            path: path.into(),
            source,
        }
    }

    /// Build a workspace error.
    pub fn workspace(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::Workspace {
            path: path.into(),
            message: message.into(),
        }
    }

    /// Build a configuration error.
    pub fn config(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::Config {
            path: path.into(),
            message: message.into(),
        }
    }

    /// Build a configuration-value error.
    pub fn config_value(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ConfigValue {
            field: field.into(),
            message: message.into(),
        }
    }

    /// Build an isolate error.
    pub fn isolate(agent: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Isolate {
            agent: agent.into(),
            message: message.into(),
        }
    }

    /// Build a JSON error.
    pub fn json(
        operation: &'static str,
        context: impl Into<String>,
        source: serde_json::Error,
    ) -> Self {
        Self::Json {
            operation,
            context: context.into(),
            source,
        }
    }
}

/// Helper for the common `serde_json` encode step.
pub fn encode<T: serde::Serialize>(context: &str, value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|error| Error::json("encode", context, error))
}

/// Helper for the common `serde_json` decode step.
pub fn decode<T: serde::de::DeserializeOwned>(context: &str, input: &str) -> Result<T> {
    serde_json::from_str(input).map_err(|error| Error::json("decode", context, error))
}
