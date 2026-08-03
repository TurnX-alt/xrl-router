use super::adapter::Adapter;
use async_trait::async_trait;
use futures::stream::{Stream, StreamExt};
use reqwest::Client;
use std::pin::Pin;

pub struct AnthropicAdapter {
    client: Client,
    base_url: String,
    api_key: String,
}

impl AnthropicAdapter {
    pub fn new(base_url: String, api_key: String) -> Self {
        Self {
            client: crate::http::http_client(),
            base_url,
            api_key,
        }
    }
}

#[async_trait]
impl Adapter for AnthropicAdapter {
    async fn chat(&self, body: &str) -> Result<String, anyhow::Error> {
        let url = format!("{}/v1/messages", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
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
        let url = format!("{}/v1/messages", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
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
        let url = format!("{}/v1/messages", self.base_url);
        self.client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }
}
