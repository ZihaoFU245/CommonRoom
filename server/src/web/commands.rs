use super::auth::{identity, token};
use super::*;
use crate::engine::{
    authorization::{Action, Scope},
    hash_password, sudo_command, verify_password,
};
use axum::http::HeaderMap;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct Input {
    pub(crate) id: u64,
    pub(crate) room: Option<String>,
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) markdown: bool,
}
pub(crate) async fn execute_input(
    app: &App,
    headers: &HeaderMap,
    user: &str,
    input: &Input,
) -> Result<(String, Option<Change>), String> {
    let permit = app.io_permit()?;
    let app = app.clone();
    let headers = headers.clone();
    let user = user.to_owned();
    let input = Input {
        id: input.id,
        markdown: input.markdown,
        room: input.room.clone(),
        text: input.text.clone(),
    };
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        runtime.block_on(execute_input_inner(&app, &headers, &user, &input))
    })
    .await
    .map_err(|_| "Command worker unavailable.")?
}
async fn execute_input_inner(
    app: &App,
    headers: &HeaderMap,
    user: &str,
    input: &Input,
) -> Result<(String, Option<Change>), String> {
    {
        let engine = app.engine()?;
        if identity(&engine, headers).as_deref() != Some(user) {
            return Err("Please log in.".into());
        }
    }
    if let Some(secret) = super::auth::bearer(headers) {
        app.engine()?.admit_bot_input(secret)?;
    }
    let elevated = sudo_command(&input.text)?;
    let text = elevated.unwrap_or(&input.text);
    let sudo = elevated.is_some();
    let parts: Vec<_> = text.split_whitespace().collect();
    if let Some(command) = crate::bots::runtime::ConfigCommand::parse(text)? {
        let authority = crate::bots::runtime::Authority {
            actor: Some(user.to_owned()),
            headers: headers.clone(),
            view: input.room.clone(),
            sudo,
        };
        return app
            .bot_runtimes
            .execute(app, authority, command)
            .await
            .map(|reply| (reply, None));
    }
    let provision = crate::commands::provision_command(&parts)?;
    if sudo && matches!(parts.first(), Some(&"/clear" | &"/logout")) {
        return app.engine()?.with_command_authority(
            Some(user),
            input.room.as_deref(),
            true,
            |_| {
                if parts.len() != 1 {
                    return Err(format!("Usage: {}", parts[0]));
                }
                Ok(("Done.".into(), None))
            },
        );
    }
    if parts.first() == Some(&"/configs") {
        let mut engine = app.engine()?;
        if identity(&engine, headers).as_deref() != Some(user) {
            return Err("Please log in.".into());
        }
        return engine.with_command_authority(Some(user), input.room.as_deref(), sudo, |engine| {
            engine.require_command(Some(user), input.room.as_deref(), "/configs")?;
            engine.require_target_command(Some(user), &Scope::Server, "/configs")?;
            engine.require(Some(user), &Scope::Server, Action::Config)?;
            if parts.len() != 1 {
                return Err("Usage: /configs".into());
            }
            app.config
                .display_with_history(engine.history_store.stats()?)
                .map(|text| (text, None))
        });
    }
    let passwd = parts.first() == Some(&"/passwd");
    if !passwd && provision.is_none() {
        let mut engine = app.engine()?;
        if identity(&engine, headers).as_deref() != Some(user) {
            return Err("Please log in.".into());
        }
        let before = engine.revision;
        let reply = engine.execute_formatted(
            Some(user),
            input.room.as_deref(),
            &input.text,
            input.markdown,
        )?;
        let change = (engine.revision != before).then(|| {
            engine
                .changed_conversation
                .as_ref()
                .map_or(Change::All, |id| Change::Conversation(id.clone()))
        });
        return Ok((reply, change));
    }
    if passwd && parts.len() != 3 {
        return Err("Usage: /passwd old new".into());
    }
    if input.text.len() > 4000 {
        return Err("Password commands support at most 4000 bytes.".into());
    }
    let reset = provision.is_some_and(|account| account.reset);
    let admin = provision.is_some_and(|account| account.admin);
    let name = provision.map_or(parts[1], |account| account.name);
    let password = provision.map_or(parts[2], |account| account.password);
    let target_id = {
        let mut engine = app.engine()?;
        engine.with_command_authority(Some(user), input.room.as_deref(), sudo, |engine| {
            engine.require_command(Some(user), input.room.as_deref(), parts[0])?;
            if passwd {
                let scope = engine.account_target(Some(user), user, Action::Password)?;
                engine.require_target_command(Some(user), &scope, parts[0])?;
                Ok(None)
            } else if reset {
                let scope = engine.account_target(Some(user), name, Action::Reset)?;
                engine.require_target_command(Some(user), &scope, parts[0])?;
                Ok(Some(
                    engine
                        .data
                        .users
                        .get(name)
                        .ok_or("User not found.")?
                        .id
                        .clone(),
                ))
            } else {
                engine.require_target_command(Some(user), &Scope::Server, parts[0])?;
                engine.require(Some(user), &Scope::Server, Action::CreateAccount)?;
                if admin {
                    engine.require(Some(user), &Scope::Server, Action::AssignAdmin)?;
                }
                Ok(None)
            }
        })?
    };
    let expected = if passwd {
        Some(
            app.engine()?
                .data
                .users
                .get(user)
                .filter(|u| !u.disabled)
                .ok_or("Account unavailable.")?
                .hash
                .clone(),
        )
    } else {
        None
    };
    let old = name.to_owned();
    let password = password.to_owned();
    let expected_job = expected.clone();
    let permit = app
        .password_jobs
        .clone()
        .try_acquire_owned()
        .map_err(|_| "Account management busy. Try again shortly.")?;
    let hash = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if let Some(hash) = expected_job
            && (old.len() > 128 || !verify_password(&old, &hash))
        {
            return Err("Old password is incorrect.".into());
        }
        hash_password(&password)
    })
    .await
    .map_err(|_| "Password hashing failed.")??;
    let mut engine = app.engine()?;
    let current = super::auth::bearer(headers)
        .or_else(|| token(headers))
        .ok_or("Please log in.")?;
    if identity(&engine, headers).as_deref() != Some(user) {
        return Err("Please log in.".into());
    }
    engine.with_command_authority(Some(user), input.room.as_deref(), sudo, |engine| {
        if passwd {
            engine.require_command(Some(user), input.room.as_deref(), "/passwd")?;
            let scope = engine.account_target(Some(user), user, Action::Password)?;
            engine.require_target_command(Some(user), &scope, parts[0])?;
            return engine
                .change_password(
                    user,
                    expected.as_deref().ok_or("Account unavailable.")?,
                    hash,
                    current,
                )
                .map(|text| (text, Some(Change::All)));
        }
        // Recheck the command, action and target identity after hashing.
        engine.require_command(Some(user), input.room.as_deref(), parts[0])?;
        if reset {
            let scope = engine.account_target(Some(user), name, Action::Reset)?;
            engine.require_target_command(Some(user), &scope, parts[0])?;
            if engine.data.users.get(name).map(|u| &u.id) != target_id.as_ref() {
                return Err("Account changed. Try again.".into());
            }
        } else {
            engine.require_target_command(Some(user), &Scope::Server, parts[0])?;
            engine.require(Some(user), &Scope::Server, Action::CreateAccount)?;
            if admin {
                engine.require(Some(user), &Scope::Server, Action::AssignAdmin)?;
            }
        }
        engine
            .provision_by(Some(user), name, hash, admin, reset)
            .map(|text| (text, Some(Change::All)))
    })
}
