//! Versioned notification summaries. Add fields here deliberately when consumers need them.
use crate::{InboxEntry, Job, Project, Task};
use serde_json::{Value, json};

pub(crate) fn project(p: &Project) -> Value {
    compact_payload(&json!({"id":p.id,"name":p.name,"archived_at":p.archived_at}))
}
pub(crate) fn job(j: &Job) -> Value {
    compact_payload(
        &json!({"id":j.id,"title":j.title,"name":j.name,"status":j.status,
        "review_reason":j.review_reason,"archived_at":j.archived_at,"completed_at":j.completed_at,
        "cancelled_at":j.cancelled_at,"pending_review_at":j.pending_review_at}),
    )
}
pub(crate) fn task(t: &Task) -> Value {
    compact_payload(
        &json!({"id":t.id,"title":t.title,"name":t.name,"status":t.status,
        "phase":t.phase,"reason":t.reason,"completed_at":t.completed_at}),
    )
}
pub(crate) fn inbox(i: &InboxEntry) -> Value {
    compact_payload(&json!({"id":i.id,"status":i.status,"deleted":i.deleted}))
}

/// Compatibility projection for historical snapshots and deletion records.
/// Unknown shapes are explicitly marked, never retained as unbounded blobs.
pub(crate) fn compact_payload(payload: &Value) -> Value {
    let mut result = json!({"schema_version":1});
    let Some(object) = payload.as_object() else {
        result["unsupported_payload"] = json!(true);
        return result;
    };
    let kind = match object
        .get("id")
        .and_then(Value::as_str)
        .and_then(|s| s.split('_').next())
    {
        Some("prj") => "project",
        Some("job") => "job",
        Some("task") => "task",
        Some("inbox") => "inbox",
        _ => "unknown",
    };
    result["entity_type"] = json!(kind);
    let mut truncated = Vec::new();
    for key in [
        "id",
        "title",
        "name",
        "status",
        "phase",
        "reason",
        "review_reason",
        "deleted",
        "archived_at",
        "completed_at",
        "cancelled_at",
        "pending_review_at",
    ] {
        if let Some(value) = object.get(key) {
            if let Some(text) = value.as_str() {
                let mut end = text.len().min(4096);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                result[key] = json!(&text[..end]);
                if end < text.len() {
                    truncated.push(key);
                }
            } else if value.is_null() || value.is_boolean() || value.is_number() {
                result[key] = value.clone();
            } else {
                result["unsupported_payload"] = json!(true);
            }
        }
    }
    if !truncated.is_empty() {
        result["truncated_fields"] = json!(truncated);
    }
    // Preserve markers when already summarized; applying projection is idempotent.
    for key in ["unsupported_payload", "truncated_fields"] {
        if result.get(key).is_none()
            && let Some(value) = object.get(key)
        {
            if key == "unsupported_payload" {
                result[key] = json!(value == true);
            } else if let Some(fields) = value.as_array() {
                result[key] = json!(
                    fields
                        .iter()
                        .filter_map(Value::as_str)
                        .filter(|s| s.len() <= 32)
                        .take(12)
                        .collect::<Vec<_>>()
                );
            }
        }
    }
    result
}
