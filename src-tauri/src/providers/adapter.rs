use async_trait::async_trait;
use futures::stream::Stream;
use std::pin::Pin;

/// Adapter trait for provider-specific implementations.
#[async_trait]
pub trait Adapter: Send + Sync {
    /// Send a non-streaming chat request.
    async fn chat(&self, body: &str) -> Result<String, anyhow::Error>;

    /// Send a streaming chat request, returning an async stream of SSE chunks.
    async fn chat_stream(
        &self,
        body: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, anyhow::Error>;

    /// Check provider health.
    async fn health_check(&self) -> bool;
}
