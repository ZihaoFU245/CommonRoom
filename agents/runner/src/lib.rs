//! CommonRoom agent runner library.
//!
//! The binary is a thin CLI over this crate. Keeping the platform logic in a
//! library is what makes it testable without spawning a process: configuration,
//! workspaces and the isolate pool are all reachable from integration tests.
//!
//! One process, one V8 isolate per agent. The runner is the only component that
//! holds a model key or a chat credential; an agent's isolate reaches both
//! through host calls it cannot forge.

#![forbid(unsafe_code)]

pub mod config;
pub mod errors;
pub mod workspace;

pub use errors::{Error, Result};
