mod embedding;
mod http;
mod limits;
mod model;
pub(crate) mod pass;
pub use embedding::HttpEmbedding;
pub use http::HttpProvider;
pub(crate) use http::ProviderHttpError;
pub use limits::ProviderLimits;
pub use model::{
    HttpModel, Message, Model, ModelReply, ModelRequest, TokenUsage, ToolCall, ToolDefinition,
};

pub(crate) fn google_model_resource(model: &str) -> anyhow::Result<String> {
    let name = model.strip_prefix("models/").unwrap_or(model);
    anyhow::ensure!(
        name.as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        "invalid: Gemini model name"
    );
    Ok(format!("models/{name}"))
}
