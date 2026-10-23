use super::auth::identity;
#[path = "credential_commands.rs"]
mod credentials;
use super::*;
use crate::engine::EngineError;
use crate::engine::{
    authorization::{Action, Scope},
    sudo_command,
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
    crate::limits::validate_input(&input.text)?;
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
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let account = app
            .engine_read()?
            .active_account_id(&user)
            .ok_or("Account unavailable.")?
            .to_owned();
        let _account_permit = app.admission.http(account)?;
        execute_input_inner(&app, &headers, &user, &input).map_err(String::from)
    })
    .await
    .map_err(|_| "Command worker unavailable.")?
}
fn execute_input_inner(
    app: &App,
    headers: &HeaderMap,
    user: &str,
    input: &Input,
) -> Result<(String, Option<Change>), EngineError> {
    {
        let engine = app.engine_read()?;
        if identity(&engine, &app.config, headers).as_deref() != Some(user) {
            return Err("Please log in.".into());
        }
    }
    let elevated = sudo_command(&input.text)?;
    let text = elevated.unwrap_or(&input.text);
    let sudo = elevated.is_some();
    let command = text.split_whitespace().next().unwrap_or("");
    let routed = crate::commands::route(text);
    let parts: Vec<_> = routed.split_whitespace().collect();
    let provision = crate::commands::provision_command(&parts).map_err(EngineError::Invalid)?;
    if sudo && matches!(parts.first(), Some(&"/clear" | &"/logout")) {
        return app.engine()?.with_command_authority(
            Some(user),
            input.room.as_deref(),
            true,
            |_| {
                if parts.len() != 1 {
                    return Err(EngineError::Invalid(format!("Usage: {}", parts[0])));
                }
                Ok(("Done.".into(), None))
            },
        );
    }
    if parts.first() == Some(&"/configs") {
        let mut engine = app.engine()?;
        if identity(&engine, &app.config, headers).as_deref() != Some(user) {
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
                .display_with_history(engine.history_store().stats()?)
                .map_err(EngineError::Invalid)
                .map(|text| (text, None))
        });
    }
    let passwd = parts.first() == Some(&"/passwd");
    if !passwd && provision.is_none() {
        if !sudo {
            let engine = app.engine_read()?;
            if identity(&engine, &app.config, headers).as_deref() != Some(user) {
                return Err("Please log in.".into());
            }
            if let Some(result) =
                engine.read_command(Some(user), input.room.as_deref(), &input.text)
            {
                return result.map(|reply| (reply, None));
            }
        }
        let mut engine = app.engine()?;
        if identity(&engine, &app.config, headers).as_deref() != Some(user) {
            return Err("Please log in.".into());
        }
        let outcome = engine.execute_command(
            Some(user),
            input.room.as_deref(),
            &input.text,
            input.markdown,
        )?;
        let change = Change::command(outcome.change);
        return Ok((outcome.reply, change));
    }
    credentials::execute(app, headers, user, input, sudo, command, &parts)
}
