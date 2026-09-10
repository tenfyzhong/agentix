//! Shared slash-command naming for manifests and inbound Socket Mode events.
use agentix_domain::{ChannelCommand, ChannelError};
use serde_json::{Value, json};

#[derive(Debug, Clone, Default)]
pub struct CommandAffixes {
    prefix: String,
    suffix: String,
}

impl CommandAffixes {
    pub fn new(prefix: &str, suffix: &str) -> Result<Self, ChannelError> {
        if !prefix
            .bytes()
            .chain(suffix.bytes())
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'-'))
        {
            return Err(ChannelError::InvalidPayload("Slack command affixes allow only lowercase letters, digits, hyphens and underscores".into()));
        }
        Ok(Self {
            prefix: prefix.into(),
            suffix: suffix.into(),
        })
    }

    fn is_default(&self) -> bool {
        self.prefix.is_empty() && self.suffix.is_empty()
    }

    pub fn encode(&self, name: &str) -> Result<String, ChannelError> {
        let command = if self.is_default() && matches!(name, "rename" | "status") {
            format!("/agentix-{name}")
        } else {
            format!("/{}{name}{}", self.prefix, self.suffix)
        };
        if command.len() > 32 || !crate::manifest::valid_name(name) {
            return Err(ChannelError::InvalidPayload(
                "Slack command including affixes exceeds 32 characters or is invalid".into(),
            ));
        }
        Ok(command)
    }

    #[must_use]
    pub fn decode<'a>(&self, command: &'a str) -> Option<&'a str> {
        let name = if self.is_default() {
            command
                .strip_prefix("/agentix-")
                .or_else(|| command.strip_prefix('/'))?
        } else {
            command
                .strip_prefix('/')?
                .strip_prefix(&self.prefix)?
                .strip_suffix(&self.suffix)?
        };
        crate::manifest::valid_name(name).then_some(name)
    }

    #[must_use]
    pub fn normalize(&self, envelope: &Value) -> Option<Value> {
        let mut result = envelope.clone();
        if envelope["type"] == "slash_commands" {
            let name = self.decode(envelope["payload"]["command"].as_str()?)?;
            result["payload"]["command"] = json!(format!("/{name}"));
        }
        Some(result)
    }

    pub fn merge_manifest(
        &self,
        remote: &Value,
        commands: &[ChannelCommand],
    ) -> Result<Value, ChannelError> {
        if self.is_default() {
            return crate::manifest::merge_command_manifest(remote, commands);
        }
        let mut source = remote.clone();
        if let Some(slash) = source["features"]["slash_commands"].as_array_mut() {
            slash.retain(|entry| {
                !entry["command"]
                    .as_str()
                    .and_then(|name| self.decode(name))
                    .is_some_and(|name| {
                        matches!(name, "agentix" | "claim")
                            || commands.iter().any(|c| c.name == name)
                    })
            });
        }
        let mut merged = crate::manifest::merge_command_manifest(&source, commands)?;
        let slash = merged["features"]["slash_commands"]
            .as_array_mut()
            .expect("validated manifest");
        // The canonical merger appends the entrypoint followed by the catalog.
        let generated = slash.len() - commands.len() - 1;
        slash[generated]["command"] = json!(self.encode("agentix")?);
        for (entry, command) in slash[generated + 1..].iter_mut().zip(commands) {
            entry["command"] = json!(self.encode(&command.name)?);
        }
        Ok(merged)
    }
}
