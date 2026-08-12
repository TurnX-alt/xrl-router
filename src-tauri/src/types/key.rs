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
