use super::auth::token;
use super::*;
use crate::engine::{hash_password, verify_password};
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
        if !app.engine()?.is_admin(user) {
            return Err("Admin permission required.".into());
        }
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
    if !passwd && !app.engine()?.is_admin(user) {
        return Err("Admin permission required.".into());
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
        return engine
            .change_password(
                user,
                expected.as_deref().ok_or("Account unavailable.")?,
                hash,
                current,
            )
            .map(|text| (text, true));
    }
    // Recheck access after hashing, without constructing a chat snapshot.
    if !engine.is_admin(user) {
        return Err("Admin permission required.".into());
    }
    engine
        .provision(parts[1], hash, parts.get(3) == Some(&"admin"), reset)
        .map(|text| (text, true))
}
