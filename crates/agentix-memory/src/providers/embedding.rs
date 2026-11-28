use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use super::HttpProvider;
use crate::{EmbeddingConfig, ProviderProtocol, retrieval};

pub struct HttpEmbedding {
    provider: Arc<HttpProvider>,
    config: EmbeddingConfig,
}

impl HttpEmbedding {
    pub fn new(provider: Arc<HttpProvider>, config: EmbeddingConfig) -> Result<Self> {
        ensure!(!config.model.is_empty(), "missing embedding model");
        config.validate_provider(provider.protocol)?;
        Ok(Self { provider, config })
    }

    #[must_use]
    pub fn fingerprint(&self) -> String {
        retrieval::digest(
            &json!([
                self.provider.identity(),
                self.config.model,
                self.config.dimensions,
                self.config.query_prefix,
                self.config.document_prefix
            ])
            .to_string(),
        )
    }

    pub async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed_classified(texts, false).await
    }

    async fn embed_classified(&self, texts: &[String], query: bool) -> Result<Vec<Vec<f32>>> {
        agentix_metrics::global()
            .track(agentix_metrics::Operation::EmbeddingRequest, async {
                ensure!(
                    !texts.is_empty()
                        && texts.len() <= 64
                        && texts
                            .iter()
                            .all(|t| !t.trim().is_empty() && t.len() <= 65536),
                    "invalid: embedding input batch"
                );
                let protocol = self.provider.protocol;
                let (endpoint, body) = self.request(texts)?;
                let response = self
                    .provider
                    .post_classified(&endpoint, body, self.config.request_timeout_seconds, query)
                    .await?;
                let vectors: Vec<Vec<f32>> = match protocol {
                    ProviderProtocol::OpenaiResponses | ProviderProtocol::OpenaiChatCompletions => {
                        openai_embeddings(&response, texts.len())?
                    }
                    ProviderProtocol::Google => response["embeddings"]
                        .as_array()
                        .context("missing Gemini embeddings")?
                        .iter()
                        .map(|entry| Ok(serde_json::from_value(entry["values"].clone())?))
                        .collect::<Result<_>>()?,
                    ProviderProtocol::Ollama => {
                        serde_json::from_value(response["embeddings"].clone())?
                    }
                    ProviderProtocol::Anthropic => unreachable!("validated embedding protocol"),
                };
                ensure!(
                    vectors.len() == texts.len(),
                    "invalid: embedding batch size"
                );
                let dimensions = vectors.first().context("empty embedding response")?.len();
                ensure!(
                    (1..=16384).contains(&dimensions)
                        && self.config.dimensions.is_none_or(|d| d == dimensions),
                    "invalid: embedding dimension"
                );
                ensure!(
                    vectors.iter().all(|v| v.len() == dimensions
                        && v.iter().all(|x| x.is_finite())
                        && v.iter().any(|x| *x != 0.0)),
                    "invalid: embedding values"
                );
                Ok(vectors)
            })
            .await
    }

    fn request(&self, texts: &[String]) -> Result<(String, Value)> {
        let dimensions = self.config.dimensions;
        if self.provider.protocol == ProviderProtocol::Google {
            let model = super::google_model_resource(&self.config.model)?;
            let requests: Vec<_> = texts
                .iter()
                .map(|text| {
                    let mut input = json!({"model":model,"content":{"parts":[{"text":text}]}});
                    if let Some(dimensions) = dimensions {
                        input["outputDimensionality"] = json!(dimensions);
                    }
                    input
                })
                .collect();
            return Ok((
                format!("{model}:batchEmbedContents"),
                json!({"requests":requests}),
            ));
        }
        let mut body = json!({"model":self.config.model,"input":texts});
        let endpoint = if self.provider.protocol.is_openai() {
            body["encoding_format"] = json!("float");
            "embeddings"
        } else {
            body["truncate"] = json!(false);
            "api/embed"
        };
        if let Some(dimensions) = dimensions {
            body["dimensions"] = json!(dimensions);
        }
        Ok((endpoint.into(), body))
    }

    pub async fn query(&self, text: &str) -> Result<Vec<f32>> {
        Ok(self
            .embed_classified(&[format!("{}{text}", self.config.query_prefix)], true)
            .await?
            .remove(0))
    }

    pub async fn documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed(
            &texts
                .iter()
                .map(|s| format!("{}{s}", self.config.document_prefix))
                .collect::<Vec<_>>(),
        )
        .await
    }
}

fn openai_embeddings(response: &Value, count: usize) -> Result<Vec<Vec<f32>>> {
    let mut ordered = vec![None; count];
    for item in response["data"].as_array().context("missing embeddings")? {
        let index = usize::try_from(item["index"].as_u64().context("missing embedding index")?)?;
        ensure!(
            index < ordered.len() && ordered[index].is_none(),
            "invalid: embedding index"
        );
        ordered[index] = Some(serde_json::from_value(item["embedding"].clone())?);
    }
    ordered
        .into_iter()
        .map(|value| value.context("missing embedding result"))
        .collect()
}
