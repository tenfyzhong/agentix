use std::collections::HashMap;

use thiserror::Error;
use uuid::Uuid;

use crate::ConversationRef;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionScope {
    pub conversation: ConversationRef,
    pub owner_id: String,
    pub connection_generation: u64,
    pub binding_epoch: u64,
    pub group_id: String,
}

impl ActionScope {
    #[must_use]
    pub fn new(
        conversation: ConversationRef,
        owner_id: impl Into<String>,
        connection_generation: u64,
        binding_epoch: u64,
        group_id: impl Into<String>,
    ) -> Self {
        Self {
            conversation,
            owner_id: owner_id.into(),
            connection_generation,
            binding_epoch,
            group_id: group_id.into(),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ActionTokenError {
    #[error("action token is unknown or has already been consumed")]
    Unknown,
    #[error("action token does not belong to this owner or conversation")]
    RouteMismatch,
    #[error("action token belongs to an obsolete agent connection")]
    StaleConnection,
    #[error("action token belongs to an obsolete session binding")]
    StaleBinding,
}

#[derive(Debug)]
struct RegisteredAction<T> {
    scope: ActionScope,
    value: T,
}

/// Stores opaque, single-use UI actions and validates their complete execution scope.
#[derive(Debug)]
pub struct ActionRegistry<T> {
    actions: HashMap<String, RegisteredAction<T>>,
}

impl<T> Default for ActionRegistry<T> {
    fn default() -> Self {
        Self {
            actions: HashMap::new(),
        }
    }
}

impl<T> ActionRegistry<T> {
    pub fn issue(&mut self, scope: ActionScope, value: T) -> String {
        let token = Uuid::new_v4().simple().to_string();
        self.actions
            .insert(token.clone(), RegisteredAction { scope, value });
        token
    }

    pub fn consume(
        &mut self,
        token: &str,
        conversation: &ConversationRef,
        owner_id: &str,
        connection_generation: u64,
        binding_epoch: u64,
    ) -> Result<T, ActionTokenError> {
        let registered = self.actions.get(token).ok_or(ActionTokenError::Unknown)?;
        if &registered.scope.conversation != conversation || registered.scope.owner_id != owner_id {
            return Err(ActionTokenError::RouteMismatch);
        }
        if registered.scope.connection_generation != connection_generation {
            return Err(ActionTokenError::StaleConnection);
        }
        if registered.scope.binding_epoch != binding_epoch {
            return Err(ActionTokenError::StaleBinding);
        }
        let group_id = registered.scope.group_id.clone();
        let value = self
            .actions
            .remove(token)
            .map(|registered| registered.value)
            .ok_or(ActionTokenError::Unknown)?;
        self.revoke_group(&group_id);
        Ok(value)
    }

    pub fn revoke_group(&mut self, group_id: &str) {
        self.actions
            .retain(|_, action| action.scope.group_id != group_id);
    }

    pub fn invalidate_generation(&mut self, generation: u64) {
        self.actions
            .retain(|_, action| action.scope.connection_generation != generation);
    }

    pub fn retain(&mut self, mut predicate: impl FnMut(&T) -> bool) {
        self.actions.retain(|_, action| predicate(&action.value));
    }

    pub fn clear(&mut self) {
        self.actions.clear();
    }
}
