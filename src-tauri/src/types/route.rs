use serde::{Deserialize, Serialize};

/// Route definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Route {
    /// UUID
    pub id: String,
    /// Display name
    pub name: String,
    /// FK to Model
    pub model_id: String,
    /// FK to Provider
    pub provider_id: String,
    /// Priority (lower = higher priority)
    pub priority: u32,
    /// Weight for distribution (0.0-1.0)
    pub weight: f64,
    /// Whether route is enabled
    pub enabled: bool,
    /// Creation timestamp
    pub created_at: i64,
    /// Last update timestamp
    pub updated_at: i64,
}
