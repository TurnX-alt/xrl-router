use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Unified chat request (compatible with OpenAI and Anthropic protocols)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    /// Anthropic-specific: system prompt
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<SystemPrompt>,
    /// Anthropic-specific: thinking configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
}

/// System prompt (can be plain text or structured blocks)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SystemPrompt {
    Text(String),
    Blocks(Vec<SystemBlock>),
}

/// System prompt block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemBlock {
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

/// Cache control configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub type_: String, // "ephemeral"
}

/// Thinking configuration (Anthropic)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingConfig {
    #[serde(rename = "type")]
    pub type_: String, // "enabled" or "disabled"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<usize>,
}

/// Chat message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String, // "system", "user", "assistant", "tool"
    pub content: MessageContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Message content (can be plain text or structured parts)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

/// Content part (text, image, tool_use, tool_result)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentPart {
    #[serde(rename = "type")]
    pub type_: String, // "text", "image_url", "tool_use", "tool_result"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<ImageUrl>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use: Option<ToolUse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ToolResult>,
}

/// Image URL
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
    #[serde(default = "default_detail")]
    pub detail: String,
}

fn default_detail() -> String {
    "auto".to_string()
}

/// Tool use (Anthropic)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUse {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// Tool result (Anthropic)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_use_id: String,
    pub content: String,
}

/// Tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    #[serde(rename = "type")]
    pub type_: Option<String>, // "function" or server tool type
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Value,
}

/// Tool call (OpenAI)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub type_: String, // "function"
    pub function: ToolCallFunction,
}

/// Tool call function
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String, // JSON string
}

/// Tool choice mode
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolChoice {
    Auto,
    Any,
    None,
    Tool(ToolChoiceTool),
}

/// Specific tool choice
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolChoiceTool {
    #[serde(rename = "type")]
    pub type_: String,
    pub function: ToolChoiceFunction,
}

/// Tool choice function
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolChoiceFunction {
    pub name: String,
}

/// Unified chat response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
    /// Anthropic-specific: stop reason
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// Streaming event type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_event: Option<StreamEvent>,
}

/// Response choice
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub index: usize,
    pub message: ResponseMessage,
    pub finish_reason: String,
}

/// Response message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

/// Token usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<usize>,
}

/// Streaming event (Anthropic)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StreamEvent {
    #[serde(rename = "message_start")]
    MessageStart(StreamMessageStart),
    #[serde(rename = "content_block_start")]
    ContentBlockStart(StreamContentBlockStart),
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta(StreamContentBlockDelta),
    #[serde(rename = "content_block_stop")]
    ContentBlockStop(StreamContentBlockStop),
    #[serde(rename = "message_delta")]
    MessageDelta(StreamMessageDelta),
    #[serde(rename = "message_stop")]
    MessageStop,
    #[serde(rename = "error")]
    Err(StreamError),
}

/// Stream message start
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamMessageStart {
    pub message: StreamMessage,
}

/// Stream message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamMessage {
    pub id: String,
    pub model: String,
    #[serde(default = "default_role")]
    pub role: String,
}

fn default_role() -> String {
    "assistant".to_string()
}

/// Stream content block start
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamContentBlockStart {
    pub index: usize,
    pub content_block: StreamContentBlock,
}

/// Stream content block
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StreamContentBlock {
    Text(StreamTextBlock),
    ToolUse(StreamToolUseBlock),
    Thinking(StreamThinkingBlock),
}

/// Stream text block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamTextBlock {
    pub text: String,
}

/// Stream tool use block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamToolUseBlock {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// Stream thinking block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamThinkingBlock {
    pub thinking: String,
}

/// Stream content block delta
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamContentBlockDelta {
    pub index: usize,
    pub delta: StreamDelta,
}

/// Stream delta
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StreamDelta {
    TextDelta(StreamTextDelta),
    InputJsonDelta(StreamInputJsonDelta),
    ThinkingDelta(StreamThinkingDelta),
}

/// Stream text delta
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamTextDelta {
    pub text: String,
}

/// Stream input JSON delta
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamInputJsonDelta {
    pub partial_json: String,
}

/// Stream thinking delta
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamThinkingDelta {
    pub thinking: String,
}

/// Stream content block stop
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamContentBlockStop {
    pub index: usize,
}

/// Stream message delta
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamMessageDelta {
    pub stop_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// Stream error
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamError {
    pub message: String,
}
