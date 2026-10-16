//! CommonRoom bot agent cell: a [`rig`] agent compiled to WebAssembly.
//!
//! The guest owns the agent loop (prompt assembly, tool dispatch, turn
//! accounting, conversation state). It owns no capability: every model call and
//! every tool call is an imported JavaScript function supplied by the Deno agent
//! host, which is where the sandboxed permissions are actually enforced.
//!
//! Imported functions, all in the default `env` wasm module so the Deno host can
//! satisfy them by merging its own imports into the instantiation object:
//!
//! - `host_llm_complete(requestJson)` -> `Promise<string>`
//! - `host_tool_call(requestJson)` -> `Promise<string>`
//! - `host_log(level, message)`
//!
//! Exported functions (called by `agents/deno/src/wasm.ts`):
//!
//! - `agent_configure(configJson)` -> config echo JSON
//! - `agent_answer(requestJson)` -> `Promise<string>`
//! - `agent_reset()`

use std::cell::RefCell;

use rig_agent::agent::Agent;
use rig_agent::core::message::Message;
use rig_agent::core::completion::{
    AssistantContent, CompletionError, CompletionModel, CompletionRequest, CompletionResponse,
    FinishReason, Usage,
};
use rig_agent::core::streaming::StreamingCompletionResponse;
use rig_agent::tool::{DynamicTool, ToolExecutionError, ToolOutput};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

/// Hard ceiling on one model completion, in bytes of JSON. The host applies its
/// own limit too; this one stops a runaway host from exhausting the guest heap.
const MAX_HOST_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn host_llm_complete(request: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch)]
    async fn host_tool_call(request: &str) -> Result<JsValue, JsValue>;

    fn host_log(level: &str, message: &str);
}

fn log(level: &str, message: &str) {
    host_log(level, message);
}

/// One model completion as the chat host asks for it. The field names are the
/// wire contract with `bindings.js`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HostCompletionRequest {
    model: String,
    request: CompletionRequest,
}

/// One tool invocation as the chat host asks for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HostToolRequest {
    name: String,
    arguments: serde_json::Value,
}

/// Token accounting as the host reports it. Every field is optional because
/// providers differ in what they report, and a missing counter must mean zero
/// rather than a decode failure.
#[derive(Debug, Clone, Default, Deserialize)]
struct HostUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    tool_use_prompt_tokens: u64,
    #[serde(default)]
    reasoning_tokens: u64,
}

