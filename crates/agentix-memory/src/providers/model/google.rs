use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use super::{AgentConfig, ModelReply, ModelRequest, TokenUsage, ToolCall, native_call};

pub(super) fn body(config: &AgentConfig, request: &ModelRequest) -> Result<Value> {
    let contents = super::native_history(
        &request.history,
        |text| json!({"role":"user","parts":[{"text":text}]}),
        Value::clone,
        tool_results,
    )?;
    let mut body = json!({
        "systemInstruction":{"parts":[{"text":request.instructions}]},
        "contents":contents, "generationConfig":{"maxOutputTokens":config.max_output_tokens}
    });
    if !request.tools.is_empty() {
        let declarations: Vec<_> = request.tools.iter().map(|tool| {
            json!({"name":tool.name,"description":tool.description,"parametersJsonSchema":tool.parameters})
        }).collect();
        body["tools"] = json!([{"functionDeclarations":declarations}]);
        body["toolConfig"] = json!({"functionCallingConfig":{"mode":"ANY"}});
    }
    Ok(body)
}

fn tool_results(content: &Value, results: &[(&str, &str)]) -> Result<Value> {
    let calls = calls(content)?;
    // Keep the results in the original call order, including older Gemini models without call IDs.
    ensure!(
        calls.len() == results.len(),
        "incomplete Gemini tool results"
    );
    let mut parts = Vec::with_capacity(calls.len());
    for ((call, native_id), (id, output)) in calls.into_iter().zip(results) {
        ensure!(
            call.id == *id,
            "Gemini tool result does not match the preceding call"
        );
        let output = serde_json::from_str(output).unwrap_or_else(|_| json!(output));
        let mut response = json!({"name":call.name,"response":{"output":output}});
        if let Some(id) = native_id {
            response["id"] = json!(id);
        }
        parts.push(json!({"functionResponse":response}));
    }
    Ok(json!({"role":"user","parts":parts}))
}

fn calls(content: &Value) -> Result<Vec<(ToolCall, Option<&str>)>> {
    let parts = content["parts"]
        .as_array()
        .context("missing Gemini parts")?;
    let mut calls = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for (index, part) in parts.iter().enumerate() {
        if let Some(function) = part.get("functionCall") {
            let native_id = function["id"].as_str();
            let fallback = format!("gemini-part-{index}");
            let args = function.get("args").cloned().unwrap_or_else(|| json!({}));
            let call = native_call(native_id.or(Some(&fallback)), &function["name"], args)?;
            ensure!(ids.insert(call.id.clone()), "duplicate model tool call ID");
            calls.push((call, native_id));
        }
    }
    Ok(calls)
}

pub(super) fn reply(value: &Value, usage: TokenUsage) -> Result<ModelReply> {
    let candidate = value["candidates"]
        .as_array()
        .and_then(|items| items.first())
        .context("missing Gemini candidate")?;
    ensure!(
        candidate["finishReason"] == "STOP",
        "Gemini response is incomplete or failed"
    );
    let content = &candidate["content"];
    let calls = calls(content)?.into_iter().map(|(call, _)| call).collect();
    let mut text = String::new();
    for part in content["parts"]
        .as_array()
        .context("missing Gemini parts")?
    {
        if part["thought"] != true
            && let Some(fragment) = part["text"].as_str()
        {
            text.push_str(fragment);
        }
    }
    Ok(ModelReply {
        continuation: content.clone(),
        calls,
        text,
        usage,
    })
}

pub(super) fn usage(value: &Value) -> TokenUsage {
    let usage = &value["usageMetadata"];
    TokenUsage {
        input_tokens: usage["promptTokenCount"].as_u64().unwrap_or(0),
        output_tokens: usage["candidatesTokenCount"]
            .as_u64()
            .unwrap_or(0)
            .saturating_add(usage["thoughtsTokenCount"].as_u64().unwrap_or(0)),
    }
}
