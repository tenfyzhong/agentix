//! Bounded rendered payload cache for removing controls without retaining model output.
use agentix_domain::MessageRef;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};

const BYTE_LIMIT: usize = 8 * 1024 * 1024;
const ENTRY_LIMIT: usize = 256;

#[derive(Default)]
pub(crate) struct ViewCache {
    values: HashMap<MessageRef, (Value, usize)>,
    order: VecDeque<MessageRef>,
    bytes: usize,
}
impl ViewCache {
    pub fn get(&self, key: &MessageRef) -> Option<&Value> {
        self.values.get(key).map(|(value, _)| value)
    }
    pub fn remove(&mut self, key: &MessageRef) {
        if let Some((_, bytes)) = self.values.remove(key) {
            self.bytes -= bytes;
            self.order.retain(|entry| entry != key);
        }
    }
    pub fn insert(&mut self, key: MessageRef, value: Value) {
        self.remove(&key);
        let bytes = value.to_string().len();
        if bytes > BYTE_LIMIT {
            return;
        }
        while self.bytes + bytes > BYTE_LIMIT || self.values.len() >= ENTRY_LIMIT {
            let Some(oldest) = self.order.front().cloned() else {
                break;
            };
            self.remove(&oldest);
        }
        self.bytes += bytes;
        self.order.push_back(key.clone());
        self.values.insert(key, (value, bytes));
    }
    #[cfg(test)]
    fn len(&self) -> usize {
        self.values.len()
    }
    #[cfg(test)]
    fn bytes(&self) -> usize {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::ViewCache;
    use agentix_domain::{ChannelKind, ConversationRef, MessageRef};
    use serde_json::json;

    #[test]
    fn retained_payloads_are_bounded_by_bytes_and_replacement_does_not_leak() {
        let mut cache = ViewCache::default();
        let message = MessageRef::new(ConversationRef::new(ChannelKind::Slack, "T:C"), "1.1");
        for _ in 0..10 {
            cache.insert(message.clone(), json!({"text":"x".repeat(1024)}));
        }
        assert_eq!(cache.len(), 1);
        for index in 0..200 {
            cache.insert(
                MessageRef::new(message.conversation.clone(), index.to_string()),
                json!({"text":"x".repeat(100_000)}),
            );
        }
        assert!(cache.bytes() <= 8 * 1024 * 1024);
        assert!(cache.len() < 100);
        assert!(cache.get(&message).is_none());
    }
}
