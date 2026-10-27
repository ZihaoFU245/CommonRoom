//! Worker-owned command catalogues. Transient, bounded, and tied to a live credential.
use super::{
    auth::Context,
    error::{Code, Error},
};
use crate::{
    engine::{Engine, authorization::Action},
    web::{App, Change},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Command {
    pub name: String,
    pub usage: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subcommands: Vec<Command>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub operator_only: bool,
}
const REQUEST_TTL: Duration = Duration::from_secs(60);

fn is_false(value: &bool) -> bool {
    !value
}
fn visible_commands(commands: &[Command], operator: bool) -> Vec<Command> {
    commands
        .iter()
        .filter(|command| operator || !command.operator_only)
        .map(|command| {
            let mut command = command.clone();
            command.subcommands = visible_commands(&command.subcommands, operator);
            command
        })
        .collect()
}
pub(crate) fn validate(commands: &[Command]) -> Result<(), Error> {
    if serde_json::to_vec(commands).map_err(Error::storage)?.len() > 32768 {
        return Err(Error::new(
            Code::InvalidRequest,
            "Command catalogue exceeds its limit.",
        ));
    }
    validate_level(commands, "", 0, &mut 0)
}
fn validate_level(
    commands: &[Command],
    parent: &str,
    depth: usize,
    count: &mut usize,
) -> Result<(), Error> {
    let mut names = BTreeSet::new();
    for command in commands {
        *count += 1;
        let name = if depth == 0 {
            command.name.strip_prefix('/').unwrap_or_default()
        } else {
            &command.name
        };
        let path = if depth == 0 {
            command.name.clone()
        } else {
            format!("{parent} {}", command.name)
        };
        if name.is_empty()
            || depth >= 3
            || *count > 64
            || name.len() > 32
            || !name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-'))
            || !names.insert(&command.name)
            || command.usage.len() > 256
            || !(command.usage == path || command.usage.starts_with(&format!("{path} ")))
            || command.description.is_empty()
            || command.description.len() > 512
            || command
                .usage
                .chars()
                .chain(command.description.chars())
                .any(char::is_control)
        {
            return Err(Error::new(
                Code::InvalidRequest,
                "Invalid command name, usage or description.",
            ));
        }
        validate_level(&command.subcommands, &path, depth + 1, count)?;
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub(crate) enum Control {
    #[serde(rename = "commands_ready")]
    Ready,
    #[serde(rename = "commands_response")]
    Response { id: String, commands: Vec<Command> },
    #[serde(rename = "commands_changed")]
    Changed,
}
struct Publisher {
    session: String,
    context: Context,
    catalogues: BTreeMap<String, Vec<Command>>,
    bytes: usize,
}
#[derive(Clone, Default)]
pub(crate) struct Hub(Arc<Mutex<BTreeMap<String, Publisher>>>);
pub(crate) struct Lease {
    hub: Hub,
    account: String,
    session: String,
    changes: tokio::sync::broadcast::Sender<Change>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut publishers) = self.hub.0.lock()
            && publishers
                .get(&self.account)
                .is_some_and(|p| p.session == self.session)
            && let Some(publisher) = publishers.remove(&self.account)
        {
            for id in publisher.catalogues.keys() {
                let _ = self.changes.send(Change::BotCommands(id.clone()));
            }
        }
    }
}
impl Hub {
    fn register(
        &self,
        context: &Context,
        changes: &tokio::sync::broadcast::Sender<Change>,
    ) -> Result<Lease, Error> {
        let mut publishers = self.0.lock().map_err(Error::storage)?;
        if publishers.contains_key(&context.account) {
            return Err(Error::new(
                Code::RuntimeRejected,
                "A command runtime is already connected for this bot.",
            ));
        }
        let session = uuid::Uuid::new_v4().to_string();
        publishers.insert(
            context.account.clone(),
            Publisher {
                session: session.clone(),
                context: context.clone(),
                catalogues: BTreeMap::new(),
                bytes: 0,
            },
        );
        Ok(Lease {
            hub: self.clone(),
            account: context.account.clone(),
            session,
            changes: changes.clone(),
        })
    }
    fn clear(&self, lease: &Lease) -> Result<(), Error> {
        let mut publishers = self.0.lock().map_err(Error::storage)?;
        if let Some(publisher) = publishers
            .get_mut(&lease.account)
            .filter(|p| p.session == lease.session)
        {
            for id in publisher.catalogues.keys() {
                let _ = lease.changes.send(Change::BotCommands(id.clone()));
            }
            publisher.catalogues.clear();
            publisher.bytes = 0;
        }
        Ok(())
    }
    fn retain(&self, lease: &Lease, ids: &BTreeSet<String>) -> Result<(), Error> {
        let mut publishers = self.0.lock().map_err(Error::storage)?;
        if let Some(publisher) = publishers
            .get_mut(&lease.account)
            .filter(|p| p.session == lease.session)
        {
            publisher.catalogues.retain(|id, _| {
                if ids.contains(id) {
                    return true;
                }
                let _ = lease.changes.send(Change::BotCommands(id.clone()));
                false
            });
            publisher.bytes = publisher
                .catalogues
                .values()
                .map(|commands| serde_json::to_vec(commands).map(|bytes| bytes.len()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(Error::storage)?
                .into_iter()
                .sum();
            // Ownership/visibility may have changed even when syntax did not.
            for id in publisher.catalogues.keys() {
                let _ = lease.changes.send(Change::BotCommands(id.clone()));
            }
        }
        Ok(())
    }
    fn publish(&self, lease: &Lease, id: &str, commands: Vec<Command>) -> Result<(), Error> {
        let mut publishers = self.0.lock().map_err(Error::storage)?;
        let publisher = publishers
            .get_mut(&lease.account)
            .filter(|p| p.session == lease.session)
            .ok_or_else(|| Error::new(Code::Unavailable, "Command runtime disconnected."))?;
        if publisher.catalogues.get(id) != Some(&commands) {
            let size = serde_json::to_vec(&commands).map_err(Error::storage)?.len();
            let old = publisher
                .catalogues
                .get(id)
                .map(serde_json::to_vec)
                .transpose()
                .map_err(Error::storage)?
                .map_or(0, |bytes| bytes.len());
            let bytes = publisher.bytes - old + size;
            if bytes > 256 * 1024 {
                return Err(Error::new(
                    Code::Unavailable,
                    "Command metadata capacity exceeded.",
                ));
            }
            publisher.catalogues.insert(id.into(), commands);
            publisher.bytes = bytes;
            let _ = lease.changes.send(Change::BotCommands(id.into()));
        }
        Ok(())
    }
    pub(crate) fn visible(
        &self,
        engine: &Engine,
        user: &str,
        conversation: &str,
    ) -> Result<Vec<serde_json::Value>, crate::engine::EngineError> {
        use crate::engine::EngineError;
        let peers = engine
            .bot_command_peers(user, conversation)
            .map_err(|error| match error {
                EngineError::NotFound(_) | EngineError::Forbidden(_) => {
                    EngineError::Forbidden("Conversation not found or access denied.".into())
                }
                error => error,
            })?;
        let publishers = self
            .0
            .lock()
            .map_err(|_| EngineError::Unavailable("Command registry unavailable.".into()))?;
        let mut bots = Vec::new();
        for (account, username) in peers {
            let Some(publisher) = publishers.get(&account) else {
                continue;
            };
            if publisher
                .context
                .permits(
                    engine,
                    "read",
                    crate::bots::auth::Scope::Conversation(conversation),
                )
                .is_err()
                || publisher
                    .context
                    .permits(
                        engine,
                        "send",
                        crate::bots::auth::Scope::Conversation(conversation),
                    )
                    .is_err()
            {
                continue;
            }
            if let Some(commands) = publisher.catalogues.get(conversation) {
                let commands = visible_commands(commands, engine.bot_operator(user, &account));
                if !commands.is_empty() {
                    bots.push(serde_json::json!({"account_id":account,"username":username,"commands":commands}));
                }
            }
        }
        Ok(bots)
    }
}
#[derive(Default)]
pub(crate) struct Runtime {
    lease: Option<Lease>,
    pending: BTreeMap<String, (String, Instant)>,
    queue: VecDeque<String>,
    attempts: BTreeMap<String, u8>,
}
impl Runtime {
    pub(crate) async fn apply(
        &mut self,
        app: &App,
        context: &Context,
        control: Option<Control>,
    ) -> Result<Option<String>, Error> {
        let mut runtime = std::mem::take(self);
        let app = app.clone();
        let context = context.clone();
        context.verify_async(&app).await?;
        let state = app.clone();
        let (runtime, result) = app
            .inspect_engine(move |engine, _| -> Result<_, Error> {
                let result = match control {
                    Some(control) => runtime.control_state(&state, engine, &context, control),
                    None => runtime
                        .refresh_state(&state, engine, &context)
                        .map(|()| None),
                };
                Ok((runtime, result))
            })
            .await?;
        *self = runtime;
        result
    }
    pub(crate) fn expected(&self, control: &Control) -> bool {
        matches!(control, Control::Response { id, .. } if self.pending.contains_key(id))
    }
    pub(crate) fn active(&self) -> bool {
        self.lease.is_some()
    }
    #[cfg(test)]
    pub(crate) fn refresh(&mut self, app: &App, context: &Context) -> Result<(), Error> {
        let engine = app.engine_read().map_err(Error::storage)?;
        self.refresh_state(app, &engine, context)
    }
    fn refresh_state(
        &mut self,
        app: &App,
        engine: &crate::engine::Engine,
        context: &Context,
    ) -> Result<(), Error> {
        let Some(lease) = &self.lease else {
            return Ok(());
        };
        let user = context.user(engine)?;
        let ids: BTreeSet<_> = engine
            .bot_index
            .conversations
            .keys()
            .filter(|id| {
                context
                    .permits(engine, "read", crate::bots::auth::Scope::Conversation(id))
                    .is_ok()
                    && context
                        .permits(engine, "send", crate::bots::auth::Scope::Conversation(id))
                        .is_ok()
                    && engine.bot_target(&user, id, Action::Read).is_ok()
                    && engine.bot_target(&user, id, Action::Send).is_ok()
            })
            .take(256)
            .cloned()
            .collect();
        app.bot_runtimes.commands.retain(lease, &ids)?;
        self.pending.retain(|_, (id, _)| ids.contains(id));
        self.queue.retain(|id| ids.contains(id));
        self.attempts.retain(|id, _| ids.contains(id));
        for id in ids {
            if !self.pending.values().any(|(pending, _)| pending == &id)
                && !self.queue.contains(&id)
            {
                self.attempts.remove(&id);
                self.queue.push_back(id);
            }
        }
        Ok(())
    }
    pub(crate) fn next(&mut self) -> Option<serde_json::Value> {
        self.pending.retain(|_, (id, at)| {
            if at.elapsed() < REQUEST_TTL {
                return true;
            }
            if self.attempts.get(id).copied().unwrap_or(0) < 3 && !self.queue.contains(id) {
                self.queue.push_back(id.clone());
            }
            false
        });
        if self.pending.len() >= 8 {
            return None;
        }
        let conversation = self.queue.pop_front()?;
        let id = uuid::Uuid::new_v4().to_string();
        *self.attempts.entry(conversation.clone()).or_default() += 1;
        self.pending
            .insert(id.clone(), (conversation.clone(), Instant::now()));
        Some(serde_json::json!({"kind":"commands_request","id":id,"conversation_id":conversation}))
    }
    #[cfg(test)]
    pub(crate) fn control(
        &mut self,
        app: &App,
        context: &Context,
        control: Control,
    ) -> Result<Option<String>, Error> {
        let engine = app.engine_read().map_err(Error::storage)?;
        self.control_state(app, &engine, context, control)
    }
    fn control_state(
        &mut self,
        app: &App,
        engine: &crate::engine::Engine,
        context: &Context,
        control: Control,
    ) -> Result<Option<String>, Error> {
        match control {
            Control::Ready => {
                context.permits(engine, "read", crate::bots::auth::Scope::Global)?;
                if self.lease.is_none() {
                    self.lease = Some(app.bot_runtimes.commands.register(context, &app.changes)?);
                    self.refresh_state(app, engine, context)?;
                }
                Ok(None)
            }
            Control::Changed => {
                if !self.active() {
                    return Err(Error::new(
                        Code::InvalidRequest,
                        "Register commands_ready first.",
                    ));
                }
                if let Some(lease) = &self.lease {
                    app.bot_runtimes.commands.clear(lease)?;
                }
                self.pending.clear();
                self.queue.clear();
                self.attempts.clear();
                self.refresh_state(app, engine, context)?;
                Ok(None)
            }
            Control::Response { id, commands } => {
                let (conversation, at) = self
                    .pending
                    .remove(&id)
                    .ok_or_else(|| Error::new(Code::NotFound, "Unknown command request."))?;
                if at.elapsed() >= REQUEST_TTL {
                    return Err(Error::new(Code::NotFound, "Command request expired."));
                }
                validate(&commands)?;
                let user = context.user(engine)?;
                context.permits(
                    engine,
                    "read",
                    crate::bots::auth::Scope::Conversation(&conversation),
                )?;
                context.permits(
                    engine,
                    "send",
                    crate::bots::auth::Scope::Conversation(&conversation),
                )?;
                engine.bot_target(&user, &conversation, Action::Read)?;
                engine.bot_target(&user, &conversation, Action::Send)?;
                let lease = self.lease.as_ref().ok_or_else(|| {
                    Error::new(Code::Unavailable, "Command runtime disconnected.")
                })?;
                app.bot_runtimes
                    .commands
                    .publish(lease, &conversation, commands)?;
                Ok(Some(id))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_queries_are_bounded_and_expiry_allows_other_conversations() {
        let mut runtime = Runtime {
            queue: (0..10).map(|n| format!("conversation-{n}")).collect(),
            ..Default::default()
        };
        for _ in 0..8 {
            assert!(runtime.next().is_some());
        }
        assert!(runtime.next().is_none());
        for (_, at) in runtime.pending.values_mut() {
            *at = Instant::now() - Duration::from_secs(61);
        }
        assert!(runtime.next().is_some());
        assert_eq!(runtime.pending.len(), 1);
    }
}
