//! Hash outside engine guards; commit account/session changes as one native delta.
use super::*;
use crate::engine::{
    EngineError,
    authorization::{Action, Scope},
    hash_password, verify_password,
};
use crate::web::auth::token;

#[derive(Clone)]
pub(super) struct Request {
    command: String,
    view: Option<String>,
    sudo: bool,
    passwd: bool,
    reset: bool,
    admin: bool,
    name: String,
    password: String,
}
pub(super) fn parse(input: &Input) -> Result<Option<Request>, EngineError> {
    let elevated = sudo_command(&input.text)?;
    let text = elevated.unwrap_or(&input.text);
    let routed = crate::commands::route(text);
    let parts = routed.split_whitespace().collect::<Vec<_>>();
    let provision = crate::commands::provision_command(&parts).map_err(EngineError::Invalid)?;
    let passwd = parts.first() == Some(&"/passwd");
    if !passwd && provision.is_none() {
        return Ok(None);
    }
    if passwd && parts.len() != 3 {
        return Err("Usage: /user passwd old new".into());
    }
    Ok(Some(Request {
        command: text.split_whitespace().next().unwrap_or("").into(),
        view: input.room.clone(),
        sudo: elevated.is_some(),
        passwd,
        reset: provision.is_some_and(|account| account.reset),
        admin: provision.is_some_and(|account| account.admin),
        name: provision.map_or(parts[1], |account| account.name).into(),
        password: provision
            .map_or(parts[2], |account| account.password)
            .into(),
    }))
}
impl Request {
    fn authorize(&self, engine: &Engine, user: &str) -> Result<Option<String>, EngineError> {
        engine.require_command(Some(user), self.view.as_deref(), &self.command)?;
        if self.passwd {
            let scope = engine.account_target(Some(user), user, Action::Password)?;
            engine.require_target_command(Some(user), &scope, &self.command)?;
            Ok(None)
        } else if self.reset {
            let scope = engine.account_target(Some(user), &self.name, Action::Reset)?;
            engine.require_target_command(Some(user), &scope, &self.command)?;
            Ok(Some(
                engine
                    .account_id(&self.name)
                    .ok_or("User not found.")?
                    .into(),
            ))
        } else {
            engine.require_target_command(Some(user), &Scope::Server, &self.command)?;
            engine.require(Some(user), &Scope::Server, Action::CreateAccount)?;
            if self.admin {
                engine.require(Some(user), &Scope::Server, Action::AssignAdmin)?;
            }
            Ok(None)
        }
    }
}
pub(super) async fn execute(
    app: &App,
    headers: HeaderMap,
    user: String,
    request: Request,
) -> Result<(String, Option<Change>), EngineError> {
    let account = app
        .inspect_engine({
            let headers = headers.clone();
            let user = user.clone();
            move |engine, config| -> Result<_, EngineError> {
                if identity(engine, config, &headers).as_deref() != Some(&user) {
                    return Err("Please log in.".into());
                }
                Ok(engine
                    .active_account_id(&user)
                    .ok_or("Account unavailable.")?
                    .to_owned())
            }
        })
        .await?;
    let _account = app
        .admission
        .http(account.clone())
        .map_err(EngineError::Unavailable)?;
    let current = super::auth::bearer(&headers)
        .or_else(|| token(&app.config, &headers))
        .ok_or("Please log in.")?
        .to_owned();
    let identity = auth::Identity {
        headers,
        user,
        account,
        config: app.config.clone(),
    };
    let (target, expected) = {
        let ordering = app.mutations.conversations(vec![]).await?;
        app.with_conversation(&ordering, {
            let request = request.clone();
            let identity = identity.clone();
            move |engine| -> Result<_, EngineError> {
                identity.verify(engine)?;
                engine.with_command_authority(
                    Some(&identity.user),
                    request.view.as_deref(),
                    request.sudo,
                    |engine| {
                        let target = request.authorize(engine, &identity.user)?;
                        let expected = if request.passwd {
                            Some(
                                engine
                                    .password_hash(&identity.user)
                                    .ok_or("Account unavailable.")?
                                    .to_owned(),
                            )
                        } else {
                            None
                        };
                        Ok((target, expected))
                    },
                )
            }
        })
        .await?
    };
    let hash = tokio::task::spawn_blocking({
        let permit = app
            .password_jobs
            .clone()
            .try_acquire_owned()
            .map_err(|_| "Account management busy. Try again shortly.")?;
        let old = request.name.clone();
        let password = request.password.clone();
        let expected = expected.clone();
        move || -> Result<_, EngineError> {
            let _permit = permit;
            if let Some(hash) = expected
                && (old.len() > crate::limits::MAX_PASSWORD_BYTES || !verify_password(&old, &hash))
            {
                return Err("Old password is incorrect.".into());
            }
            hash_password(&password)
        }
    })
    .await
    .map_err(|_| EngineError::Unavailable("Password worker unavailable.".into()))??;
    let passwd = request.passwd;
    let ordering = app.mutations.metadata().await;
    let (reply, pending) = app
        .with_metadata(&ordering, {
            let identity = identity.clone();
            move |engine| -> Result<_, EngineError> {
                identity.verify(engine)?;
                engine.with_command_authority(
                    Some(&identity.user),
                    request.view.as_deref(),
                    request.sudo,
                    |engine| {
                        let current_target = request.authorize(engine, &identity.user)?;
                        if current_target != target {
                            return Err("Account changed. Try again.".into());
                        }
                        if request.passwd {
                            engine.prepare_password_change(
                                &identity.user,
                                expected.as_deref().ok_or("Account unavailable.")?,
                                hash,
                                &current,
                            )
                        } else {
                            engine.prepare_provision(
                                Some(&identity.user),
                                &request.name,
                                hash,
                                request.admin,
                                request.reset,
                            )
                        }
                    },
                )
            }
        })
        .await?;
    let pending = pending.save().await?;
    let changes = app.changes.clone();
    app.with_metadata(&ordering, move |engine| -> Result<_, EngineError> {
        let delivery = identity.verify(engine);
        engine.publish_metadata(pending)?;
        let change = if passwd {
            Change::Account(identity.user)
        } else {
            Change::Metadata(engine.metadata_change())
        };
        let _ = changes.send(change);
        delivery?;
        Ok((reply, None))
    })
    .await
}
