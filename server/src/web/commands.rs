use super::auth::token;
use super::*;
use crate::engine::{
    authorization::{Action, Scope},
    hash_password, verify_password,
};
use axum::http::HeaderMap;
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct Input {
    pub(super) id: u64,
    pub(super) room: Option<String>,
    pub(super) text: String,
}
pub(super) async fn execute_input(
    app: &App,
    headers: &HeaderMap,
    user: &str,
    input: &Input,
) -> Result<(String, bool), String> {
    let parts: Vec<_> = input.text.split_whitespace().collect();
    if parts.first() == Some(&"/configs") {
        let engine = app.engine()?;
        engine.require_command(Some(user), input.room.as_deref(), "/configs")?;
        engine.require_target_command(Some(user), &Scope::Server, "/configs")?;
        engine.require(Some(user), &Scope::Server, Action::Config)?;
        if parts.len() != 1 {
            return Err("Usage: /configs".into());
        }
        return app.config.display().map(|text| (text, false));
    }
    let passwd = parts.first() == Some(&"/passwd");
    if !passwd && !matches!(parts.first(), Some(&"/user") | Some(&"/reset")) {
        let mut engine = app.engine()?;
        let before = engine.revision;
        let reply = engine.execute(Some(user), input.room.as_deref(), &input.text)?;
        return Ok((reply, engine.revision != before));
    }
    let reset = parts[0] == "/reset";
    if input.text.len() > 4000
        || ((reset || passwd) && parts.len() != 3)
        || (!reset && !passwd && !(3..=4).contains(&parts.len()))
    {
        return Err(if passwd {
            "Usage: /passwd old new"
        } else {
            "Usage: /user name password [admin|user] or /reset name password"
        }
        .into());
    }
    if !passwd
        && !reset
        && parts
            .get(3)
            .is_some_and(|role| !["admin", "user"].contains(role))
    {
        return Err("Role must be admin or user.".into());
    }
    let target_id = {
        let engine = app.engine()?;
        engine.require_command(Some(user), input.room.as_deref(), parts[0])?;
        if passwd {
            let scope = engine.account_target(Some(user), user, Action::Password)?;
            engine.require_target_command(Some(user), &scope, parts[0])?;
            None
        } else if reset {
            let scope = engine.account_target(Some(user), parts[1], Action::Reset)?;
            engine.require_target_command(Some(user), &scope, parts[0])?;
            Some(
                engine
                    .data
                    .users
                    .get(parts[1])
                    .ok_or("User not found.")?
                    .id
                    .clone(),
            )
        } else {
            engine.require_target_command(Some(user), &Scope::Server, parts[0])?;
            engine.require(Some(user), &Scope::Server, Action::CreateAccount)?;
            if parts.get(3) == Some(&"admin") {
                engine.require(Some(user), &Scope::Server, Action::AssignAdmin)?;
            }
            None
        }
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
    let old = parts[1].to_owned();
    let password = parts[2].to_owned();
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
    let current = token(headers).ok_or("Please log in.")?;
    if engine.session(current).as_deref() != Some(user) {
        return Err("Please log in.".into());
    }
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
            .map(|text| (text, true));
    }
    // Recheck the command, action and target identity after hashing.
    engine.require_command(Some(user), input.room.as_deref(), parts[0])?;
    if reset {
        let scope = engine.account_target(Some(user), parts[1], Action::Reset)?;
        engine.require_target_command(Some(user), &scope, parts[0])?;
        if engine.data.users.get(parts[1]).map(|u| &u.id) != target_id.as_ref() {
            return Err("Account changed. Try again.".into());
        }
    } else {
        engine.require_target_command(Some(user), &Scope::Server, parts[0])?;
        engine.require(Some(user), &Scope::Server, Action::CreateAccount)?;
        if parts.get(3) == Some(&"admin") {
            engine.require(Some(user), &Scope::Server, Action::AssignAdmin)?;
        }
    }
    engine
        .provision_by(
            Some(user),
            parts[1],
            hash,
            parts.get(3) == Some(&"admin"),
            reset,
        )
        .map(|text| (text, true))
}
