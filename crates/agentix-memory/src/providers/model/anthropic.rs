use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use super::{AgentConfig, ModelReply, ModelRequest, TokenUsage, native_call};

pub(super) fn body(config: &AgentConfig, request: &ModelRequest) -> Result<Value> {
    let messages = super::native_history(
        &request.history,
        |text| json!({"role":"user","content":text}),
        |content| json!({"role":"assistant","content":content}),
        |_, results| {
            let blocks: Vec<_> = results
                .iter()
                .map(|(id, output)| json!({"type":"tool_result","tool_use_id":id,"content":output}))
                .collect();
            Ok(json!({"role":"user","content":blocks}))
        },
    )?;
    let mut body = json!({
        "model":config.model, "max_tokens":config.max_output_tokens,
        "system":request.instructions, "messages":messages
    });
    if !request.tools.is_empty() {
        let tools: Vec<_> = request.tools.iter().map(|tool| {
            json!({"name":tool.name,"description":tool.description,"input_schema":tool.parameters})
        }).collect();
        body["tools"] = json!(tools);
        // Some Claude models require auto tool choice even when thinking is disabled.
    }
    Ok(body)
}

pub(super) fn reply(value: &Value, usage: TokenUsage) -> Result<ModelReply> {
    ensure!(
        matches!(
            value["stop_reason"].as_str(),
            Some("end_turn" | "tool_use" | "stop_sequence")
        ),
        "Anthropic response is incomplete or failed"
    );
    let blocks = value["content"]
        .as_array()
        .context("missing Anthropic content")?;
    let mut calls = Vec::new();
    let mut text = String::new();
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => {
                text.push_str(block["text"].as_str().context("invalid Anthropic text")?);
            }
            Some("tool_use") => calls.push(native_call(
                block["id"].as_str(),
                &block["name"],
                block["input"].clone(),
            )?),
            _ => {}
        }
    }
    super::ensure_unique_calls(&calls)?;
    Ok(ModelReply {
        continuation: value["content"].clone(),
        calls,
        text,
        usage,
    })
}

pub(super) fn usage(value: &Value) -> TokenUsage {
    let usage = &value["usage"];
    TokenUsage {
        input_tokens: usage["input_tokens"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(usage["cache_creation_input_tokens"].as_u64().unwrap_or(0))
            .saturating_add(usage["cache_read_input_tokens"].as_u64().unwrap_or(0)),
        output_tokens: usage["output_tokens"].as_u64().unwrap_or(0),
    }
}
