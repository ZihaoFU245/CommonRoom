use super::*;

pub fn sudo_command(input: &str) -> Result<Option<&str>, String> {
    let input = input.trim();
    if input.split_whitespace().next() != Some("/sudo") {
        return Ok(None);
    }
    let command = input
        .strip_prefix("/sudo")
        .ok_or("Invalid command.")?
        .trim_start();
    if input.chars().count() > 4100 || !command.starts_with('/') || command == "/" {
        return Err("Usage: /sudo /command [arguments]".into());
    }
    if command.split_whitespace().next() == Some("/sudo") {
        return Err("Nested /sudo commands are not supported.".into());
    }
    Ok(Some(command))
}

/// One object kind after `/new`, `/list`, or `/list member`.
///
/// The plural is accepted because `/list rooms` reads naturally, and every
/// spelling resolves to the same kind.
fn object_kind(word: &str) -> Option<&'static str> {
    match word {
        "room" | "rooms" => Some("room"),
        "user" | "users" | "account" | "accounts" => Some("user"),
        "agent" | "agents" => Some("agent"),
        "member" | "members" => Some("member"),
        _ => None,
    }
}

/// The alias accepting each agent setting, and whether its value is free text
/// that swallows the rest of the line.
fn agent_subcommand(word: &str) -> Option<(&'static str, bool)> {
    match word {
        "key" => Some(("/agent-key", false)),
        "reply" => Some(("/agent-reply", false)),
        "name" => Some(("/agent-name", false)),
        "prompt" => Some(("/agent-prompt", true)),
        "provider" => Some(("/agent-provider", false)),
        "url" => Some(("/agent-base-url", false)),
        "model" => Some(("/agent-model", false)),
        "search" => Some(("/agent-search", false)),
        "search-key" => Some(("/agent-search-key", false)),
        "sources" => Some(("/agent-sources", false)),
        "config" => Some(("/agent-config", false)),
        "remove" => Some(("/agent-remove", false)),
        _ => None,
    }
}

/// Words that name an agent but cannot also be a setting, so `remove`,
/// `owner`, and `-` may stand in for the target name.
fn agent_name_word(word: &str) -> bool {
    matches!(word, "remove" | "owner") || word == "-"
}

/// Rewrite the typed line into the canonical command the rest of the engine
/// already implements.
///
/// The canonical commands stay the authority for authorization: `/list user`
/// becomes `/users` and therefore needs the `/users` grant, and `/new user`
/// becomes `/user`, so no permission decision changes because of an alias.
/// An unrecognized form is left untouched, so an unknown command still reports
/// itself as unknown rather than as a bad type.
pub fn resolve_command(input: &str) -> Result<Option<String>, String> {
    let mut parts: Vec<&str> = input.split_whitespace().collect();
    if !parts.first().is_some_and(|word| word.starts_with('/')) {
        return Ok(None);
    }
    // A word before the first argument, so a bare `/list` keeps its own name.
    let kind = |parts: &[&str]| parts.get(1).and_then(|word| object_kind(word));
    match parts.first().copied() {
        Some("/new") => match kind(&parts) {
            // The kind word is the command, so it is dropped and the room name
            // becomes the first argument of the canonical room command.
            Some("room") => Ok(Some(
                std::iter::once("/new-room")
                    .chain(parts[2..].iter().copied())
                    .collect::<Vec<_>>()
                    .join(" "),
            )),
            Some("user") => Ok(Some(
                std::iter::once("/user")
                    .chain(parts[2..].iter().copied())
                    .collect::<Vec<_>>()
                    .join(" "),
            )),
            Some("agent") => Ok(Some(
                std::iter::once("/agent-create")
                    .chain(parts[2..].iter().copied())
                    .collect::<Vec<_>>()
                    .join(" "),
            )),
            Some(_) => parts
                .get(1)
                .map(|word| Err(format!("Unknown kind {word}. Use /new room|user|agent.")))
                .transpose(),
            None => Ok(None),
        },
        Some("/list" | "/ls" | "/show") => match kind(&parts) {
            // A bare `/list` keeps its own name so the handler can list the kinds.
            None if parts.len() == 1 => Ok(None),
            None => parts
                .get(1)
                .map(|word| {
                    Err(format!(
                        "Unknown kind {word}. Use /list room|user|agent|member."
                    ))
                })
                .transpose(),
            Some(kind) => {
                parts[0] = match kind {
                    "room" => "/rooms",
                    "user" => "/users",
                    "agent" => "/agent-list",
                    _ => "/members",
                };
                parts.remove(1);
                Ok(Some(parts.join(" ")))
            }
        },
        Some("/agent") => {
            let Some(word) = parts.get(1).copied() else {
                return Ok(None);
            };
            // The bare form takes an agent name first, so a leading word that
            // is not a setting is a name rather than an unknown setting.
            let (canonical, free_text) = match agent_subcommand(word) {
                Some(found) => found,
                None if agent_name_word(word) => ("/agent-remove", false),
                None => return Ok(None),
            };
            // `/list agent` and `/agent config` are the same read.
            let canonical = if canonical == "/agent-config" && parts.len() == 2 {
                "/agent-list"
            } else {
                canonical
            };
            if free_text {
                // The personality is the text between the setting and an
                // optional trailing agent name, so it may contain anything,
                // including newlines. It is taken from the original line rather
                // than from the tokenized parts, which would collapse it.
                let rest = input
                    .trim()
                    .split_once(char::is_whitespace)
                    .map(|(_, rest)| rest.trim_start())
                    .and_then(|rest| rest.split_once(char::is_whitespace))
                    .map(|(_, rest)| rest)
                    .unwrap_or_default();
                return Ok(Some(format!("{canonical} {rest}")));
            }
            let mut arguments = parts[2..].to_vec();
            if canonical == "/agent-remove" && word == "remove" {
                // `remove` is the verb, not the target, so it is only dropped
                // when it was the first argument rather than the agent's name.
                if arguments.first() == Some(&word) {
                    arguments.remove(0);
                }
            }
            let mut rewritten = canonical.to_string();
            for argument in arguments {
                rewritten.push(' ');
                rewritten.push_str(argument);
            }
            Ok(Some(rewritten))
        }
        Some(_) => Ok(None),
        None => Ok(None),
    }
}