impl From<HostUsage> for Usage {
    fn from(value: HostUsage) -> Self {
        Usage {
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            total_tokens: value.total_tokens,
            cached_input_tokens: value.cached_input_tokens,
            cache_creation_input_tokens: value.cache_creation_input_tokens,
            tool_use_prompt_tokens: value.tool_use_prompt_tokens,
            reasoning_tokens: value.reasoning_tokens,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct HostCompletionResponse {
    choice: Vec<AssistantContent>,
    #[serde(default)]
    usage: HostUsage,
    #[serde(default)]
    finish_reason: Option<FinishReason>,
}

#[derive(Debug, Clone, Deserialize)]
struct HostToolResponse {
    #[serde(default)]
    content: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<String>,
}

/// Configuration pushed by the host on every mention, so `/config set` and key
/// rotation apply immediately instead of only to a freshly started agent.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    /// Provider model identifier, for example `deepseek-chat`.
    pub model: String,
    /// System preamble.
    #[serde(default)]
    pub preamble: String,
    /// Default turn budget when a request does not carry one.
    #[serde(default = "default_max_turns")]
    pub max_turns: usize,
    /// Sampling temperature.
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Tool names this agent is allowed to use, as advertised to the model.
    #[serde(default)]
    pub tools: Vec<ToolBinding>,
}

/// A tool advertised to the model. `name` and `parameters` are forwarded to the
/// provider verbatim; execution is delegated to the host.
#[derive(Debug, Clone, Deserialize)]
pub struct ToolBinding {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

fn default_max_turns() -> usize {
    4
}

/// One mention handed to the guest.
#[derive(Debug, Clone, Deserialize)]
pub struct AnswerRequest {
    pub prompt: String,
    /// Retained conversation context, already shaped as rig messages by the
    /// host. Tool results are attached to the retained chat, not replayed here.
    #[serde(default)]
    pub history: Vec<Message>,
    #[serde(default)]
    pub max_turns: Option<usize>,
}

/// One answer returned by the guest.
#[derive(Debug, Clone, Serialize)]
pub struct AnswerResponse {
    pub text: String,
    /// Completed model turns in this run.
    pub turns: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

fn short(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let mut end = limit;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

fn decode_host_json(raw: &str, what: &str) -> Result<serde_json::Value, CompletionError> {
    if raw.len() > MAX_HOST_RESPONSE_BYTES {
        return Err(CompletionError::ResponseError(format!(
            "{what} response exceeded {MAX_HOST_RESPONSE_BYTES} bytes"
        )));
    }
    serde_json::from_str(raw).map_err(|error| {
        CompletionError::ResponseError(format!(
            "{what} response was not JSON ({error}): {}",
            short(raw, 200)
        ))
    })
}

fn js_error(name: &str, value: JsValue) -> CompletionError {
    let detail = value
        .as_string()
        .unwrap_or_else(|| format!("{value:?}"));
    CompletionError::ResponseError(format!("{name} failed: {detail}"))
}

/// A [`CompletionModel`] whose transport is the Deno host.
///
/// rig's built-in providers speak HTTP directly, which is not available (and,
/// for a sandboxed agent, not desirable) inside the guest. Delegating the
/// transport keeps rig's agent loop, tool dispatch and turn accounting while
/// leaving network access, allowlists and API keys in the host.
struct HostModel {
    model: String,
}

impl CompletionModel for HostModel {
    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse, CompletionError> {
        let payload = HostCompletionRequest {
            model: self.model.clone(),
            request,
        };
        let encoded = serde_json::to_string(&payload)?;
        let raw = host_llm_complete(&encoded)
            .await
            .map_err(|error| js_error("host_llm_complete", error))?;
        let raw = raw.as_string().ok_or_else(|| {
            CompletionError::ResponseError("host_llm_complete did not return a string".to_string())
        })?;
        let parsed: HostCompletionResponse = serde_json::from_value(decode_host_json(
            &raw,
            "host_llm_complete",
        )?)
        .map_err(|error| {
            CompletionError::ResponseError(format!("unexpected completion shape: {error}"))
        })?;
        if parsed.choice.is_empty() {
            return Err(CompletionError::ResponseError(
                "completion contained no content".to_string(),
            ));
        }
        Ok(CompletionResponse::new(parsed.choice, parsed.usage.into(), "commonroom-host")
            .with_optional_finish_reason(parsed.finish_reason))
    }

    async fn stream(
        &self,
        _request: CompletionRequest,
    ) -> Result<StreamingCompletionResponse, CompletionError> {
        // The chat host consumes one whole answer per turn; streaming has no
        // consumer here, and pretending otherwise would need a second bridge.
        Err(CompletionError::ProviderError(
            "streaming is not implemented by the CommonRoom host transport".to_string(),
        ))
    }
}

/// Build one host-backed tool. Tool execution is always a round trip to the
/// Deno host, which rechecks the agent's grants before doing any work.
fn host_tool(binding: ToolBinding) -> DynamicTool {
    let ToolBinding {
        name,
        description,
        parameters,
    } = binding;
    let requested = name.clone();
    DynamicTool::new(
        name,
        description,
        parameters,
        move |_context, arguments| {
            let name = requested.clone();
            Box::pin(async move {
                let request = HostToolRequest { name, arguments };
                let encoded = serde_json::to_string(&request).map_err(|error| {
                    ToolExecutionError::invalid_args(format!(
                        "could not encode tool request: {error}"
                    ))
                })?;
                let raw = host_tool_call(&encoded).await.map_err(|error| {
                    ToolExecutionError::invalid_args(format!(
                        "host tool bridge failed: {}",
                        error.as_string().unwrap_or_else(|| format!("{error:?}"))
                    ))
                })?;
                let raw = raw.as_string().ok_or_else(|| {
                    ToolExecutionError::invalid_args(
                        "host tool bridge did not return a string".to_string(),
                    )
                })?;
                let parsed: HostToolResponse =
                    serde_json::from_str(&raw).map_err(|error| {
                        ToolExecutionError::invalid_args(format!(
                            "host tool response was not JSON ({error}): {}",
                            short(&raw, 200)
                        ))
                    })?;
                if let Some(error) = parsed.error {
                    return Err(ToolExecutionError::invalid_args(error));
                }
                Ok(match parsed.content {
                    Some(value) => ToolOutput::json(value),
                    None => ToolOutput::text(""),
                })
            })
        },
    )
}

/// Live agent state. The host re-sends configuration on every mention, so this
/// is replaceable rather than write-once.
struct State {
    config: Option<AgentConfig>,
    /// `Agent` is not `Clone`; it is wrapped so a runner can be built from a
    /// shared handle without holding a `RefCell` borrow across an await.
    agent: Option<std::rc::Rc<Agent>>,
}

thread_local! {
    static STATE: RefCell<State> = const {
        RefCell::new(State {
            config: None,
            agent: None,
        })
    };
}

fn build_agent(config: &AgentConfig) -> Agent {
    let mut builder = rig_agent::AgentBuilder::new(HostModel {
        model: config.model.clone(),
    })
    .name("commonroom-agent")
    .default_max_turns(config.max_turns.max(1));

    if !config.preamble.trim().is_empty() {
        builder = builder.preamble(&config.preamble);
    }
    if let Some(temperature) = config.temperature {
        builder = builder.temperature(temperature);
    }

    // `dynamic_tools` consumes the whole list at once, which keeps the builder
    // chain intact instead of folding across changing tool-state types.
    let builder = builder.dynamic_tools(
        config
            .tools
            .iter()
            .cloned()
            .map(host_tool)
            .collect::<Vec<DynamicTool>>(),
    );

    builder.build()
}

/// Replaces the live agent, dropping any previous conversation state.
fn install(config: AgentConfig) {
    let agent = std::rc::Rc::new(build_agent(&config));
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.agent = Some(agent);
        state.config = Some(config);
    });
}

#[derive(Serialize)]
struct ConfigureAck {
    ok: bool,
    model: String,
    tools: Vec<String>,
    max_turns: usize,
}

/// Install or replace this cell's configuration. Safe to call repeatedly.
#[wasm_bindgen]
pub fn agent_configure(config_json: &str) -> Result<String, JsValue> {
    console_error_panic_hook::set_once();
    let config: AgentConfig = serde_json::from_str(config_json)
        .map_err(|error| JsValue::from_str(&format!("invalid agent configuration: {error}")))?;
    let ack = ConfigureAck {
        ok: true,
        model: config.model.clone(),
        tools: config.tools.iter().map(|tool| tool.name.clone()).collect(),
        max_turns: config.max_turns,
    };
    install(config);
    serde_json::to_string(&ack)
        .map_err(|error| JsValue::from_str(&format!("could not encode ack: {error}")))
}

/// Drop all conversation state, keeping the configuration.
#[wasm_bindgen]
pub fn agent_reset() {
    STATE.with(|state| {
        let state = state.borrow();
        if let Some(config) = state.config.clone() {
            drop(state);
            install(config);
        }
    });
}

/// Run one agent turn and return the reply text.
#[wasm_bindgen]
pub async fn agent_answer(request_json: &str) -> Result<String, JsValue> {
    console_error_panic_hook::set_once();
    let request: AnswerRequest = serde_json::from_str(request_json)
        .map_err(|error| JsValue::from_str(&format!("invalid answer request: {error}")))?;

    let max_turns = STATE.with(|state| {
        let state = state.borrow();
        request
            .max_turns
            .or_else(|| state.config.as_ref().map(|config| config.max_turns))
            .unwrap_or_else(default_max_turns)
            .max(1)
    });

    // Retained chat arrives as rig messages, so the model sees real roles and
    // the guest stays stateless across turns: the host owns durable history.
    let history = request.history.clone();

    // `AgentRunner` owns a clone of the agent's configuration and tool handle,
    // so the runner is built while the agent is borrowed and then awaited
    // outside the borrow. Awaiting inside `STATE.with` would hold a `RefCell`
    // borrow across a suspension point, and the borrow cannot escape `with`.
    let runner = STATE.with(|state| {
        let agent = state.borrow().agent.clone();
        agent.map(|agent| {
            let mut runner = agent
                .runner(request.prompt.clone())
                .max_turns(max_turns);
            if !history.is_empty() {
                runner = runner.history(history.clone());
            }
            runner
        })
    });
    let runner = runner.ok_or_else(|| {
        JsValue::from_str("agent_answer called before agent_configure")
    })?;

    let response = runner.run().await;

    match response {
        Ok(response) => {
            let answer = AnswerResponse {
                text: response.output,
                turns: response.completion_calls.len().max(1),
                input_tokens: response.usage.input_tokens,
                output_tokens: response.usage.output_tokens,
            };
            serde_json::to_string(&answer)
                .map_err(|error| JsValue::from_str(&format!("could not encode answer: {error}")))
        }
        Err(error) => {
            log("error", &format!("agent turn failed: {error}"));
            Err(JsValue::from_str(&format!("agent turn failed: {error}")))
        }
    }
}
