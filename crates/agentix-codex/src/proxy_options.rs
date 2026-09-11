//! Public connection-layer options shared by configuration and CLI decoding.
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Deserialize, clap::ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WsAuthMode {
    CapabilityToken,
    SignedBearerToken,
}

#[derive(Clone, Default, Debug, Deserialize, clap::Args, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ProxyOptions {
    /// Authentication for the Codex proxy WebSocket listener.
    #[arg(long, value_enum)]
    pub ws_auth: Option<WsAuthMode>,
    /// Absolute path to the capability-token file.
    #[arg(long)]
    pub ws_token_file: Option<PathBuf>,
    /// SHA-256 verifier for the capability token (64 hex digits).
    #[arg(long)]
    pub ws_token_sha256: Option<String>,
    /// Absolute path to the HS256 JWT shared-secret file (at least 32 bytes).
    #[arg(long)]
    pub ws_shared_secret_file: Option<PathBuf>,
    #[arg(long)]
    pub ws_issuer: Option<String>,
    #[arg(long)]
    pub ws_audience: Option<String>,
    /// JWT validation clock tolerance; defaults to 30 seconds.
    #[arg(long)]
    pub ws_max_clock_skew_seconds: Option<u64>,
}

impl ProxyOptions {
    /// Explicit CLI fields override the corresponding configuration fields.
    pub fn overlay(&mut self, other: &Self) {
        if other.ws_auth.is_some() {
            self.ws_auth = other.ws_auth;
        }
        if other.ws_token_file.is_some() {
            self.ws_token_file.clone_from(&other.ws_token_file);
        }
        if other.ws_token_sha256.is_some() {
            self.ws_token_sha256.clone_from(&other.ws_token_sha256);
        }
        if other.ws_shared_secret_file.is_some() {
            self.ws_shared_secret_file
                .clone_from(&other.ws_shared_secret_file);
        }
        if other.ws_issuer.is_some() {
            self.ws_issuer.clone_from(&other.ws_issuer);
        }
        if other.ws_audience.is_some() {
            self.ws_audience.clone_from(&other.ws_audience);
        }
        if other.ws_max_clock_skew_seconds.is_some() {
            self.ws_max_clock_skew_seconds = other.ws_max_clock_skew_seconds;
        }
    }
}