/// The whitespace-separated parts of `input` when it starts with `command`,
/// with the command itself as the first part.
fn command_parts<'a>(input: &'a str, command: &'a str) -> Option<Vec<&'a str>> {
    let mut parts = input.split_whitespace();
    if parts.next() != Some(command) {
        return None;
    }
    let mut all = vec![command];
    all.extend(parts);
    Some(all)
}

pub(super) struct CommandContext<'a> {
    pub(super) actor: Option<&'a str>,
    pub(super) room: Option<&'a str>,
    pub(super) input: &'a str,
    pub(super) parts: &'a [&'a str],
    pub(super) view: Option<&'a str>,
    pub(super) scope: authorization::Scope,
    pub(super) author: &'a str,
}
impl Engine {
    /// Run one input and return its console text plus any agent work the caller
    /// must perform after releasing the engine lock.
    pub fn run(
        &mut self,
        actor: Option<&str>,
        room: Option<&str>,
        input: &str,
    ) -> Result<Execution, String> {
        self.agent_queue.clear();
        let reply = self.execute(actor, room, input)?;
        Ok(Execution {
            reply,
            agents: std::mem::take(&mut self.agent_queue),
        })
    }
    /// Run one input and return its console text.
    ///
    /// This is the single-phase entry point callers that never need agent work
    /// use, such as the console and the test suites. Anything that may trigger
    /// an agent reply goes through `run`, which also returns the queued jobs.
    pub fn execute(
        &mut self,
        actor: Option<&str>,
        room: Option<&str>,
        input: &str,
    ) -> Result<String, String> {
        // Aliases are rewritten before anything else reads the command name, so
        // a rewritten command is authorized exactly like the canonical one. The
        // rewrite is applied once, here, and never re-enters `execute`, so the
        // canonical name is not mistaken for something the caller typed.
        let rewritten = resolve_command(input)?;
        let input = rewritten.as_deref().unwrap_or(input);
        // `/new agent` creates an account, which no other command path does, so
        // it is handled here rather than in a handler that owns a command grant.
        // Creating an agent creates an account, so it needs x:account.create,
        // exactly like `/new user` does; the creation path checks that itself.
        if let Some(parts) = command_parts(input, "/agent-create") {
            let owner = actor.ok_or("Create agents from a signed-in account.")?;
            require_len(&parts, 3, "/new agent name api-key")?;
            let created = self.create_agent(owner, parts[1], parts[2]);
            if created.is_ok() && self.save().is_err() {
                return Err("Storage unavailable; the agent was not created.".into());
            }
            return created;
        }
        if matches!(input.trim(), "/list" | "/ls" | "/show") {
            return Err("Usage: /list room|user|agent|member".into());
        }
        if let Some(command) = sudo_command(input)? {
            return self.with_command_authority(actor, room, true, |engine| {
                engine.execute(actor, room, command)
            });
        }
        if input.split_whitespace().next() == Some("/rename") {
            let parts: Vec<_> = input.split_whitespace().collect();
            self.require_command(actor, room, "/rename")?;
            let (name, new_name) = match parts.as_slice() {
                [_, new_name] => (actor.ok_or("Usage: /rename user new-name")?, *new_name),
                [_, name, new_name] => (*name, *new_name),
                _ => return Err("Usage: /rename [user] new-name".into()),
            };
            return self.rename_user(actor, name, new_name);
        }
        if input.split_whitespace().next() == Some("/deleteuser") {
            let parts: Vec<_> = input.split_whitespace().collect();
            require_len(&parts, 2, "/deleteuser user")?;
            self.require_command(actor, room, "/deleteuser")?;
            let scope =
                self.account_target(actor, parts[1], authorization::Action::DeleteAccount)?;
            self.require_target_command(actor, &scope, "/deleteuser")?;
            return self.delete_user(actor, parts[1]);
        }
        if matches!(
            input.split_whitespace().next(),
            Some(
                "/permissions"
                    | "/debug"
                    | "/help"
                    | "/man"
                    | "/whoami"
                    | "/console"
                    | "/rooms"
                    | "/users"
                    | "/members"
                    | "/history"
            )
        ) {
            return self.apply(actor, room, input);
        }
        let before = self.data.clone();
        let result = self.apply(actor, room, input);
        match result {
            Ok(reply) => {
                if self.data == before {
                    return Ok(reply);
                }
                if self.data.policy.assignments != before.policy.assignments
                    || self.data.policy.grants != before.policy.grants
                {
                    self.data.policy.revision = before.policy.revision.wrapping_add(1);
                }
                if let Err(e) = self.save() {
                    self.data = before;
                    return Err(e);
                }
                Ok(reply)
            }
            Err(e) => {
                self.data = before;
                Err(e)
            }
        }
    }
    pub(super) fn apply(
        &mut self,
        actor: Option<&str>,
        room: Option<&str>,
        input: &str,
    ) -> Result<String, String> {
        let input = input.trim();
        if input.is_empty() || input.chars().count() > 4100 {
            return Err("Enter a message of up to 4000 characters or a command.".into());
        }
        if actor.is_some_and(|name| !self.active(name)) {
            return Err("Account unavailable.".into());
        }
        let parts: Vec<&str> = input.split_whitespace().collect();
        if !input.starts_with('/') {
            if input.chars().count() > 4000 {
                return Err("Messages support at most 4000 characters.".into());
            }
            let room = room.ok_or("Select a room first.")?;
            self.send_room(actor, room, input)?;
            return Ok(String::new());
        }
        // A flat agent command cannot arrive here: `resolve_command` turns
        // `/agent <setting>` into the canonical name before dispatch, and the
        // flat names stay in COMMANDS only so an existing grant keeps its
        // meaning. A rewritten name is therefore authorized like any other.
        self.require_command(actor, room, parts[0])?;
        let view = room;
        let scope = self.command_scope(actor, room)?;
        let author = actor.unwrap_or("console");
        let room = room.filter(|name| !name.starts_with('@'));
        let context = CommandContext {
            actor,
            room,
            input,
            parts: &parts,
            author,
            scope,
            view,
        };
        match parts[0] {
            "/enable" | "/disable" => self.apply_accounts(&context),
            "/grant" | "/revoke" | "/permissions" => self.apply_policy(&context),
            "/new" | "/new-room" | "/add" | "/kick" | "/join" | "/leave" | "/delete" | "/owner" => {
                self.apply_rooms(&context)
            }
            "/react" | "/reply" | "/retract" | "/clean" | "/tell" => self.apply_messages(&context),
            "/debug" | "/help" | "/man" | "/whoami" | "/console" | "/rooms" | "/users"
            | "/members" | "/history" => self.apply_queries(&context),
            "/agent" | "/agent-create" | "/agent-list" | "/agent-key" | "/agent-reply"
            | "/agent-name" | "/agent-remove" | "/agent-search" | "/agent-search-key"
            | "/agent-sources" | "/agent-prompt" | "/agent-provider" | "/agent-base-url"
            | "/agent-model" | "/agent-config" => self.apply_agents(&context),
            "/user" | "/reset" => Err("Account provisioning is console-only.".into()),
            _ => Err("Unknown command. Try /help.".into()),
        }
    }
}
