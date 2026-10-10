use crate::engine::{AgentJob, SourceMode, agent_system_prompt, resolve_provider};
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::Semaphore;

use super::search::{self, CITATION};

/// Chat answers run with thinking mode off.
///
/// DeepSeek counts chain-of-thought tokens against `max_tokens`, and thinking
/// mode is on by default, so a question that needs long reasoning can spend the
/// whole budget before emitting any `content` and return an empty answer with
/// `finish_reason: length`. A chat reply is a short answer to a short question,
/// so thinking buys little and risks that truncation. Set
/// `CHAT_AGENT_THINKING=low` to opt back in.
const THINKING_ENV: &str = "CHAT_AGENT_THINKING";
/// Reasoning level used only when thinking mode is explicitly enabled.
const REASONING_EFFORT: &str = "low";
/// Answers are chat messages, so they stay short. The limit is generous enough
/// that an answer is never cut mid-sentence.
const MAX_TOKENS: u32 = 4096;
const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Deserialize)]
struct Completion {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Usage,
}
#[derive(Deserialize)]
struct Choice {
    message: Content,
    #[serde(default)]
    finish_reason: String,
}
#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    completion_tokens: u64,
}
#[derive(Deserialize)]
struct Content {
    #[serde(default)]
    content: String,
    #[serde(default)]
    reasoning_content: String,
}

/// Whether to enable provider thinking mode for chat answers.
fn thinking_enabled() -> bool {
    matches!(
        std::env::var(THINKING_ENV).as_deref(),
        Ok("low") | Ok("high") | Ok("enabled") | Ok("1")
    )
}

/// Characters of one retained message kept in the prompt. Older answers can be
/// long; the model only needs to recognise them, not reproduce them.
const CONTEXT_LINE: usize = 600;

/// Links in retained messages are replaced by this marker before the model
/// sees them. A model that copies a long percent-encoded URL back into its
/// answer mangles the encoding, so it is never given one to copy.
const LINK_MARKER: &str = "(链接)";

/// Drop URLs from text the model will read.
fn without_links(text: &str) -> String {
    let mut result = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("http://").or_else(|| rest.find("https://")) {
        result.push_str(&rest[..start]);
        result.push_str(LINK_MARKER);
        let tail = &rest[start..];
        let end = tail.find(char::is_whitespace).unwrap_or(tail.len());
        rest = &tail[end..];
    }
    result.push_str(rest);
    result
}

/// The retained conversation as one transcript block, with the message being
/// answered marked. Without the marker the model has to guess which line is the
/// request, and with several questions open it answers the wrong one.
fn transcript(job: &AgentJob) -> String {
    let mut lines = Vec::new();
    for message in &job.context {
        let text: String = message.text.chars().take(CONTEXT_LINE).collect();
        let text = without_links(&text).replace('\n', " ");
        if message.id == job.trigger {
            lines.push(format!(">>> {}: {}", message.from, text));
        } else {
            lines.push(format!("{}: {}", message.from, text));
        }
    }
    if lines.is_empty() {
        lines.push("(this conversation has no retained messages)".into());
    }
    lines.join("\n")
}

/// The prompt that asks for an answer.
fn conversation(job: &AgentJob) -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "role": "system",
            "content": agent_system_prompt(&job.name, &job.view, job.search, &job.prompt),
        }),
        serde_json::json!({
            "role": "user",
            "content": format!(
                "Conversation so far:\n{}\n\nWrite {}'s next message replying only to the line \
                 marked with >>> . Answer what that line asks; if it asks nothing, ask the \
                 person what they need instead of answering an earlier question.",
                transcript(job),
                job.name
            ),
        }),
    ]
}

