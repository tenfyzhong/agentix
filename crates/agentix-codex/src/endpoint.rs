use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EndpointError {
    #[error("Codex endpoint must use unix://, ws://, or stdio://")]
    UnsupportedTransport,
    #[error("a custom Unix socket path must be absolute")]
    RelativeSocketPath,
    #[error("the current user's home directory is unavailable")]
    HomeUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexEndpoint {
    socket_path: PathBuf,
    websocket_url: Option<String>,
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
        if value == "stdio://" {
            return Ok(Self {
                socket_path: PathBuf::new(),
                websocket_url: Some(value.to_owned()),
                codex_home: None,
            });
        }
        if value.starts_with("ws://") {
            let url = url::Url::parse(value).map_err(|_| EndpointError::UnsupportedTransport)?;
            if url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
            {
                return Err(EndpointError::UnsupportedTransport);
            }
            return Ok(Self {
                socket_path: PathBuf::new(),
                websocket_url: Some(url.to_string()),
                codex_home: None,
            });
        }
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
                websocket_url: None,
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
            websocket_url: None,
            codex_home: codex_home.map(Path::to_owned),
        })
    }

    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    #[must_use]
    pub fn address(&self) -> String {
        self.websocket_url
            .clone()
            .unwrap_or_else(|| format!("unix://{}", self.socket_path.display()))
    }

    pub fn default_upstream() -> Result<Self, EndpointError> {
        let proxy = Self::parse("unix://")?;
        Self::from_socket_path(
            &proxy
                .socket_path
                .with_file_name("app-server-control-upstream.sock"),
        )
    }

    #[must_use]
    pub fn is_websocket(&self) -> bool {
        self.websocket_url
            .as_deref()
            .is_some_and(|s| s.starts_with("ws://"))
    }

    #[must_use]
    pub fn is_stdio(&self) -> bool {
        self.websocket_url.as_deref() == Some("stdio://")
    }

    pub(crate) fn codex_home(&self) -> Option<&Path> {
        self.codex_home.as_deref()
    }
}
