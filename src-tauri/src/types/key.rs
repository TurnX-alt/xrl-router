use serde::{Deserialize, Serialize};

/// Key health status (traffic light pattern)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyStatus {
    /// Healthy, actively used
    Green,
    /// Quota low (402), temporarily skipped
    Yellow,
    /// Auth failure (401), permanently invalid
    Red,
    /// Not yet validated
    Unknown,
}

impl KeyStatus {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "green" => Some(KeyStatus::Green),
            "yellow" => Some(KeyStatus::Yellow),
            "red" => Some(KeyStatus::Red),
            "unknown" => Some(KeyStatus::Unknown),
            _ => None,
        }
    }
}

impl std::fmt::Display for KeyStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyStatus::Green => write!(f, "green"),
            KeyStatus::Yellow => write!(f, "yellow"),
            KeyStatus::Red => write!(f, "red"),
            KeyStatus::Unknown => write!(f, "unknown"),
        }
    }
}

/// API key for a provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Key {
    /// UUID
    pub id: String,
    /// FK to Provider
    pub provider_id: String,
    /// Human-readable label
    pub name: String,
    /// SHA-256 hash of actual key
    pub key_hash: String,
    /// Masked key for display (e.g., "sk-xxxx...xxxx")
    pub key_masked: String,
    /// Health status
    pub status: KeyStatus,
    /// Last error message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Last error HTTP code
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error_code: Option<u16>,
    /// Last error timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error_time: Option<i64>,
    /// Last used timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<i64>,
    /// Current balance
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<f64>,
    /// Balance update timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance_updated_at: Option<i64>,
    /// Total requests made
    pub total_requests: u64,
    /// Total tokens consumed
    pub total_tokens: u64,
    /// Creation timestamp
    pub created_at: i64,
    /// Last update timestamp
    pub updated_at: i64,
}
