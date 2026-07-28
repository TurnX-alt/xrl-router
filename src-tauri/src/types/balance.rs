use serde::{Deserialize, Serialize};

/// Balance information for a provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceInfo {
    /// Whether the balance is valid
    pub is_valid: bool,
    /// Remaining balance
    pub remaining: f64,
    /// Currency unit (e.g., "USD", "CNY")
    pub unit: String,
    /// Provider name
    pub provider_name: String,
    /// Last update timestamp
    pub updated_at: i64,
}
