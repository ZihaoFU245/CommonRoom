//! Transient remote configuration control; the external runtime owns configuration.
use crate::web::{App, auth};
use axum::http::HeaderMap;
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};

const USAGE: &str = "Usage: /bot config set name key value | /bot config fetch name [key]";
#[derive(Clone, Debug)]
pub(crate) struct ConfigCommand {
    pub name: String,
    pub operation: &'static str,
    pub key: Option<String>,
    pub value: Option<String>,
}
pub(crate) fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}
impl ConfigCommand {
    pub fn parse(input: &str) -> Result<Option<Self>, String> {
        let mut rest = input.trim();
        let mut next = || {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let word = &rest[..end];
            rest = rest[end..].trim_start();
            word
        };
        if next() != "/bot" || next() != "config" {
            return Ok(None);
        }
        let operation = next();
        let name = next().to_owned();
        let key = next().to_owned();
        let value = rest.trim_end();
        if name.is_empty() {
            return Err(USAGE.into());
        }
        let (operation, key, value) = match operation {
            "fetch" if value.is_empty() => ("fetch", (!key.is_empty()).then_some(key), None),
            "set" if !key.is_empty() && !value.is_empty() => {
                let value: String = if value.starts_with('"') {
                    serde_json::from_str(value).map_err(|_| "Use a valid JSON-quoted string or unquoted remaining text for the value.")?
                } else {
                    value.to_owned()
                };
                if value.len() > 4096 {
                    return Err("Config values support at most 4096 bytes.".into());
                }
                ("set", Some(key), Some(value))
            }
            _ => return Err(USAGE.into()),
        };
        if key.as_deref().is_some_and(|k| !valid_key(k)) {
            return Err(
                "Config keys support 1–64 ASCII letters, digits, dots, underscores or hyphens."
                    .into(),
            );
        }
        Ok(Some(Self {
            name,
            operation,
            key,
            value,
        }))
    }
    pub fn frame(&self, id: &str) -> serde_json::Value {
        json!({"kind":"config_request","id":id,"operation":self.operation,"key":self.key,"value":self.value})
    }
}

#[derive(Clone)]
pub(crate) struct Authority {
    pub actor: Option<String>,
    pub headers: HeaderMap,
    pub view: Option<String>,
    pub sudo: bool,
}
impl Authority {
    pub fn authorize(&self, app: &App, command: &ConfigCommand) -> Result<String, String> {
        if app.stopping.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("Server stopping.".into());
        }
        let mut engine = app.engine()?;
        if let Some(actor) = &self.actor
            && auth::identity(&engine, &self.headers).as_ref() != Some(actor)
        {
            return Err("Please log in.".into());
        }
        engine.with_command_authority(
            self.actor.as_deref(),
            self.view.as_deref(),
            self.sudo,
            |engine| {
                engine.authorize_bot_config(
                    self.actor.as_deref(),
                    self.view.as_deref(),
                    &command.name,
                    command.operation == "set",
                )
            },
        )
    }
}

pub(crate) struct Request {
    pub id: String,
    pub target: String,
    pub command: ConfigCommand,
    pub authority: Authority,
    pub reply: oneshot::Sender<Result<BTreeMap<String, String>, String>>,
}
struct Connection {
    id: String,
    sender: mpsc::Sender<Request>,
}
#[derive(Clone, Default)]
pub(crate) struct Hub(Arc<Mutex<BTreeMap<String, Connection>>>);
pub(crate) struct Lease {
    hub: Hub,
    target: String,
    id: String,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut connections) = self.hub.0.lock()
            && connections
                .get(&self.target)
                .is_some_and(|c| c.id == self.id)
        {
            connections.remove(&self.target);
        }
    }
}
impl Hub {
    pub fn register(&self, target: String, sender: mpsc::Sender<Request>) -> Result<Lease, String> {
        let mut connections = self.0.lock().map_err(|_| "Runtime registry unavailable.")?;
        if connections.contains_key(&target) {
            return Err("A configuration runtime is already connected for this bot.".into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        connections.insert(
            target.clone(),
            Connection {
                id: id.clone(),
                sender,
            },
        );
        Ok(Lease {
            hub: self.clone(),
            target,
            id,
        })
    }
    pub async fn execute(
        &self,
        app: &App,
        authority: Authority,
        command: ConfigCommand,
    ) -> Result<String, String> {
        let target = authority.authorize(app, &command)?;
        let sender = self
            .0
            .lock()
            .map_err(|_| "Runtime registry unavailable.")?
            .get(&target)
            .ok_or("Bot configuration runtime is offline.")?
            .sender
            .clone();
        let (reply, receiver) = oneshot::channel();
        sender
            .try_send(Request {
                id: uuid::Uuid::new_v4().to_string(),
                target: target.clone(),
                command: command.clone(),
                authority: authority.clone(),
                reply,
            })
            .map_err(|_| "Bot configuration runtime busy or disconnected.")?;
        let result = tokio::time::timeout(Duration::from_secs(10), receiver).await
            .map_err(|_| "Bot configuration request timed out; a set may have applied. Fetch before retrying.")?
            .map_err(|_| "Bot configuration runtime disconnected.")?;
        if authority.authorize(app, &command)? != target {
            return Err("Bot account changed.".into());
        }
        let values = result?;
        serde_json::to_string_pretty(
            &json!({"bot":command.name,"operation":command.operation,"values":values}),
        )
        .map_err(|e| e.to_string())
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub(crate) enum Control {
    #[serde(rename = "config_ready")]
    Ready,
    #[serde(rename = "config_response")]
    Response {
        id: String,
        values: Option<BTreeMap<String, String>>,
        error: Option<String>,
    },
}
pub(crate) fn validate_response(
    values: Option<BTreeMap<String, String>>,
    error: Option<String>,
) -> Result<BTreeMap<String, String>, String> {
    match (values, error) {
        (Some(values), None)
            if values.len() <= 128
                && values.iter().all(|(k, v)| valid_key(k) && v.len() <= 4096)
                && serde_json::to_vec(&values).is_ok_and(|v| v.len() <= 32768) =>
        {
            Ok(values)
        }
        (None, Some(error)) if !error.is_empty() && error.len() <= 1024 => {
            Err(format!("External runtime: {error}"))
        }
        _ => Err("Invalid external runtime configuration response.".into()),
    }
}
