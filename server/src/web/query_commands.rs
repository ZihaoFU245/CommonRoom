//! Read-only command phases capture authority briefly, then await native readers.
use super::commands::Input;
use super::*;
use crate::engine::{
    CommandRead, EngineError,
    authorization::{Action, Scope},
    sudo_command,
};
use std::collections::BTreeSet;

enum Query {
    Read(CommandRead),
    Configs,
}
impl Query {
    fn ids(&self) -> BTreeSet<String> {
        match self {
            Self::Read(plan) => plan.ids(),
            Self::Configs => BTreeSet::new(),
        }
    }
}
pub(super) fn supports(input: &str) -> Result<bool, EngineError> {
    let elevated = sudo_command(input)?;
    let input = elevated.unwrap_or(input);
    if elevated.is_some() && matches!(input.split_whitespace().next(), Some("/clear" | "/logout")) {
        return Ok(true);
    }
    let routed = crate::commands::route(input);
    let parts = routed.split_whitespace().collect::<Vec<_>>();
    Ok(match parts.as_slice() {
        ["/user", "ls" | "get", ..] | ["/room", "ls"] => true,
        ["/bot", "ls" | "info", ..] | ["/bot", "token", "ls", ..] => true,
        ["/clean", ..] => parts.last() == Some(&"--dry-run"),
        [
            "/configs" | "/permissions" | "/debug" | "/help" | "/man" | "/whoami" | "/console"
            | "/ls" | "/members" | "/history",
            ..,
        ] => true,
        _ => false,
    })
}
fn prepare(
    engine: &Engine,
    identity: &auth::Identity,
    input: &Input,
    text: &str,
) -> Result<Query, EngineError> {
    identity.verify(engine)?;
    if sudo_command(&input.text)?.is_some()
        && matches!(text.split_whitespace().next(), Some("/clear" | "/logout"))
    {
        let parts = text.split_whitespace().collect::<Vec<_>>();
        if parts.len() != 1 {
            return Err(EngineError::Invalid(format!("Usage: {}", parts[0])));
        }
        return Ok(Query::Read(CommandRead::Text("Done.".into())));
    }
    if text.split_whitespace().next() == Some("/configs") {
        engine.require_command(Some(&identity.user), input.room.as_deref(), "/configs")?;
        engine.require_target_command(Some(&identity.user), &Scope::Server, "/configs")?;
        engine.require(Some(&identity.user), &Scope::Server, Action::Config)?;
        if text.split_whitespace().count() != 1 {
            return Err("Usage: /configs".into());
        }
        return Ok(Query::Configs);
    }
    engine
        .prepare_read_command(Some(&identity.user), input.room.as_deref(), text)
        .ok_or("Command is not read-only.")?
        .map(Query::Read)
}

// Sudo exists only inside each synchronous capture. Neither the metadata fence
// nor temporary authority survives a database await.
async fn capture(
    app: &App,
    identity: auth::Identity,
    input: Input,
) -> Result<(Query, crate::engine::DeliveryStamp), EngineError> {
    let elevated = sudo_command(&input.text)?.map(str::to_owned);
    if let Some(text) = elevated {
        let ordering = app.mutations.conversations(vec![]).await?;
        app.with_conversation(&ordering, move |engine| {
            identity.verify(engine)?;
            engine.with_command_authority(
                Some(&identity.user),
                input.room.as_deref(),
                true,
                |engine| {
                    let plan = prepare(engine, &identity, &input, &text)?;
                    let stamp = engine.delivery_stamp(&identity.user, &plan.ids(), false)?;
                    Ok((plan, stamp))
                },
            )
        })
        .await
    } else {
        app.inspect_engine(move |engine, _| {
            let plan = prepare(engine, &identity, &input, &input.text)?;
            let stamp = engine.delivery_stamp(&identity.user, &plan.ids(), false)?;
            Ok((plan, stamp))
        })
        .await
    }
}

pub(super) async fn execute(
    app: &App,
    headers: axum::http::HeaderMap,
    user: String,
    input: Input,
) -> Result<(String, Option<Change>), EngineError> {
    let (account, history) = app
        .inspect_engine({
            let headers = headers.clone();
            let user = user.clone();
            move |engine, config| -> Result<_, EngineError> {
                if auth::identity(engine, config, &headers).as_deref() != Some(&user) {
                    return Err("Please log in.".into());
                }
                Ok((
                    engine
                        .active_account_id(&user)
                        .ok_or("Account unavailable.")?
                        .to_owned(),
                    engine.history_store().clone(),
                ))
            }
        })
        .await?;
    let _account = app
        .admission
        .http(account.clone())
        .map_err(EngineError::Unavailable)?;
    let identity = auth::Identity {
        headers,
        user,
        account,
        config: app.config.clone(),
    };
    for _ in 0..2 {
        let (plan, stamp) = capture(app, identity.clone(), input.clone()).await?;
        let ids = plan.ids();
        let text = match plan {
            Query::Read(plan) => {
                history.ensure_available_async().await?;
                plan.finish_async().await?
            }
            Query::Configs => app
                .config
                .display_with_history(history.stats_async().await?)
                .map_err(EngineError::Invalid)?,
        };
        let (current, current_stamp) = capture(app, identity.clone(), input.clone()).await?;
        if current_stamp == stamp && current.ids() == ids {
            return Ok((text, None));
        }
    }
    Err(EngineError::Conflict(
        "Command state changed; retry shortly.".into(),
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "query_tests.rs"]
mod tests;
