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
        ensure!(
            !texts.is_empty()
                && texts.len() <= 64
                && texts
                    .iter()
                    .all(|t| !t.trim().is_empty() && t.len() <= 65536),
            "invalid: embedding input batch"
        );
        let mut body = json!({"model":self.config.model,"input":texts});
        let endpoint = if self.provider.protocol == ProviderProtocol::Openai {
            body["encoding_format"] = json!("float");
            "embeddings"
        } else {
            body["truncate"] = json!(false);
            "api/embed"
        };
        if let Some(dimensions) = self.config.dimensions {
            body["dimensions"] = json!(dimensions);
        }
        let response = self
            .provider
            .post(endpoint, body, self.config.request_timeout_seconds)
            .await?;
        let vectors: Vec<Vec<f32>> = if self.provider.protocol == ProviderProtocol::Openai {
            let mut ordered = vec![None; texts.len()];
            for item in response["data"].as_array().context("missing embeddings")? {
                let index =
                    usize::try_from(item["index"].as_u64().context("missing embedding index")?)?;
                ensure!(
                    index < ordered.len() && ordered[index].is_none(),
                    "invalid: embedding index"
                );
                ordered[index] = Some(serde_json::from_value::<Vec<f32>>(
                    item["embedding"].clone(),
                )?);
            }
            ordered
                .into_iter()
                .map(|v| v.context("missing embedding result"))
                .collect::<Result<_>>()?
        } else {
            serde_json::from_value(response.get("embeddings").cloned().unwrap_or(Value::Null))?
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
    }

    pub async fn query(&self, text: &str) -> Result<Vec<f32>> {
        Ok(self
            .embed(&[format!("{}{text}", self.config.query_prefix)])
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
