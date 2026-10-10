use super::*;

/// Where a message was posted, used to resolve agent work later.
pub(crate) enum AgentScope<'a> {
    Room(&'a str),
    Private(&'a str),
}

/// Usage text for the owner-scoped agent commands. The value comes first and
/// the agent name is optional, because an account usually owns one agent.
fn agent_usage(command: &str) -> &'static str {
    match command {
        "/agent-key" => "/agent-key api-key [agent-name]",
        "/agent-reply" => "/agent-reply [auto|mention] [agent-name]",
        "/agent-name" => "/agent-name new-name [agent-name]",
        "/agent-prompt" => "/agent-prompt <personality text> [agent-name]",
        "/agent-search" => "/agent-search [on|off] [agent-name]",
        "/agent-search-key" => "/agent-search-key search-api-key [agent-name]",
        "/agent-sources" => "/agent-sources [auto|always|never] [agent-name]",
        "/agent-provider" => "/agent-provider provider-name [agent-name]",
        "/agent-base-url" => "/agent-base-url https://host/path [agent-name]",
        "/agent-model" => "/agent-model model-id [agent-name]",
        "/agent-config" => "/agent-config [agent-name]",
        _ => "/agent-remove [agent-name]",
    }
}

/// Characters of personality text one agent may carry.
const MAX_PROMPT: usize = 4000;

/// Providers an agent may name by hand, with their chat-completions roots.
///
/// The registry lives in the engine so commands can validate a name without
/// reaching into the web layer, which owns the request itself.
pub const KNOWN_PROVIDERS: &[(&str, &str)] = &[
    ("deepseek", "https://api.deepseek.com"),
    ("openrouter", "https://openrouter.ai/api/v1"),
    ("openai", "https://api.openai.com/v1"),
    ("groq", "https://api.groq.com/openai/v1"),
    ("together", "https://api.together.xyz/v1"),
    ("mistral", "https://api.mistral.ai/v1"),
    ("xai", "https://api.x.ai/v1"),
];

/// Longest base URL or model id an agent may store.
const MAX_URL: usize = 200;
const MAX_MODEL: usize = 120;

/// Normalise an owner-supplied base URL to its root.
///
/// A plain root, a trailing slash, and the endpoint path itself all normalise
/// to the same value, because provider documentation shows different forms.
fn validate_base_url(value: &str) -> Result<String, String> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() {
        return Err("A base URL cannot be empty.".into());
    }
    if value.len() > MAX_URL {
        return Err(format!("A base URL holds at most {MAX_URL} bytes."));
    }
    if !value.starts_with("https://") {
        return Err("A provider base URL must start with https:// .".into());
    }
    let root = value
        .strip_suffix("/chat/completions")
        .or_else(|| value.strip_suffix("/responses"))
        .unwrap_or(value)
        .trim_end_matches('/');
    if root.strip_prefix("https://").is_none_or(str::is_empty) {
        return Err("A base URL must name a host.".into());
    }
    Ok(root.to_string())
}

/// Validate a model id. Provider ids contain letters, digits, and the
/// punctuation providers use, such as `z-ai/glm-4.6`.
fn validate_model(model: &str) -> Result<(), String> {
    let model = model.trim();
    if model.is_empty() {
        return Err("A model id cannot be empty.".into());
    }
    if model.len() > MAX_MODEL {
        return Err(format!("A model id holds at most {MAX_MODEL} bytes."));
    }
    if !model
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/' | b':' | b'@'))
    {
        return Err("A model id may contain letters, digits, - _ . / : and @ .".into());
    }
    Ok(())
}

/// How a provider turns thinking mode on or off.
///
/// DeepSeek and OpenRouter accept a `thinking` object; a model that supports
/// neither field fails the whole request when one is sent, so an owner can turn
/// the controls off for a gateway.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThinkingStyle {
    /// `{"thinking":{"type":"disabled"}}`, or `reasoning_effort` when on.
    #[default]
    Deepseek,
    /// Send nothing beyond the model and messages.
    None,
}

/// The model used when an agent names none.
pub const DEFAULT_MODEL: &str = "deepseek-flash";
/// The provider used when an agent names none.
const DEFAULT_PROVIDER: &str = KNOWN_PROVIDERS[0].1;

/// One resolved model provider.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provider {
    /// Name shown in command output and logs.
    pub name: String,
    /// Root URL, without a trailing slash or the endpoint path.
    pub base_url: String,
    pub model: String,
    pub thinking: ThinkingStyle,
}

