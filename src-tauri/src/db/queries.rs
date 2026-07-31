/// Prepared query helpers for common database operations.
/// These SQL strings are used throughout the application.

pub struct Queries;

impl Queries {
    // --- Providers ---
    pub const LIST_PROVIDERS: &str = "SELECT * FROM providers ORDER BY created_at DESC";
    pub const GET_PROVIDER_BY_ID: &str = "SELECT * FROM providers WHERE id = ?";
    pub const INSERT_PROVIDER: &str = "INSERT INTO providers (id, name, kind, base_url, api_path, enabled, config_json, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)";
    pub const UPDATE_PROVIDER: &str = "UPDATE providers SET name=?, kind=?, base_url=?, api_path=?, enabled=?, config_json=?, updated_at=? WHERE id=?";
    pub const DELETE_PROVIDER: &str = "DELETE FROM providers WHERE id = ?";

    // --- Models ---
    pub const LIST_MODELS: &str = "SELECT * FROM models ORDER BY tier, display_name";
    pub const LIST_MODELS_BY_PROVIDER: &str = "SELECT * FROM models WHERE provider_id = ? ORDER BY tier, display_name";
    pub const GET_MODEL_BY_ID: &str = "SELECT * FROM models WHERE id = ?";
    pub const GET_MODEL_BY_PROVIDER_AND_MODEL_ID: &str = "SELECT * FROM models WHERE provider_id = ? AND model_id = ?";
    pub const INSERT_MODEL: &str = "INSERT INTO models (id, provider_id, model_id, display_name, tier, context_window, max_output_tokens, capabilities, enabled, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
    pub const UPDATE_MODEL: &str = "UPDATE models SET display_name=?, tier=?, context_window=?, max_output_tokens=?, capabilities=?, enabled=?, updated_at=? WHERE id=?";
    pub const DELETE_MODEL: &str = "DELETE FROM models WHERE id = ?";

    // --- API Keys ---
    pub const LIST_KEYS: &str = "SELECT * FROM api_keys ORDER BY created_at DESC";
    pub const LIST_KEYS_BY_PROVIDER: &str = "SELECT * FROM api_keys WHERE provider_id = ? ORDER BY status, created_at";
    pub const GET_KEY_BY_ID: &str = "SELECT * FROM api_keys WHERE id = ?";
    pub const INSERT_KEY: &str = "INSERT INTO api_keys (id, provider_id, name, key_hash, key_masked, status, last_error, last_error_code, last_error_time, last_used_at, balance, balance_updated_at, total_requests, total_tokens, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
    pub const UPDATE_KEY_STATUS: &str = "UPDATE api_keys SET status=?, last_error=?, last_error_code=?, last_error_time=?, updated_at=? WHERE id=?";
    pub const UPDATE_KEY_BALANCE: &str = "UPDATE api_keys SET balance=?, balance_updated_at=?, updated_at=? WHERE id=?";
    pub const UPDATE_KEY_USAGE: &str = "UPDATE api_keys SET total_requests=total_requests+1, total_tokens=total_tokens+?, last_used_at=?, updated_at=? WHERE id=?";
    pub const DELETE_KEY: &str = "DELETE FROM api_keys WHERE id = ?";

    // --- Routes ---
    pub const LIST_ROUTES: &str = "SELECT * FROM routes ORDER BY priority ASC, weight DESC";
    pub const LIST_ROUTES_BY_MODEL: &str = "SELECT * FROM routes WHERE model_id = ? ORDER BY priority ASC";
    pub const GET_ROUTE_BY_ID: &str = "SELECT * FROM routes WHERE id = ?";
    pub const INSERT_ROUTE: &str = "INSERT INTO routes (id, name, model_id, provider_id, priority, weight, enabled, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)";
    pub const UPDATE_ROUTE: &str = "UPDATE routes SET name=?, priority=?, weight=?, enabled=?, updated_at=? WHERE id=?";
    pub const DELETE_ROUTE: &str = "DELETE FROM routes WHERE id = ?";

    // --- Usage Log ---
    pub const INSERT_USAGE_LOG: &str = "INSERT INTO usage_log (timestamp, provider_id, model_id, key_id, request_type, prompt_tokens, completion_tokens, latency_ms, success, error_message) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
    pub const GET_USAGE_SUMMARY: &str = "SELECT provider_id, model_id, SUM(prompt_tokens) as total_prompt, SUM(completion_tokens) as total_completion, COUNT(*) as request_count, AVG(latency_ms) as avg_latency FROM usage_log WHERE timestamp >= ? AND timestamp <= ? GROUP BY provider_id, model_id ORDER BY request_count DESC";

    // --- Dashboard ---
    pub const GET_PROVIDER_HEALTH: &str = "SELECT p.id, p.name, p.kind, p.enabled, COUNT(CASE WHEN k.status='green' THEN 1 END) as green_keys, COUNT(CASE WHEN k.status='yellow' THEN 1 END) as yellow_keys, COUNT(CASE WHEN k.status='red' THEN 1 END) as red_keys, COUNT(k.id) as total_keys FROM providers p LEFT JOIN api_keys k ON p.id = k.provider_id GROUP BY p.id";
    pub const GET_MODEL_COUNT: &str = "SELECT COUNT(*) as count FROM models WHERE enabled = 1";
    pub const GET_TOTAL_USAGE: &str = "SELECT COALESCE(SUM(prompt_tokens + completion_tokens), 0) as total_tokens, COUNT(*) as total_requests FROM usage_log";
}