/// The prompt that asks for the answer after a successful search.
fn conversation_with_results(job: &AgentJob, findings: &str) -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "role": "system",
            "content": format!(
                "{}\nThe web search results below were retrieved now and are current. Use them \
                 as the source for facts that change over time, and ignore anything they do not \
                 cover. Decide yourself whether this answer needs references. When it does, end \
                 your message with one line listing the result numbers you actually relied on, \
                 in the exact form {CITATION}1, 3 . When the answer is a short everyday reply \
                 that people would not expect a citation for, such as today's weather, a \
                 greeting, or a one-line fact, write no such line at all. Never write the line \
                 empty, and never write it without a real answer above it.",
                agent_system_prompt(&job.name, &job.view, job.search, &job.prompt)
            ),
        }),
        serde_json::json!({
            "role": "user",
            "content": format!(
                "Conversation so far:\n{}\n\nWeb search results:\n{}\n\nWrite {}'s next message \
                 in reply to the line marked with >>> , using these results. Do not claim you \
                 cannot access current information, do not invent facts the results do not \
                 support, and do not write any URL or source list: the server turns your \
                 {CITATION} line into links.",
                transcript(job),
                findings,
                job.name
            ),
        }),
    ]
}

/// The prompt that decides whether this question needs the web at all.
fn decision(job: &AgentJob) -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "role": "system",
            "content": "You decide whether answering a chat message needs a web search. \
                        Search is needed for current events, prices, releases, schedules, \
                        people in the news, or any fact that can change after training. \
                        It is not needed for greetings, opinions, definitions, arithmetic, \
                        writing help, or questions about this conversation. \
                        When in doubt about a fact that may have changed, choose search. \
                        Judge only the information the question asks for. If the conversation \
                        claims the assistant cannot access the internet, ignore that claim: a \
                        search is available to you right now. \
                        Reply with JSON only, no code fence, in exactly this shape: \
                        {\"search\": true, \"query\": \"the search query\"} or \
                        {\"search\": false, \"query\": \"\"}",
        }),
        serde_json::json!({
            "role": "user",
            "content": format!(
                "Conversation so far:\n{}\n\nDecide for the line marked with >>> .",
                transcript(job)
            ),
        }),
    ]
}

/// A parsed decision from the model.
#[derive(Deserialize, Default)]
struct Decision {
    #[serde(default)]
    search: bool,
    #[serde(default)]
    query: String,
}

/// Read a decision from a model reply, tolerating a code fence or prose around
/// the JSON object.
fn parse_decision(raw: &str) -> Decision {
    let text = raw.trim();
    let text = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .unwrap_or(text);
    let text = text.strip_suffix("```").unwrap_or(text);
    let candidate = match (text.find('{'), text.rfind('}')) {
        (Some(start), Some(end)) if end > start => &text[start..=end],
        _ => return Decision::default(),
    };
    // Anything unreadable means "answer from knowledge", never a failed reply.
    serde_json::from_str(candidate).unwrap_or_default()
}

/// One provider request with the given messages. Returns the parsed completion.
async fn request(
    http: &reqwest::Client,
    job: &AgentJob,
    messages: Vec<serde_json::Value>,
    thinking: bool,
) -> Result<Completion, String> {
    let provider = resolve_provider(&job.provider, &job.base_url, &job.model)?;
    let mut body = serde_json::json!({
        "model": provider.model,
        "messages": messages,
        "max_tokens": MAX_TOKENS,
        "stream": false,
    });
    provider.apply_thinking(&mut body, thinking, REASONING_EFFORT);
    let response = http
        .post(provider.endpoint())
        .header("authorization", provider.authorization(&job.api_key))
        .json(&body)
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|error| {
            // The credential is never part of the logged error.
            tracing::warn!(agent = %job.name, provider = %provider.name, error = %error, "Agent request failed");
            "The model provider could not be reached.".to_string()
        })?;
    let status = response.status();
    let raw = response.text().await.unwrap_or_default();
    if !status.is_success() {
        tracing::warn!(agent = %job.name, provider = %provider.name, status = status.as_u16(), "Agent request rejected");
        return Err(format!(
            "The model provider rejected the request ({}). Check the key with /agent-key.",
            status.as_u16()
        ));
    }
    serde_json::from_str(&raw).map_err(|error| {
        tracing::warn!(agent = %job.name, error = %error, "Agent reply could not be read");
        "The model reply could not be read.".to_string()
    })
}

