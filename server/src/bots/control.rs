//! Native server commands and verified source facts; workers own their command ACLs.
use super::{
    api::{admit, inspect, work},
    auth::Context,
    error::{Code, Error},
};
use crate::{
    engine::authorization::Action,
    web::{App, Change},
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
};
use serde::Deserialize;
use serde_json::{Value, json};

// Browser-local controls, password/provisioning jobs and worker administration
// remain on their own authenticated control paths, outside model command tools.
fn supported_command(name: &str) -> bool {
    matches!(
        name,
        "/user"
            | "/room"
            | "/help"
            | "/man"
            | "/whoami"
            | "/ls"
            | "/members"
            | "/history"
            | "/new"
            | "/add"
            | "/kick"
            | "/join"
            | "/leave"
            | "/rm"
            | "/owner"
            | "/react"
            | "/reply"
            | "/retract"
            | "/edit"
            | "/clean"
            | "/tell"
            | "/enable"
            | "/disable"
            | "/grant"
            | "/revoke"
            | "/permissions"
            | "/rename"
            | "/configs"
            | "/sudo"
    )
}
fn model_commands(
    engine: &crate::engine::Engine,
    user: &str,
    view: &str,
) -> Vec<crate::commands::Command> {
    engine
        .commands_for(Some(user), Some(view))
        .into_iter()
        .filter(|command| supported_command(command.name))
        .map(|mut command| {
            if command.name == "/user" {
                command.usage = "/user ls [scope] | get who [key] [scope] | set who key value [scope] | rename [who] name | delete who | enable who | disable who";
            }
            command
        })
        .collect()
}
fn server_command_json(command: &crate::commands::Command) -> Value {
    let mut value =
        json!({"name":command.name,"usage":command.usage,"description":command.description});
    let children: &[(&str, &str, &str)] = match command.name {
        "/user" => &[
            ("ls", "/user ls [scope]", "List users in a scope"),
            ("get", "/user get who [key] [scope]", "Read user metadata"),
            (
                "set",
                "/user set who key value [scope]",
                "Set user metadata",
            ),
            (
                "rename",
                "/user rename [who] name",
                "Rename an authorized account",
            ),
            ("delete", "/user delete who", "Delete an authorized account"),
            ("enable", "/user enable who", "Enable an account"),
            ("disable", "/user disable who", "Disable an account"),
        ],
        "/room" => &[
            ("ls", "/room ls", "List rooms"),
            ("new", "/room new #name", "Create a room when granted"),
            ("delete", "/room delete #name", "Delete an authorized room"),
            (
                "rename",
                "/room rename #name new-name",
                "Rename an authorized room",
            ),
            ("join", "/room join #name", "Open a room"),
            ("leave", "/room leave [#name]", "Leave a room"),
            ("add", "/room add user [#name]", "Invite a user"),
            ("kick", "/room kick user [#name]", "Remove a user"),
            ("owner", "/room owner user [#name]", "Transfer ownership"),
        ],
        _ => &[],
    };
    if !children.is_empty() {
        value["subcommands"] = json!(children.iter().map(|(name,usage,description)| json!({"name":name,"usage":usage,"description":description})).collect::<Vec<_>>());
    }
    value
}
fn command_view(
    engine: &crate::engine::Engine,
    context: &Context,
    id: &str,
) -> Result<(String, String), Error> {
    let user = context.user(engine)?;
    context.permits(engine, "commands", Some(id))?;
    context.permits(engine, "read", Some(id))?;
    // Text commands can name additional targets. A conversation-scoped token
    // cannot be routed through that grammar without escaping its token scope.
    if context.metadata(engine)?.conversations.is_some() {
        return Err(Error::new(
            Code::Forbidden,
            "Server commands require an account-wide conversation scope.",
        ));
    }
    Ok((
        user.clone(),
        engine.bot_target(&user, id, Action::Read)?.view,
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CommandsQuery {
    conversation_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MessageContextQuery {
    conversation_id: String,
    message_id: String,
}
pub(super) async fn message_context(
    State(app): State<App>,
    headers: HeaderMap,
    Query(input): Query<MessageContextQuery>,
) -> Result<Json<Value>, Error> {
    work(app, headers, move |app, context| {
        let (revision, message) = inspect(app, context, &input.conversation_id, |store| {
            store.locate(&input.message_id)
        })?;
        let message = message
            .filter(|(id, _)| id == &input.conversation_id)
            .map(|(_, message)| message)
            .ok_or_else(|| Error::new(Code::NotFound, "Source message not found."))?;
        let engine = app.engine_read().map_err(Error::storage)?;
        let user = context.user(&engine)?;
        context.permits(&engine, "config", Some(&input.conversation_id))?;
        context.permits(&engine, "read", Some(&input.conversation_id))?;
        if engine
            .bot_target(&user, &input.conversation_id, Action::Read)?
            .revision
            != revision
        {
            return Err(Error::new(
                Code::Conflict,
                "Source message changed; retry authorization.",
            ));
        }
        let author = engine.active_account_name(&message.author_id)
            .ok_or_else(||Error::new(Code::Forbidden,"Source account unavailable."))?;
        let target = engine.bot_target(&user,&input.conversation_id,Action::Read)?;
        let permissions = engine.allows(Some(&user),&target.scope,Action::PolicyRead)
            .then(||engine.effective(Some(author),&target.scope).names());
        let owner_id = engine.bot_owner_id(&context.account);
        Ok(json!({"author":{"id":message.author_id,"username":author,"groups":engine.groups(Some(author)),"permissions":permissions},"bot":{"id":context.account,"groups":engine.groups(Some(&user)),"owner_id":owner_id,"owner":engine.bot_owner(&context.account)}}))
    })
    .await
}
pub(super) async fn commands(
    State(app): State<App>,
    headers: HeaderMap,
    Query(input): Query<CommandsQuery>,
) -> Result<Json<Value>, Error> {
    work(app, headers, move |app, context| {
        let engine = app.engine_read().map_err(Error::storage)?;
        let (user, view) = command_view(&engine, context, &input.conversation_id)?;
        let commands: Vec<_> = model_commands(&engine, &user, &view)
            .iter()
            .map(server_command_json)
            .collect();
        Ok(json!({"conversation_id":input.conversation_id,"commands":commands}))
    })
    .await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CommandInput {
    conversation_id: String,
    command: String,
}
pub(super) async fn command(
    State(app): State<App>,
    headers: HeaderMap,
    Json(input): Json<CommandInput>,
) -> Result<Json<Value>, Error> {
    let key = super::api::idempotency_key(&headers)?;
    work(app, headers, move |app, context| {
        let mut engine = app.engine().map_err(Error::storage)?;
        command_account(&engine, context)?;
        admit(&mut engine, context)?;
        let digest = crate::bots::digest(
            &serde_json::to_string(
                &json!({"conversation_id":input.conversation_id,"command":input.command.trim()}),
            )
            .map_err(Error::storage)?,
        );
        let before = engine.revision;
        let outcome = engine.bot_command_once(&context.account, &key, &digest, |engine| {
            execute_command(engine, app, context, &input)
        })?;
        if before != engine.revision {
            let _ = app
                .changes
                .send(engine.changed_conversation.as_ref().map_or_else(
                    || Change::Metadata(engine.metadata_change()),
                    |id| Change::Conversation(id.clone()),
                ));
        }
        outcome.into_result()
    })
    .await
}
fn command_account(engine: &crate::engine::Engine, context: &Context) -> Result<(), Error> {
    context.user(engine)?;
    context.permits(engine, "commands", None)?;
    context.permits(engine, "read", None)?;
    if context.metadata(engine)?.conversations.is_some() {
        return Err(Error::new(
            Code::Forbidden,
            "Command receipts require account-wide conversation scope.",
        ));
    }
    Ok(())
}
pub(super) async fn command_receipt(
    State(app): State<App>,
    headers: HeaderMap,
    Path(key): Path<String>,
) -> Result<Json<Value>, Error> {
    if !super::models::valid_id(&key) {
        return Err(Error::new(Code::InvalidRequest, "Invalid receipt key."));
    }
    work(app,headers,move|app,context|{
        let engine=app.engine_read().map_err(Error::storage)?;
        command_account(&engine,context)?;
        let receipt=engine.bot_command_receipt(&context.account,&key)?;
        Ok(match receipt {
            None=>json!({"state":"not_found"}),
            Some((_,outcome))=>json!({"state":if outcome.error.is_some(){"failed"}else{"succeeded"},"result":outcome.result,"error":outcome.error,"created":outcome.created,"expires":outcome.expires}),
        })
    }).await
}
fn execute_command(
    engine: &mut crate::engine::Engine,
    app: &App,
    context: &Context,
    input: &CommandInput,
) -> Result<String, Error> {
    let (user, view) = command_view(engine, context, &input.conversation_id)?;
    let parts: Vec<_> = input.command.split_whitespace().collect();
    let elevated = crate::engine::sudo_command(&input.command)
        .map_err(|e| Error::new(Code::InvalidRequest, e))?;
    let inner: Vec<_> = elevated
        .unwrap_or(&input.command)
        .split_whitespace()
        .collect();
    if input.command.len() > 4100
        || input
            .command
            .chars()
            .any(|c| c.is_control() && !c.is_whitespace())
        || !parts.first().is_some_and(|name| supported_command(name))
        || !inner.first().is_some_and(|name| supported_command(name))
        || inner.starts_with(&["/new", "user"])
        || (inner.first() == Some(&"/user")
            && inner
                .get(1)
                .is_some_and(|v| ["new", "passwd", "reset"].contains(v)))
    {
        return Err(Error::new(
            Code::InvalidRequest,
            "Unsupported server command.",
        ));
    }
    engine
        .require_command(Some(&user), Some(&view), parts[0])
        .map_err(Error::from)?;
    let result = if inner.as_slice() == ["/help"] {
        engine
            .with_command_authority(Some(&user), Some(&view), elevated.is_some(), |engine| {
                engine.require_command(Some(&user), Some(&view), "/help")?;
                Ok(crate::commands::help_for(&model_commands(
                    engine, &user, &view,
                )))
            })
            .map_err(Error::from)?
    } else if inner.as_slice() == ["/configs"] {
        engine
            .with_command_authority(Some(&user), Some(&view), elevated.is_some(), |engine| {
                use crate::engine::authorization::Scope;
                engine.require_command(Some(&user), Some(&view), "/configs")?;
                engine.require_target_command(Some(&user), &Scope::Server, "/configs")?;
                engine.require(Some(&user), &Scope::Server, Action::Config)?;
                app.config
                    .display_with_history(engine.history_store().stats()?)
                    .map_err(crate::engine::EngineError::Invalid)
            })
            .map_err(Error::from)?
    } else {
        engine
            .execute_formatted(Some(&user), Some(&view), &input.command, true)
            .map_err(Error::from)?
    };
    Ok(result)
}
