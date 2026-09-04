use thiserror::Error;

use crate::{GoalCommand, SessionCommand};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentCommand {
    Help,
    Sessions,
    Multiplexer,
    Attach(String),
    Current,
    Detach,
    Stop,
    Queue,
    Cancel,
    HistoryRecent,
    HistoryOlder,
    HistoryNewer,
    Session(SessionCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedInput {
    Command(AgentCommand),
    Prompt(String),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum InputParseError {
    #[error("message is empty")]
    Empty,
    #[error("command requires an argument: {0}")]
    MissingArgument(&'static str),
    #[error("unknown command: {0}")]
    UnknownCommand(String),
    #[error("invalid history direction: {0}")]
    InvalidHistoryDirection(String),
    #[error("invalid plan mode: {0}")]
    InvalidPlanMode(String),
    #[error("invalid fast mode: {0}")]
    InvalidFastMode(String),
}

pub fn parse_input(input: &str) -> Result<ParsedInput, InputParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(InputParseError::Empty);
    }
    if !input.starts_with('/') {
        return Ok(ParsedInput::Prompt(input.to_owned()));
    }

    let mut parts = input.split_whitespace();
    let raw_command = parts.next().ok_or(InputParseError::Empty)?;
    let command = raw_command
        .split_once('@')
        .map_or(raw_command, |(name, _)| name);
    let parsed = match command {
        "/help" => AgentCommand::Help,
        "/sessions" => AgentCommand::Sessions,
        "/rmux" => AgentCommand::Multiplexer,
        "/attach" => AgentCommand::Attach(
            parts
                .next()
                .ok_or(InputParseError::MissingArgument("/attach <thread-id>"))?
                .to_owned(),
        ),
        "/current" => AgentCommand::Current,
        "/detach" => AgentCommand::Detach,
        "/stop" => AgentCommand::Stop,
        "/queue" => AgentCommand::Queue,
        "/cancel" => AgentCommand::Cancel,
        "/compact" => AgentCommand::Session(SessionCommand::Compact),
        "/fork" => AgentCommand::Session(SessionCommand::Fork),
        "/fast" => AgentCommand::Session(SessionCommand::Fast(match parts.next() {
            None => None,
            Some("on") => Some(true),
            Some("off") => Some(false),
            Some(mode) => return Err(InputParseError::InvalidFastMode(mode.to_owned())),
        })),
        "/clear" => AgentCommand::Session(SessionCommand::Clear(optional_remainder(parts))),
        "/exit" => AgentCommand::Session(SessionCommand::Exit),
        "/diff" => AgentCommand::Session(SessionCommand::Diff),
        "/rename" => AgentCommand::Session(SessionCommand::Rename(optional_remainder(parts))),
        "/model" => AgentCommand::Session(SessionCommand::Model(parts.next().map(str::to_owned))),
        "/reasoning" => {
            AgentCommand::Session(SessionCommand::Reasoning(parts.next().map(str::to_owned)))
        }
        "/skills" => AgentCommand::Session(SessionCommand::Skills),
        "/plan" => {
            let argument = parts.collect::<Vec<_>>().join(" ");
            let (enabled, prompt) = match argument.as_str() {
                "" | "on" | "plan" => (true, None),
                "off" | "default" => (false, None),
                prompt => (true, Some(prompt.to_owned())),
            };
            AgentCommand::Session(SessionCommand::Plan { enabled, prompt })
        }
        "/goal" => {
            let argument = parts.collect::<Vec<_>>().join(" ");
            let command = match argument.as_str() {
                "" => GoalCommand::Show,
                "pause" => GoalCommand::Pause,
                "resume" => GoalCommand::Resume,
                "clear" => GoalCommand::Clear,
                objective => GoalCommand::Set(objective.to_owned()),
            };
            AgentCommand::Session(SessionCommand::Goal(command))
        }
        "/review" => AgentCommand::Session(SessionCommand::Review),
        "/status" => AgentCommand::Session(SessionCommand::Status),
        "/mcp" => AgentCommand::Session(SessionCommand::Mcp),
        "/history" => match parts.next() {
            None | Some("recent") => AgentCommand::HistoryRecent,
            Some("older") => AgentCommand::HistoryOlder,
            Some("newer") => AgentCommand::HistoryNewer,
            Some(direction) => {
                return Err(InputParseError::InvalidHistoryDirection(
                    direction.to_owned(),
                ));
            }
        },
        _ => return Err(InputParseError::UnknownCommand(raw_command.to_owned())),
    };
    Ok(ParsedInput::Command(parsed))
}

fn optional_remainder<'a>(parts: impl Iterator<Item = &'a str>) -> Option<String> {
    let value = parts.collect::<Vec<_>>().join(" ");
    (!value.is_empty()).then_some(value)
}
