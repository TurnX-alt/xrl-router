//! 不同 LLM 协议格式之间的双向翻译（Anthropic ↔ OpenAI）。
//!
//! 按转换方向组织子模块；本文件仅做模块声明与 re-export，
//! 使外部 `translate::xxx` 调用路径保持不变。

pub mod common;
pub mod to_anthropic;
pub mod to_openai;

pub use common::estimate_input_tokens;
pub use to_anthropic::{
    finalize_openai_to_anthropic, openai_req_to_anthropic, translate_openai_chunk_to_anthropic,
    StreamState,
};
pub use to_openai::{
    anthropic_req_to_openai, extract_anthropic_usage, translate_anthropic_chunk_to_openai,
    OaStreamState,
};
