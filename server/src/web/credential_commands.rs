//! Password verification and account provisioning run in the command worker.
//! No engine guard is held during password hashing; authority is rechecked after it.
use super::*;
use crate::engine::{EngineError, hash_password, verify_password};
use crate::web::auth::token;

pub(super) fn execute(
    app: &App,
    headers: &HeaderMap,
    user: &str,
    input: &Input,
    sudo: bool,
    command: &str,
    parts: &[&str],
) -> Result<(String, Option<Change>), EngineError> {
    let provision = crate::commands::provision_command(parts).map_err(EngineError::Invalid)?;
    let passwd = parts.first() == Some(&"/passwd");
    if passwd && parts.len() != 3 {
        return Err("Usage: /user passwd old new".into());
    }
    let reset = provision.is_some_and(|account| account.reset);
    let admin = provision.is_some_and(|account| account.admin);
    let name = provision.map_or(parts[1], |account| account.name);
    let password = provision.map_or(parts[2], |account| account.password);
    let target_id = {
        let mut engine = app.engine()?;
        engine.with_command_authority(Some(user), input.room.as_deref(), sudo, |engine| {
            engine.require_command(Some(user), input.room.as_deref(), command)?;
            if passwd {
                let scope = engine.account_target(Some(user), user, Action::Password)?;
                engine.require_target_command(Some(user), &scope, command)?;
                Ok(None)
            } else if reset {
                let scope = engine.account_target(Some(user), name, Action::Reset)?;
                engine.require_target_command(Some(user), &scope, command)?;
                Ok(Some(
                    engine.account_id(name).ok_or("User not found.")?.to_owned(),
                ))
            } else {
                engine.require_target_command(Some(user), &Scope::Server, command)?;
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
                .password_hash(user)
                .ok_or("Account unavailable.")?
                .to_owned(),
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
    let hash = {
        let _permit = permit;
        if let Some(hash) = expected_job
            && (old.len() > crate::limits::MAX_PASSWORD_BYTES || !verify_password(&old, &hash))
        {
            return Err("Old password is incorrect.".into());
        }
        hash_password(&password)?
    };
    let mut engine = app.engine()?;
    let current = super::auth::bearer(headers)
        .or_else(|| token(&app.config, headers))
        .ok_or("Please log in.")?;
    if identity(&engine, &app.config, headers).as_deref() != Some(user) {
        return Err("Please log in.".into());
    }
    engine.with_command_authority(Some(user), input.room.as_deref(), sudo, |engine| {
        if passwd {
            engine.require_command(Some(user), input.room.as_deref(), command)?;
            let scope = engine.account_target(Some(user), user, Action::Password)?;
            engine.require_target_command(Some(user), &scope, command)?;
            return engine
                .change_password(
                    user,
                    expected.as_deref().ok_or("Account unavailable.")?,
                    hash,
                    current,
                )
                .map(|text| (text, Some(Change::Account(user.to_owned()))));
        }
        // Recheck the command, action and target identity after hashing.
        engine.require_command(Some(user), input.room.as_deref(), command)?;
        if reset {
            let scope = engine.account_target(Some(user), name, Action::Reset)?;
            engine.require_target_command(Some(user), &scope, command)?;
            if engine.account_id(name) != target_id.as_deref() {
                return Err("Account changed. Try again.".into());
            }
        } else {
            engine.require_target_command(Some(user), &Scope::Server, command)?;
            engine.require(Some(user), &Scope::Server, Action::CreateAccount)?;
            if admin {
                engine.require(Some(user), &Scope::Server, Action::AssignAdmin)?;
            }
        }
        engine
            .provision_by(Some(user), name, hash, admin, reset)
            .map(|text| (text, Some(Change::Metadata(engine.metadata_change()))))
    })
}