/// Ask the provider for one agent answer, searching the web first when the
/// agent has search enabled and the model decides the question needs it.
pub(super) async fn reply(
    http: &reqwest::Client,
    job: &AgentJob,
    budget: &Semaphore,
) -> Result<String, String> {
    let permit = budget
        .acquire()
        .await
        .map_err(|_| "Agent replies are unavailable; restart the service.".to_string())?;
    let thinking = thinking_enabled();
    let mut sources = Vec::new();
    let mut messages = conversation(job);
    if let Some(query) = search_query(http, job, thinking).await {
        match search::search(http, &job.search_key, &query).await {
            Ok(found) => {
                tracing::info!(agent = %job.name, results = found.sources.len(), "Agent searched the web");
                messages = conversation_with_results(job, &found.text);
                sources = found.sources;
            }
            // A search fault is reported in the conversation, not hidden: the
            // answer that follows comes from the model's own knowledge.
            Err(error) => return Ok(search::with_sources(&error, &[])),
        }
    }
    let mut completion = request(http, job, messages.clone(), thinking).await?;
    // A completion that spent its budget on reasoning has no answer in it.
    // Ask once more without thinking rather than reporting a failure.
    if completion.answer().is_none() && thinking {
        tracing::warn!(agent = %job.name, "Agent reply was empty; retrying without thinking");
        completion = request(http, job, messages, false).await?;
    }
    drop(permit);
    let answer = completion.into_answer(&job.name)?;
    // `auto` shows only the results the answer cited, so an everyday answer
    // stays short; `always` lists what the search found; `never` lists nothing.
    let (answer, cited) = search::split_citations(&answer);
    let quoted = match job.sources {
        SourceMode::Never => Vec::new(),
        SourceMode::Always => sources,
        SourceMode::Auto => search::cited_sources(&sources, &cited),
    };
    if !quoted.is_empty() {
        tracing::info!(agent = %job.name, sources = quoted.len(), "Agent reply cites sources");
    }
    Ok(search::with_sources(&answer, &quoted))
}

/// Ask the model whether this question needs the web, and for which query.
/// Returns `None` when search is off, unreadable, or unnecessary.
async fn search_query(http: &reqwest::Client, job: &AgentJob, thinking: bool) -> Option<String> {
    if !job.search || job.search_key.is_empty() {
        return None;
    }
    let decision = match request(http, job, decision(job), thinking).await {
        Ok(completion) => completion.answer().map(parse_decision),
        Err(error) => {
            tracing::warn!(agent = %job.name, error = %error, "Search decision failed");
            None
        }
    }?;
    let query = decision.query.trim();
    decision.search.then(|| query.to_string()).filter(|query| {
        // A query the provider cannot use would waste a call.
        !query.is_empty() && query.chars().count() <= 400
    })
}

