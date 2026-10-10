use super::auth::token;
use super::*;
use crate::engine::{
    AGENT_ERROR_PREFIX, AgentJob,
    authorization::{Action, Scope},
    hash_password, sudo_command, verify_password,
};
use axum::http::HeaderMap;
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct Input {
    pub(super) id: u64,
    pub(super) room: Option<String>,
    pub(super) text: String,
}
/// Run one input and start the agent replies it triggered. Starting the work
/// here keeps the engine lock released while a provider is called.
pub(super) async fn run_input(
    app: &App,
    headers: &HeaderMap,
    user: &str,
    input: &Input,
) -> Result<Outcome, String> {
    let elevated = sudo_command(&input.text)?;
    let text = elevated.unwrap_or(&input.text);
    let sudo = elevated.is_some();
    let parts: Vec<_> = text.split_whitespace().collect();
    if sudo && matches!(parts.first(), Some(&"/clear" | &"/logout")) {
        return app.engine()?.with_command_authority(
            Some(user),
            input.room.as_deref(),
            true,
            |_| {
                if parts.len() != 1 {
                    return Err(format!("Usage: {}", parts[0]));
                }
                Ok(Outcome::unchanged("Done.".into()))
            },
        );
    }
    if parts.first() == Some(&"/configs") {
        let mut engine = app.engine()?;
        return engine.with_command_authority(Some(user), input.room.as_deref(), sudo, |engine| {
            engine.require_command(Some(user), input.room.as_deref(), "/configs")?;
            engine.require_target_command(Some(user), &Scope::Server, "/configs")?;
            engine.require(Some(user), &Scope::Server, Action::Config)?;
            if parts.len() != 1 {
                return Err("Usage: /configs".into());
            }
            app.config.display().map(Outcome::unchanged)
        });
    }
    let passwd = parts.first() == Some(&"/passwd");
    if !passwd && !matches!(parts.first(), Some(&"/user") | Some(&"/reset")) {
        // The session is rechecked here because the token was read before the
        // lock, and a command may be running while another device signs out.
        // Agent work is taken out of the outcome so the provider calls run after
        // the engine lock is released.
        let mut outcome = {
            let mut engine = app.engine()?;
            if token(headers)
                .and_then(|token| engine.session(token))
                .as_deref()
                != Some(user)
            {
                return Err("Please log in.".into());
            }
            let before = engine.revision;
            let execution = engine.run(Some(user), input.room.as_deref(), &input.text)?;
            Outcome {
                reply: execution.reply,
                changed: engine.revision != before,
                agents: execution.agents,
            }
        };
        if !outcome.agents.is_empty() {
            let jobs = std::mem::take(&mut outcome.agents);
            spawn_agent_replies(app.clone(), jobs);
        }
        return Ok(outcome);
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
        let mut engine = app.engine()?;
        engine.with_command_authority(Some(user), input.room.as_deref(), sudo, |engine| {
            engine.require_command(Some(user), input.room.as_deref(), parts[0])?;
            if passwd {
                let scope = engine.account_target(Some(user), user, Action::Password)?;
                engine.require_target_command(Some(user), &scope, parts[0])?;
                Ok(None)
            } else if reset {
                let scope = engine.account_target(Some(user), parts[1], Action::Reset)?;
                engine.require_target_command(Some(user), &scope, parts[0])?;
                Ok(Some(
                    engine
                        .data
                        .users
                        .get(parts[1])
                        .ok_or("User not found.")?
                        .id
                        .clone(),
                ))
            } else {
                engine.require_target_command(Some(user), &Scope::Server, parts[0])?;
                engine.require(Some(user), &Scope::Server, Action::CreateAccount)?;
                if parts.get(3) == Some(&"admin") {
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
                .map(Outcome::changed);
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
            .map(Outcome::changed)
    })
}

/// One completed input, ready for acknowledgement and agent follow-up.
pub(super) struct Outcome {
    reply: String,
    changed: bool,
    agents: Vec<AgentJob>,
}
impl Outcome {
    /// Whether this input changed persisted state. Used by the test suite.
    #[cfg(test)]
    pub(super) fn changed_state(&self) -> bool {
        self.changed
    }
    /// Local command output for the requesting socket.
    pub(super) fn notice(&self, id: u64) -> serde_json::Value {
        json!({ "kind": "notice", "id": id, "text": self.reply })
    }
    /// Broadcast state changes only after a persisted mutation.
    pub(super) fn broadcast(&self, app: &App) {
        if self.changed {
            let _ = app.changes.send(Change::All);
        }
    }
    fn changed(reply: String) -> Self {
        Self {
            reply,
            changed: true,
            agents: Vec::new(),
        }
    }
    /// The console text. Used by the test suite.
    #[cfg(test)]
    pub(super) fn reply(self) -> String {
        self.reply
    }
    fn unchanged(reply: String) -> Self {
        Self {
            reply,
            changed: false,
            agents: Vec::new(),
        }
    }
}

/// Local error output for the requesting socket.
pub(super) fn error_reply(id: u64, text: &str) -> serde_json::Value {
    json!({ "kind": "error", "id": id, "text": text })
}

/// Generate each agent answer off the request path, then publish it.
fn spawn_agent_replies(app: App, jobs: Vec<AgentJob>) {
    for job in jobs {
        let app = app.clone();
        tokio::spawn(async move {
            {
                // One trigger produces one answer: skip a duplicate claim.
                let Ok(mut engine) = app.engine() else {
                    return;
                };
                if !engine.claim_agent(&job.name) {
                    return;
                }
            }
            let text = match agent::reply(&app.http, &job, &app.agent_jobs).await {
                Ok(text) => text,
                // The marker identifies a server fault so the engine can keep
                // it out of the model's conversation context.
                Err(error) => format!("{AGENT_ERROR_PREFIX} {error}"),
            };
            let published = app
                .engine()
                .and_then(|mut engine| engine.insert_agent_reply(&job.name, &job.view, &text));
            match published {
                Ok(_) => {
                    let _ = app.changes.send(Change::All);
                }
                Err(error) => {
                    tracing::warn!(agent = %job.name, error = %error, "Agent reply was not stored");
                }
            }
            if let Ok(mut engine) = app.engine() {
                engine.release_agent(&job.name);
            }
        });
    }
}
