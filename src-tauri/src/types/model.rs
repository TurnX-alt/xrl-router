use serde::{Deserialize, Serialize};

/// Model intelligence tier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelTier {
    /// Top-tier intelligence (Claude Opus 5, GPT-5, Gemini Ultra)
    Fable,
    /// High performance (Claude Opus 4.x, GPT-4o)
    Opus,
    /// Balanced (Claude Sonnet, GPT-4o-mini, Qwen3.7-Plus)
    Sonnet,
    /// Lightweight & fast (Claude Haiku, GPT-4o-nano, local models)
    Haiku,
    /// User-defined tier
    Custom,
}

impl ModelTier {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "fable" => Some(ModelTier::Fable),
            "opus" => Some(ModelTier::Opus),
            "sonnet" => Some(ModelTier::Sonnet),
            "haiku" => Some(ModelTier::Haiku),
            "custom" => Some(ModelTier::Custom),
            _ => None,
        }
    }
}

impl std::fmt::Display for ModelTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelTier::Fable => write!(f, "fable"),
            ModelTier::Opus => write!(f, "opus"),
            ModelTier::Sonnet => write!(f, "sonnet"),
            ModelTier::Haiku => write!(f, "haiku"),
            ModelTier::Custom => write!(f, "custom"),
        }
    }
}

/// Model capability
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Capability {
    Text,
    Tools,
    Thinking,
    Vision,
    Streaming,
    Audio,
}

impl Capability {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "text" => Some(Capability::Text),
            "tools" => Some(Capability::Tools),
            "thinking" => Some(Capability::Thinking),
            "vision" => Some(Capability::Vision),
            "streaming" => Some(Capability::Streaming),
            "audio" => Some(Capability::Audio),
            _ => None,
        }
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Capability::Text => write!(f, "text"),
            Capability::Tools => write!(f, "tools"),
            Capability::Thinking => write!(f, "thinking"),
            Capability::Vision => write!(f, "vision"),
            Capability::Streaming => write!(f, "streaming"),
            Capability::Audio => write!(f, "audio"),
        }
    }
}

/// Model definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    /// Internal UUID
    pub id: String,
    /// FK to Provider
    pub provider_id: String,
    /// Model identifier (e.g., "gpt-4o", "claude-opus-4-8")
    pub model_id: String,
    /// Display name (e.g., "GPT-4o", "Claude Opus 4.8")
    pub display_name: String,
    /// Intelligence tier
    pub tier: ModelTier,
    /// Owner (e.g., "openai", "anthropic", "dingtalk")
    pub owned_by: String,
    /// Supported capabilities
    pub capabilities: Vec<Capability>,
    /// Max context tokens
    pub context_window: usize,
    /// Max output tokens
    pub max_output_tokens: usize,
    /// Pricing for cost tracking (per 1k input tokens)
    pub cost_per_1k_input: f64,
    /// Pricing for cost tracking (per 1k output tokens)
    pub cost_per_1k_output: f64,
    /// Whether model is enabled
    pub enabled: bool,
    /// Creation timestamp
    pub created_at: i64,
    /// Last update timestamp
    pub updated_at: i64,
}

impl Model {
    /// Check if a model has a specific capability
    pub fn has_capability(&self, cap: Capability) -> bool {
        self.capabilities.contains(&cap)
    }
}
