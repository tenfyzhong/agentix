use std::collections::HashSet;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RenderKey {
    pub session_id: String,
    pub turn_id: String,
    pub item_id: String,
}

impl RenderKey {
    #[must_use]
    pub fn new(
        session_id: impl Into<String>,
        turn_id: impl Into<String>,
        item_id: impl Into<String>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            turn_id: turn_id.into(),
            item_id: item_id.into(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HistoryWatermark {
    completed_items: HashSet<RenderKey>,
}

impl HistoryWatermark {
    #[must_use]
    pub fn from_completed(items: impl IntoIterator<Item = RenderKey>) -> Self {
        Self {
            completed_items: items.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn should_apply(&self, key: &RenderKey) -> bool {
        !self.completed_items.contains(key)
    }
}

/// Split text by bytes without cutting a UTF-8 scalar value.
#[must_use]
pub fn chunk_text(text: &str, max_bytes: usize) -> Vec<String> {
    assert!(max_bytes > 0, "max_bytes must be positive");
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let tentative_end = start.saturating_add(max_bytes).min(text.len());
        let mut end = text.floor_char_boundary(tentative_end);
        if end == start {
            end = text.ceil_char_boundary(start.saturating_add(1).min(text.len()));
        }
        chunks.push(text[start..end].to_owned());
        start = end;
    }
    chunks
}
