use super::adapter::Adapter;
use async_trait::async_trait;
use futures::stream::{Stream, StreamExt};
use reqwest::Client;
use std::pin::Pin;

pub struct OpenAIAdapter {
    client: Client,
    base_url: String,
    api_key: String,
}

impl OpenAIAdapter {
    pub fn new(base_url: String, api_key: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
            api_key,
        }
    }
}

#[async_trait]
impl Adapter for OpenAIAdapter {
    async fn chat(&self, body: &str) -> Result<String, anyhow::Error> {
        let url = if self.base_url.ends_with("/chat/completions") {
            self.base_url.clone()
        } else {
            format!("{}/chat/completions", self.base_url)
        };

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await?;

        Ok(response.text().await?)
    }

    async fn chat_stream(
        &self,
        body: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, anyhow::Error> {
        let url = format!("{}/chat/completions", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await?;

        let stream = response
            .bytes_stream()
            .filter_map(|chunk| async move {
                match chunk {
                    Ok(bytes) => Some(String::from_utf8_lossy(&bytes).to_string()),
                    Err(_) => None,
                }
            })
            .boxed();

        Ok(stream)
    }

    async fn health_check(&self) -> bool {
        let url = format!("{}/models", self.base_url);
        self.client
            .get(&url)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }
}
