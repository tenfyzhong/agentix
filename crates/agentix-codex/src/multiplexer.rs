pub(crate) use agentix_multiplexer::*;
use std::path::Path;
pub(crate) fn launch_argv(command: &Path, endpoint: &str) -> Vec<String> {
    vec![
        command.to_string_lossy().into_owned(),
        "--remote".into(),
        endpoint.into(),
    ]
}