/// The host of a URL, used to label a provider an owner supplied by URL.
fn host_of(url: &str) -> String {
    url.strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .filter(|host| !host.is_empty())
        .unwrap_or("custom")
        .to_string()
}

/// Resolve the provider one answer is requested from.
///
/// A stored base URL wins over the provider name, so an owner can point a
/// single agent at a gateway without naming it.
pub fn resolve_provider(provider: &str, base_url: &str, model: &str) -> Result<Provider, String> {
    let provider = provider.trim();
    let base_url = base_url.trim();
    let model = model.trim();
    let (name, base_url) = if !base_url.is_empty() {
        let root = validate_base_url(base_url)?;
        let name = if provider.is_empty() {
            host_of(&root)
        } else {
            provider.to_string()
        };
        (name, root)
    } else if !provider.is_empty() {
        let known = KNOWN_PROVIDERS
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(provider))
            .ok_or_else(|| {
                format!(
                    "Unknown provider {provider}. Known providers: {}.",
                    KNOWN_PROVIDERS
                        .iter()
                        .map(|(name, _)| *name)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        (known.0.to_string(), known.1.to_string())
    } else {
        (
            KNOWN_PROVIDERS[0].0.to_string(),
            DEFAULT_PROVIDER.to_string(),
        )
    };
    let model = if model.is_empty() {
        DEFAULT_MODEL.to_string()
    } else {
        validate_model(model)?;
        model.to_string()
    };
    Ok(Provider {
        name,
        base_url,
        model,
        thinking: ThinkingStyle::Deepseek,
    })
}

impl Provider {
    /// The endpoint one chat answer is posted to.
    pub fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }
    /// The auth header value. Every supported provider takes a bearer token.
    pub fn authorization(&self, key: &str) -> String {
        format!("Bearer {key}")
    }
    /// Apply the provider's thinking controls to a request body.
    ///
    /// A provider that has no such parameter is left untouched, because an
    /// unknown field can fail the whole request.
    pub fn apply_thinking(&self, body: &mut serde_json::Value, thinking: bool, effort: &str) {
        if self.thinking == ThinkingStyle::None {
            return;
        }
        if thinking {
            body["reasoning_effort"] = serde_json::json!(effort);
        } else {
            body["thinking"] = serde_json::json!({ "type": "disabled" });
        }
    }
    /// A one-line description for command output.
    pub fn describe(&self) -> String {
        format!("{} · {}", self.name, self.model)
    }
}

impl Engine {
    /// Active accounts that may hold room membership, including agents.
    pub fn usable(&self, name: &str) -> bool {
        self.data.users.get(name).is_some_and(|u| !u.disabled)
    }
    /// Directory label for an account.
    ///
    /// An agent is labelled from its account record; administrator authority is
    /// read from the policy, so a legacy `admin` field cannot claim it.
    pub fn role(&self, name: &str) -> &'static str {
        match self.data.users.get(name) {
            Some(account) if account.agent => "agent",
            Some(_) if self.is_admin(name) => "admin",
            Some(_) => "user",
            None => "unknown",
        }
    }
    pub fn is_agent(&self, name: &str) -> bool {
        self.data.users.get(name).is_some_and(Account::is_agent)
    }
    /// Whether an account owns at least one agent, which unlocks agent
    /// configuration hints. Ownership is read from the policy through the
    /// grant-based `owns_agent` in the authorization module.
    pub fn owns_any_agent(&self, actor: &str) -> bool {
        self.data
            .users
            .values()
            .any(|account| account.agent && self.owns_agent(Some(actor), &account.id))
    }
    /// Reserve an agent so one trigger produces exactly one answer.
    /// Returns false when a reply is already being generated for that agent.
    pub fn claim_agent(&mut self, name: &str) -> bool {
        self.agent_pending.insert(name.into())
    }
    pub fn release_agent(&mut self, name: &str) {
        self.agent_pending.remove(name);
    }
    /// Resolve the accounts that should answer a new message.
    ///
    /// `mention` mode needs an explicit mention; `auto` mode answers anything
    /// in a conversation the agent belongs to. Messages written by an agent
    /// never trigger another reply, so agents cannot answer each other
    /// endlessly.
    pub(super) fn agent_replies(
        &self,
        sender: &str,
        mentions: &BTreeSet<String>,
        audience: impl Iterator<Item = String>,
    ) -> Vec<String> {
        // An agent never answers another agent, including an explicit mention,
        // so two agents cannot hold an unbounded conversation.
        if self.is_agent(sender) {
            return Vec::new();
        }
        let mut names = Vec::new();
        for name in audience {
            if name == sender {
                continue;
            }
            let Some(account) = self.data.users.get(&name) else {
                continue;
            };
            if account.is_agent()
                && !account.disabled
                && !account.api_key.is_empty()
                && !self.agent_pending.contains(&name)
                && (mentions.contains(&name) || account.reply == AgentReply::Auto)
            {
                names.push(name);
            }
        }
        names
    }
    /// Queue provider work for the accounts that should answer this message.
    /// The caller runs it after releasing the engine lock.
    pub(super) fn trigger_agents(
        &mut self,
        sender: &str,
        scope: AgentScope<'_>,
        message: &Message,
    ) {
        let members = match scope {
            AgentScope::Room(room) => self
                .data
                .rooms
                .get(room)
                .map(|room| room.members.clone())
                .unwrap_or_default(),
            AgentScope::Private(peer) => BTreeSet::from([peer.to_string()]),
        };
        // The sender is excluded inside `agent_replies`, which also stops an
        // agent from answering another agent.
        let names = self.agent_replies(sender, &message.mentions, members.into_iter());
        if names.is_empty() {
            return;
        }
        let view = match scope {
            AgentScope::Room(room) => format!("room:{room}"),
            AgentScope::Private(peer) => format!("dm:{}", private_key(sender, peer)),
        };
        let context = self.agent_context(&view);
        for name in names {
            let account = self.data.users.get(&name);
            let api_key = account
                .map(|account| account.api_key.clone())
                .unwrap_or_default();
            // The search credential only travels when the agent may use it.
            let search = account.is_some_and(|account| account.search);
            let search_key = if search {
                account
                    .map(|account| account.search_key.clone())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            let sources = account.map(|account| account.sources).unwrap_or_default();
            let prompt = account
                .map(|account| account.prompt.clone())
                .unwrap_or_default();
            let provider = account
                .map(|account| account.provider.clone())
                .unwrap_or_default();
            let base_url = account
                .map(|account| account.base_url.clone())
                .unwrap_or_default();
            let model = account
                .map(|account| account.model.clone())
                .unwrap_or_default();
            self.agent_queue.push(AgentJob {
                name,
                api_key,
                search_key,
                search,
                sources,
                prompt,
                provider,
                base_url,
                model,
                view: view.clone(),
                context: context.clone(),
                trigger: message.id.clone(),
            });
        }
    }
    /// Conversation turns handed to an agent as context.
    fn agent_context(&self, view: &str) -> Vec<Message> {
        static EMPTY: VecDeque<Message> = VecDeque::new();
        let messages = view
            .strip_prefix("room:")
            .and_then(|room| self.data.rooms.get(room))
            .map(|room| &room.messages)
            .or_else(|| {
                view.strip_prefix("dm:")
                    .and_then(|key| self.data.private.get(key))
                    .map(|chat| &chat.messages)
            })
            .unwrap_or(&EMPTY);
        messages
            .iter()
            .skip(messages.len().saturating_sub(AGENT_CONTEXT))
            .filter(|message| !message.text.is_empty())
            .filter(|message| !is_agent_error(&message.text))
            .cloned()
            .collect()
    }
    pub(super) fn apply_agents(&mut self, context: &CommandContext<'_>) -> Result<String, String> {
        let CommandContext {
            actor,
            input,
            parts,
            ..
        } = *context;
        match parts[0] {
            "/agent" => {
                // Any signed-in account may create an agent and becomes its
                // owner; creation itself still needs x:account.create, checked
                // where the account is created.
                let owner = actor.ok_or("Create agents from a signed-in account.")?;
                // `/agent prompt <text>` is the same command as `/agent-prompt`.
                if parts.get(1) == Some(&"prompt") {
                    let (name, text) = self.prompt_target(owner, input, parts, 2)?;
                    self.agent_for_update(&name, owner)?;
                    return match text {
                        Some(text) => self.set_agent_prompt(&name, &text),
                        None => self.show_agent_prompt(&name),
                    };
                }
                require_len(parts, 3, "/agent name api-key")?;
                self.create_agent(owner, parts[1], parts[2])
            }
            "/agent-prompt" => {
                let actor = actor.ok_or("Agent configuration requires a user account.")?;
                let (name, text) = self.prompt_target(actor, input, parts, 1)?;
                self.agent_for_update(&name, actor)?;
                match text {
                    Some(text) => self.set_agent_prompt(&name, &text),
                    None => self.show_agent_prompt(&name),
                }
            }
            "/agent-key" | "/agent-reply" | "/agent-name" | "/agent-remove" | "/agent-search"
            | "/agent-search-key" | "/agent-sources" | "/agent-provider" | "/agent-base-url"
            | "/agent-model" | "/agent-config" => {
                let actor = actor.ok_or("Agent configuration requires a user account.")?;
                // The value comes first and the agent name is optional, because
                // an account usually owns exactly one agent. A name that is not
                // an agent is a usage or "not found" error, never a silent
                // fallback to the caller's own agent.
                let usage = agent_usage(parts[0]);
                let (target, value) = if parts[0] == "/agent-remove" {
                    match parts {
                        [_, target] => (Some(target), None),
                        [_] => (None, None),
                        _ => return Err(format!("Usage: {usage}")),
                    }
                } else {
                    // Commands with a value accept `value` or `value agent-name`.
                    match parts {
                        [_, value] => (None, Some(value)),
                        [_, value, target] => (Some(target), Some(value)),
                        _ => return Err(format!("Usage: {usage}")),
                    }
                };
                let name = match target {
                    Some(target) if self.is_agent(target) => target.to_string(),
                    Some(_) => return Err(format!("Usage: {usage}")),
                    None => self.owned_agent(actor)?,
                };
                self.agent_for_update(&name, actor)?;
                match value {
                    Some(value) => self.set_agent(parts[0], &name, value),
                    None => self.remove_agent(&name),
                }
            }
            _ => Err("Unknown command.".into()),
        }
    }
    /// Split the free text of `/agent-prompt` from its optional agent name.
    ///
    /// The prompt may contain spaces and newlines, so the target is only the
    /// final word, and only when that word names an agent. `skip` is the number
    /// of leading words the command itself used.
    fn prompt_target(
        &self,
        actor: &str,
        input: &str,
        parts: &[&str],
        skip: usize,
    ) -> Result<(String, Option<String>), String> {
        let head: Vec<&str> = parts.iter().take(skip).copied().collect();
        let prefix = head.join(" ");
        let text = input
            .strip_prefix(&prefix)
            .ok_or("Invalid command.")?
            .trim();
        if text.is_empty() {
            return Ok((self.owned_agent(actor)?, None));
        }
        // A bare agent name reads that agent instead of writing a personality
        // that happens to be its name. Agent names are unique accounts, so
        // this is unambiguous.
        if self.is_agent(text) {
            return Ok((text.to_string(), None));
        }
        let (text, target) = match text.rsplit_once(char::is_whitespace) {
            // A trailing agent name selects the target, so the last word only
            // counts as one when another agent already holds that name.
            Some((head, last)) if self.is_agent(last) && !head.trim_end().is_empty() => {
                (head.trim_end(), Some(last))
            }
            _ => (text, None),
        };
        let name = match target {
            Some(target) => target.to_string(),
            None => self.owned_agent(actor)?,
        };
        // `-` clears the personality back to the default behavior.
        if text == "-" {
            return Ok((name, Some(String::new())));
        }
        Ok((name, Some(text.to_string())))
    }
    fn set_agent_prompt(&mut self, name: &str, prompt: &str) -> Result<String, String> {
        let prompt = prompt.trim();
        if prompt.chars().count() > MAX_PROMPT {
            return Err(format!(
                "An agent prompt holds at most {MAX_PROMPT} characters."
            ));
        }
        self.data
            .users
            .get_mut(name)
            .ok_or("Agent not found.")?
            .prompt = prompt.into();
        Ok(if prompt.is_empty() {
            format!("Agent {name} personality cleared.")
        } else {
            format!(
                "Agent {name} personality updated ({} characters).",
                prompt.chars().count()
            )
        })
    }
    fn show_agent_prompt(&self, name: &str) -> Result<String, String> {
        let prompt = &self.data.users.get(name).ok_or("Agent not found.")?.prompt;
        Ok(if prompt.is_empty() {
            format!("Agent {name} has no personality yet. Set one with /agent-prompt <text>.")
        } else {
            format!("Agent {name} personality:\n{prompt}")
        })
    }
    /// Apply one configuration change to an agent the actor may change.
    fn set_agent(&mut self, command: &str, name: &str, value: &str) -> Result<String, String> {
        match command {
            "/agent-key" => self.set_agent_key(name, value),
            "/agent-search-key" => self.set_search_key(name, value),
            "/agent-reply" => {
                let mode = match value.to_ascii_lowercase().as_str() {
                    "auto" => AgentReply::Auto,
                    "mention" => AgentReply::Mention,
                    _ => return Err("Reply mode must be auto or mention.".into()),
                };
                self.set_agent_reply(name, mode)
            }
            "/agent-search" => {
                let on = match value.to_ascii_lowercase().as_str() {
                    "on" => true,
                    "off" => false,
                    _ => return Err("Web search must be on or off.".into()),
                };
                self.set_agent_search(name, on)
            }
            "/agent-sources" => {
                let mode = match value.to_ascii_lowercase().as_str() {
                    "auto" => SourceMode::Auto,
                    "always" => SourceMode::Always,
                    "never" => SourceMode::Never,
                    _ => return Err("Sources must be auto, always or never.".into()),
                };
                self.set_agent_sources(name, mode)
            }
            "/agent-provider" => {
                let value = value.trim();
                // `-` returns the agent to the built-in default provider.
                if value == "-" || value.eq_ignore_ascii_case("default") {
                    return self.set_agent_provider(name, "");
                }
                if value.starts_with("https://") {
                    return Err(
                        "Use /agent-base-url for a base URL, or a provider name such as openrouter."
                            .into(),
                    );
                }
                let known = KNOWN_PROVIDERS
                    .iter()
                    .find(|(known, _)| known.eq_ignore_ascii_case(value))
                    .map(|(known, _)| *known)
                    .ok_or_else(|| {
                        format!(
                            "Unknown provider {value}. Known providers: {}.",
                            KNOWN_PROVIDERS
                                .iter()
                                .map(|(name, _)| *name)
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    })?;
                self.set_agent_provider(name, known)
            }
            "/agent-base-url" => {
                let value = value.trim();
                if value == "-" || value.eq_ignore_ascii_case("default") {
                    return self.set_agent_base_url(name, "");
                }
                self.set_agent_base_url(name, &validate_base_url(value)?)
            }
            "/agent-model" => {
                let value = value.trim();
                if value == "-" || value.eq_ignore_ascii_case("default") {
                    return self.set_agent_model(name, "");
                }
                validate_model(value)?;
                self.set_agent_model(name, value)
            }
            "/agent-config" => self.show_agent_config(name),
            _ => self.rename_agent(name, value),
        }
    }
    /// The agent an account configures when the command names none.
    fn owned_agent(&self, actor: &str) -> Result<String, String> {
        if !self.usable(actor) {
            return Err("Account unavailable.".into());
        }
        if self.is_agent(actor) {
            return Ok(actor.into());
        }
        let mut owned = self
            .data
            .users
            .iter()
            .filter(|(_, candidate)| candidate.agent && self.owns_agent(Some(actor), &candidate.id))
            .map(|(name, _)| name.clone());
        let first = owned
            .next()
            .ok_or("Create an agent with /agent name api-key first.")?;
        if owned.next().is_some() {
            return Err(
                "You own several agents; name one, e.g. /agent-reply mention agent-name.".into(),
            );
        }
        Ok(first)
    }
    /// Create an agent owned by `owner`, with the least authority it needs.
    ///
    /// The dispatcher already checked the `/agent` command grant, and every
    /// signed-in account holds it, so any account may create an agent it owns.
    /// Creating one does not need `x:account.create`: an agent cannot
    /// authenticate, so it is not a login anyone can use. `max_users` bounds how
    /// many accounts exist, and the new agent gets read and write in its
    /// conversations and no commands at all.
    fn create_agent(&mut self, owner: &str, name: &str, api_key: &str) -> Result<String, String> {
        if !self.active(owner) {
            return Err("Account unavailable.".into());
        }
        if !valid_name(name) {
            return Err("Invalid agent name.".into());
        }
        if self.data.users.contains_key(name) {
            return Err("Name already exists.".into());
        }
        if self.data.users.len() >= self.max_users {
            return Err(format!(
                "Account limit reached ({}); it covers users and agents together.",
                self.max_users
            ));
        }
        validate_api_key(api_key)?;
        let agent_id = uuid::Uuid::new_v4().to_string();
        self.data.users.insert(
            name.into(),
            Account {
                // Agents never log in; an empty hash cannot match a password.
                hash: String::new(),
                id: agent_id.clone(),
                disabled: false,
                agent: true,
                api_key: api_key.into(),
                reply: AgentReply::Mention,
                search_key: String::new(),
                search: false,
                sources: SourceMode::default(),
                prompt: String::new(),
                provider: String::new(),
                base_url: String::new(),
                model: String::new(),
            },
        );
        let owner_id = self
            .data
            .users
            .get(owner)
            .ok_or("Account unavailable.")?
            .id
            .clone();
        self.add_agent_grants(owner_id, agent_id);
        self.audit(Some(owner), "/agent", name);
        // The policy changed, so the compiled masks must follow.
        self.rebuild_authorization()?;
        Ok(format!(
            "Agent {name} created with reply mode mention and read/write access only. Invite it with /add {name} room, then mention @{name}."
        ))
    }
    /// Verify the actor may configure `name`.
    ///
    /// Ownership is a policy grant, so this is the same decision `/permissions`
    /// reports, and an administrator passes through the admin group.
    fn agent_for_update(&self, name: &str, actor: &str) -> Result<(), String> {
        let account = self.data.users.get(name).ok_or("Agent not found.")?;
        if !account.agent {
            return Err("That account is not an agent.".into());
        }
        if self.is_admin(actor) || self.owns_agent(Some(actor), &account.id) {
            Ok(())
        } else {
            Err("Only the agent owner or an admin can change it.".into())
        }
    }
    fn set_agent_key(&mut self, name: &str, api_key: &str) -> Result<String, String> {
        validate_api_key(api_key)?;
        self.data
            .users
            .get_mut(name)
            .ok_or("Agent not found.")?
            .api_key = api_key.into();
        Ok(format!("Agent {name} key updated."))
    }
    fn set_search_key(&mut self, name: &str, search_key: &str) -> Result<String, String> {
        validate_api_key(search_key)?;
        let account = self.data.users.get_mut(name).ok_or("Agent not found.")?;
        account.search_key = search_key.into();
        Ok(format!(
            "Agent {name} search key updated; web search is {}.",
            if account.search { "on" } else { "off" }
        ))
    }
    fn set_agent_search(&mut self, name: &str, on: bool) -> Result<String, String> {
        let account = self.data.users.get_mut(name).ok_or("Agent not found.")?;
        if on && account.search_key.is_empty() {
            return Err(
                "Set a search key first, for example /agent-search-key tvly-dev-... .".into(),
            );
        }
        account.search = on;
        Ok(format!(
            "Agent {name} web search {}.",
            if on { "on" } else { "off" }
        ))
    }
    fn set_agent_sources(&mut self, name: &str, mode: SourceMode) -> Result<String, String> {
        self.data
            .users
            .get_mut(name)
            .ok_or("Agent not found.")?
            .sources = mode;
        Ok(format!(
            "Agent {name} sources: {}.",
            match mode {
                SourceMode::Auto => "the model decides",
                SourceMode::Always => "listed for every searched answer",
                SourceMode::Never => "never listed",
            }
        ))
    }
    fn set_agent_provider(&mut self, name: &str, provider: &str) -> Result<String, String> {
        self.data
            .users
            .get_mut(name)
            .ok_or("Agent not found.")?
            .provider = provider.into();
        Ok(format!(
            "Agent {name} provider: {}.",
            if provider.is_empty() {
                "the default".to_string()
            } else {
                provider.to_string()
            }
        ))
    }
    fn set_agent_base_url(&mut self, name: &str, base_url: &str) -> Result<String, String> {
        self.data
            .users
            .get_mut(name)
            .ok_or("Agent not found.")?
            .base_url = base_url.into();
        Ok(format!(
            "Agent {name} base URL: {}.",
            if base_url.is_empty() {
                "the provider default".to_string()
            } else {
                base_url.to_string()
            }
        ))
    }
    fn set_agent_model(&mut self, name: &str, model: &str) -> Result<String, String> {
        self.data
            .users
            .get_mut(name)
            .ok_or("Agent not found.")?
            .model = model.into();
        Ok(format!(
            "Agent {name} model: {}.",
            if model.is_empty() {
                "the provider default".to_string()
            } else {
                model.to_string()
            }
        ))
    }
    /// Everything one agent is configured with, without any credential.
    fn show_agent_config(&self, name: &str) -> Result<String, String> {
        let account = self.data.users.get(name).ok_or("Agent not found.")?;
        let provider = resolve_provider(&account.provider, &account.base_url, &account.model)?;
        // A provider name is the owner's own label, reported as written; only a
        // bare URL falls back to the host it points at.
        let label = if account.provider.is_empty() {
            provider.describe()
        } else {
            format!("{} · {}", account.provider, provider.model)
        };
        // An operator asking about an agent also needs to know who may change
        // it and what the agent itself is allowed to do.
        let access = self.allowance(Some(name), &crate::engine::authorization::Scope::Server);
        Ok(format!(
            "Agent {name}\n\
             owner: {}\n\
             provider: {label}\n\
             base URL: {}\n\
             model key: {}\n\
             reply: {}\n\
             personality: {}\n\
             web search: {} ({})\n\
             sources: {}\n\
             provider key: {}\n\
             search key: {}\n\
             access: {}",
            self.agent_owner_name(&account.id)
                .unwrap_or_else(|| "nobody".into()),
            provider.base_url,
            if account.model.is_empty() {
                "default"
            } else {
                "set"
            },
            match account.reply {
                AgentReply::Auto => "auto",
                AgentReply::Mention => "mention",
            },
            if account.prompt.is_empty() {
                "none".to_string()
            } else {
                format!("{} characters", account.prompt.chars().count())
            },
            if account.search { "on" } else { "off" },
            if account.search_key.is_empty() {
                "no key"
            } else {
                "key set"
            },
            match account.sources {
                SourceMode::Auto => "auto",
                SourceMode::Always => "always",
                SourceMode::Never => "never",
            },
            if account.api_key.is_empty() {
                "missing"
            } else {
                "set"
            },
            if account.search_key.is_empty() {
                "missing"
            } else {
                "set"
            },
            access.join(", ")
        ))
    }
    fn set_agent_reply(&mut self, name: &str, mode: AgentReply) -> Result<String, String> {
        self.data
            .users
            .get_mut(name)
            .ok_or("Agent not found.")?
            .reply = mode;
        Ok(format!(
            "Agent {name} replies to {}.",
            match mode {
                AgentReply::Auto => "every room message",
                AgentReply::Mention => "mentions only",
            }
        ))
    }
    fn rename_agent(&mut self, name: &str, new_name: &str) -> Result<String, String> {
        if !valid_name(new_name) {
            return Err("Invalid agent name.".into());
        }
        if self.data.users.contains_key(new_name) {
            return Err("Name already exists.".into());
        }
        let previous = self.data.clone();
        let Some(account) = self.data.users.remove(name) else {
            return Err("Agent not found.".into());
        };
        // Retained history keeps the messages, relabelled to the new name.
        for room in self.data.rooms.values_mut() {
            room.members.remove(name);
            room.members.insert(new_name.into());
            let mut changed = false;
            for message in &mut room.messages {
                changed |= relabel(message, name, new_name);
            }
            if changed {
                room.revision = room.revision.wrapping_add(1);
            }
        }
        // A rename changes the private-conversation key as well as the author
        // recorded inside each retained private message.
        let mut renamed = BTreeMap::new();
        for (key, mut chat) in std::mem::take(&mut self.data.private) {
            let mut changed = false;
            for message in &mut chat.messages {
                changed |= relabel(message, name, new_name);
            }
            if changed {
                chat.revision = chat.revision.wrapping_add(1);
            }
            let key = match private_peer(&key, name) {
                Some(peer) => private_key(new_name, peer),
                None => key,
            };
            renamed.insert(key, chat);
        }
        self.data.private = renamed;
        self.data.users.insert(new_name.into(), account);
        if let Err(error) = self.save() {
            self.data = previous;
            return Err(error);
        }
        Ok(format!("Agent {name} renamed to {new_name}."))
    }
    fn remove_agent(&mut self, name: &str) -> Result<String, String> {
        let previous = self.data.clone();
        let agent_id = self
            .data
            .users
            .get(name)
            .ok_or("Agent not found.")?
            .id
            .clone();
        if self.data.users.remove(name).is_none() {
            return Err("Agent not found.".into());
        }
        // The grants that described this agent go with it: the owner record on
        // its account scope, and the agent's own read/write grant.
        self.data.policy.grants.retain(|grant| {
            if grant.scope == crate::engine::authorization::Scope::Account(agent_id.clone()) {
                return false;
            }
            !matches!(&grant.subject, crate::engine::authorization::Subject::Account(id) if id == &agent_id)
        });
        self.data.policy.revision = self.data.policy.revision.wrapping_add(1);
        for room in self.data.rooms.values_mut() {
            room.members.remove(name);
        }
        for chat in self.data.private.values_mut() {
            for message in &mut chat.messages {
                if message.mentions.remove(name) {
                    chat.revision = chat.revision.wrapping_add(1);
                }
            }
        }
        for room in self.data.rooms.values_mut() {
            let mut changed = false;
            for message in &mut room.messages {
                changed |= message.mentions.remove(name);
            }
            if changed {
                room.revision = room.revision.wrapping_add(1);
            }
        }
        let removed = self
            .data
            .private
            .keys()
            .filter(|key| private_peer(key, name).is_some())
            .cloned()
            .collect::<BTreeSet<_>>();
        self.data.private.retain(|key, _| !removed.contains(key));
        for positions in self.read_positions.values_mut() {
            for key in &removed {
                positions.remove(&format!("dm:{key}"));
            }
        }
        if let Err(error) = self.save() {
            self.data = previous;
            return Err(error);
        }
        // The policy changed, so the compiled masks must follow.
        self.rebuild_authorization()?;
        Ok(format!("Agent {name} removed."))
    }
}

/// A failed provider call posts this marker in the conversation. Those messages
/// describe a server fault, so they are never handed back to a model as if the
/// agent had said them; otherwise the next answer reacts to its own failure.
pub const AGENT_ERROR_PREFIX: &str = "⚠";

fn is_agent_error(text: &str) -> bool {
    text.trim_start().starts_with(AGENT_ERROR_PREFIX)
}

/// Relabel one retained message written by `from`.
fn relabel(message: &mut Message, from: &str, to: &str) -> bool {
    let mut changed = false;
    if message.from == from {
        message.from = to.into();
        changed = true;
    }
    if let Some(reply) = &mut message.reply
        && reply.from == from
    {
        reply.from = to.into();
        changed = true;
    }
    if message.mentions.remove(from) {
        message.mentions.insert(to.into());
        changed = true;
    }
    // A reaction author lives inside the per-reaction user set, not as a key,
    // so every set has to be inspected.
    let reactions = std::mem::take(&mut message.reactions);
    let mut relabelled = BTreeMap::new();
    for (value, mut users) in reactions {
        if users.remove(from) {
            users.insert(to.into());
            changed = true;
        }
        relabelled.insert(value, users);
    }
    message.reactions = relabelled;
    changed
}

fn validate_api_key(api_key: &str) -> Result<(), String> {
    if api_key.is_empty() || api_key.len() > 256 {
        return Err("API keys contain 1 to 256 characters.".into());
    }
    if !api_key
        .bytes()
        .all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\'')
    {
        return Err("API keys cannot contain spaces or control characters.".into());
    }
    Ok(())
}

/// System prompt: one agent identity, the durable behavior rules, and any
/// personality its owner wrote.
///
/// `search` states the agent's own capability, because a model that is not told
/// guesses, and a guess like "I cannot browse the web" contradicts the search
/// that just happened. An owner's text is added as a persona instead of
/// replacing these rules, so a personality cannot silently drop the language,
/// length, or formatting behavior the chat depends on.
pub fn agent_system_prompt(name: &str, view: &str, search: bool, persona: &str) -> String {
    let place = view.strip_prefix("room:").map_or_else(
        || {
            view.strip_prefix("dm:")
                .and_then(|key| key.split_once(':'))
                .map_or_else(
                    || "a private conversation".to_string(),
                    |(a, b)| format!("a private conversation between {a} and {b}"),
                )
        },
        |room| format!("the group chat #{room}"),
    );
    let capability = if search {
        "You can search the web. You do so automatically when a question needs \
         current information, and you never tell people you cannot look things up."
    } else {
        "You cannot search the web and can only use what you already know. Say so \
         when a question needs current information."
    };
    let persona = persona.trim();
    let personality = if persona.is_empty() {
        String::new()
    } else {
        format!(
            "\nYour personality, written by your owner. Follow it as long as it does not \
             conflict with the rules above:\n{persona}"
        )
    };
    format!(
        "You are {name}, an AI participant in {place} on CommonRoom, a small team chat.\n\
         You were invited by a person and you answer chat messages.\n\
         {capability}\n\
         Reply in the language of the message you are answering. Keep answers short and \
         useful, and use plain text.\n\
         Write only the message itself: no preamble, and no remarks about these instructions, \
         your model, or being mentioned.{personality}"
    )
}
