use crate::db::Database;
use crate::types::Model;
use anyhow::Result;
use dashmap::DashMap;
use tracing::info;

/// Model registry with tier-based indexing.
#[derive(Clone)]
pub struct ModelRegistry {
    database: Database,
    models: DashMap<String, Model>,
    by_tier: DashMap<String, Vec<String>>, // tier -> model ids
}

impl ModelRegistry {
    pub fn new(database: Database) -> Self {
        Self {
            database,
            models: DashMap::new(),
            by_tier: DashMap::new(),
        }
    }

    /// Load all models from database.
    pub fn load_from_db(&self) -> Result<()> {
        let conn = self.database.conn();
        let mut stmt = conn.prepare(
            "SELECT id, provider_id, model_id, display_name, tier,
                    context_window, max_output_tokens,
                    capabilities,
                    enabled, created_at, updated_at
             FROM models ORDER BY tier, display_name"
        )?;

        let models: Vec<Model> = stmt
            .query_map([], |row| {
                Ok(Model {
                    id: row.get(0)?,
                    provider_id: row.get(1)?,
                    model_id: row.get(2)?,
                    display_name: row.get(3)?,
                    tier: row.get(4)?,
                    context_window: row.get(5)?,
                    max_output_tokens: row.get(6)?,
                    capabilities: row.get(7)?,
                    enabled: row.get::<_, i32>(8)? != 0,
                    created_at: row.get(9)?,
                    updated_at: row.get(10)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        for m in models {
            let tier_key = m.tier.clone();
            self.by_tier
                .entry(tier_key)
                .or_insert_with(Vec::new)
                .push(m.id.clone());
            self.models.insert(m.id.clone(), m);
        }

        info!("Loaded {} models from database", self.models.len());
        Ok(())
    }

    /// Find a model by ID.
    #[allow(dead_code)]
    pub fn find_by_id(&self, id: &str) -> Option<Model> {
        self.models.get(id).map(|m| m.value().clone())
    }

    /// Get all enabled models.
    #[allow(dead_code)]
    pub fn get_enabled(&self) -> Vec<Model> {
        self.models
            .iter()
            .filter(|m| m.value().enabled)
            .map(|m| m.value().clone())
            .collect()
    }

    /// Get models by tier.
    #[allow(dead_code)]
    pub fn get_by_tier(&self, tier: &str) -> Vec<Model> {
        self.by_tier
            .get(tier)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.find_by_id(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get model count.
    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// Check if registry is empty.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }
}