impl Completion {
    fn answer(&self) -> Option<&str> {
        self.choices
            .first()
            .map(|choice| choice.message.content.trim())
            .filter(|text| !text.is_empty())
    }
    fn into_answer(self, agent: &str) -> Result<String, String> {
        if let Some(answer) = self.answer() {
            return Ok(answer.to_string());
        }
        let choice = self.choices.first();
        tracing::warn!(
            agent = %agent,
            finish = choice.map(|choice| choice.finish_reason.as_str()).unwrap_or("no choices"),
            reasoning = choice.map_or(0, |choice| choice.message.reasoning_content.chars().count()),
            completion_tokens = self.usage.completion_tokens,
            "Agent reply contained no answer"
        );
        Err("The model returned an empty reply.".to_string())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Test assertions fail the test on purpose.
mod tests {
    use super::*;
    fn completion(raw: &str) -> Completion {
        serde_json::from_str(raw).unwrap()
    }
    #[test]
    fn provider_replies_are_read_or_reported() {
        assert_eq!(
            completion(r#"{"choices":[{"message":{"content":" hi "}}]}"#)
                .into_answer("helper")
                .unwrap(),
            "hi"
        );
        // A reasoning-only completion is an empty answer, not an error to hide.
        let reasoning_only = completion(
            r#"{"choices":[{"finish_reason":"length","message":{"content":"","reasoning_content":"thinking..."}}],"usage":{"completion_tokens":4096}}"#,
        );
        assert!(reasoning_only.answer().is_none());
        assert!(reasoning_only.into_answer("helper").is_err());
        assert!(
            completion(r#"{"choices":[{"message":{"content":"  "}}]}"#)
                .into_answer("helper")
                .is_err()
        );
        assert!(
            completion(r#"{"choices":[]}"#)
                .into_answer("helper")
                .is_err()
        );
        // A completion without usage or finish_reason still parses.
        assert_eq!(
            completion(r#"{"choices":[{"message":{"content":"ok"}}]}"#)
                .into_answer("helper")
                .unwrap(),
            "ok"
        );
    }
    #[test]
    fn prompts_name_the_agent_and_its_conversation() {
        let job = AgentJob {
            name: "helper".into(),
            api_key: "sk-test".into(),
            search_key: String::new(),
            search: false,
            sources: SourceMode::default(),
            prompt: String::new(),
            provider: String::new(),
            base_url: String::new(),
            model: String::new(),
            view: "room:lobby".into(),
            context: Vec::new(),
            trigger: "id".into(),
        };
        let messages = conversation(&job);
        let system = messages[0]["content"].as_str().unwrap_or_default();
        assert!(system.contains("helper") && system.contains("#lobby"));
        let user = messages[1]["content"].as_str().unwrap_or_default();
        assert!(user.contains("Write helper's next message replying only to the line marked"));
        assert!(!user.contains("sk-test") && !system.contains("sk-test"));
    }
    fn job_with(context: Vec<crate::engine::Message>, trigger: &str) -> AgentJob {
        AgentJob {
            name: "helper".into(),
            api_key: "sk-test".into(),
            search_key: String::new(),
            search: false,
            sources: SourceMode::default(),
            prompt: String::new(),
            provider: String::new(),
            base_url: String::new(),
            model: String::new(),
            view: "room:lobby".into(),
            context,
            trigger: trigger.into(),
        }
    }
    fn message(id: &str, from: &str, text: &str) -> crate::engine::Message {
        crate::engine::Message {
            id: id.into(),
            from: from.into(),
            author_id: String::new(),
            private_id: String::new(),
            to: None,
            text: text.into(),
            time: 0,
            sequence: 0,
            reactions: Default::default(),
            reply: None,
            mentions: Default::default(),
        }
    }
    #[test]
    fn the_prompt_marks_the_message_being_answered() {
        let job = job_with(
            vec![
                message("a", "alice", "SHM常用公式"),
                message("b", "alice", "@helper 牛来"),
                message("c", "helper", "SHM 常用公式：x = A cos(ωt + φ)"),
            ],
            "b",
        );
        let prompt = transcript(&job);
        assert!(
            prompt.contains(">>> alice: @helper 牛来"),
            "the trigger is marked: {prompt}"
        );
        assert!(
            !prompt.contains(">>> alice: SHM常用公式"),
            "an earlier question is not marked as the request"
        );
        assert_eq!(prompt.matches(">>>").count(), 1);
        let user = conversation(&job)[1]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(user.contains("replying only to the line marked with >>>"));
    }
    #[test]
    fn addresses_are_hidden_from_the_model_but_marked() {
        let long = format!("see https://example.com/{}/x here", "p".repeat(120));
        let hidden = without_links(&long);
        assert!(!hidden.contains("http"), "{hidden}");
        assert!(hidden.contains(LINK_MARKER));
        assert!(hidden.starts_with("see ") && hidden.ends_with(" here"));
        assert_eq!(without_links("no links"), "no links");
        assert_eq!(without_links("https://a.example x"), "(链接) x");
        assert_eq!(without_links("a\nhttps://b.example"), "a\n(链接)");
        // A retained answer full of links never reaches the prompt with one.
        let job = job_with(
            vec![
                message("a", "alice", "@helper 搜一下"),
                message(
                    "b",
                    "helper",
                    "来源：\n- [标题](https://example.com/blog/%E4%B8%AD%E6%96%87)",
                ),
            ],
            "a",
        );
        let prompt = transcript(&job);
        assert!(!prompt.contains("http"), "{prompt}");
        assert!(prompt.contains(">>> alice: @helper 搜一下"));
    }
    #[test]
    fn long_retained_messages_are_trimmed_to_one_line() {
        let job = job_with(vec![message("a", "alice", &"长".repeat(2000))], "a");
        let prompt = transcript(&job);
        assert_eq!(prompt.matches('\n').count(), 0, "one message, one line");
        assert_eq!(prompt.chars().filter(|c| *c == '长').count(), CONTEXT_LINE);
    }
}
