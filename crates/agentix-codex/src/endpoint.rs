use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EndpointError {
    #[error("Codex endpoint must use unix:// in this release")]
    UnsupportedTransport,
    #[error("a custom Unix socket path must be absolute")]
    RelativeSocketPath,
    #[error("the current user's home directory is unavailable")]
    HomeUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexEndpoint {
    socket_path: PathBuf,
    codex_home: Option<PathBuf>,
}

impl CodexEndpoint {
    pub fn parse(value: &str) -> Result<Self, EndpointError> {
        let codex_home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".codex")));
        Self::parse_with_codex_home(value, codex_home.as_deref())
    }

    pub fn parse_with_codex_home(
        value: &str,
        codex_home: Option<&Path>,
    ) -> Result<Self, EndpointError> {
        let Some(path) = value.strip_prefix("unix://") else {
            return Err(EndpointError::UnsupportedTransport);
        };
        if path.is_empty() {
            let home = codex_home.ok_or(EndpointError::HomeUnavailable)?;
            return Ok(Self {
                socket_path: home
                    .join("app-server-control")
                    .join("app-server-control.sock"),
                codex_home: Some(home.to_owned()),
            });
        }
        let socket_path = Path::new(path);
        let managed_home = codex_home.filter(|home| {
            socket_path
                == home
                    .join("app-server-control")
                    .join("app-server-control.sock")
        });
        Self::from_socket_path_and_home(socket_path, managed_home)
    }

    pub fn from_socket_path(path: &Path) -> Result<Self, EndpointError> {
        Self::from_socket_path_and_home(path, None)
    }

    fn from_socket_path_and_home(
        path: &Path,
        codex_home: Option<&Path>,
    ) -> Result<Self, EndpointError> {
        if !path.is_absolute() {
            return Err(EndpointError::RelativeSocketPath);
        }
        Ok(Self {
            socket_path: path.to_owned(),
            codex_home: codex_home.map(Path::to_owned),
        })
    }

    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub(crate) fn codex_home(&self) -> Option<&Path> {
        self.codex_home.as_deref()
    }
}
