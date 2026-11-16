//! Authentication terminates at the proxy; client credentials never reach upstream.
use crate::{ProxyOptions, WsAuthMode};
use anyhow::{Context, Result, ensure};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use subtle::ConstantTimeEq;
use tokio_tungstenite::tungstenite::{
    handshake::server::{ErrorResponse, Request, Response},
    http::StatusCode,
};

pub(crate) enum AuthPolicy {
    None,
    Capability([u8; 32]),
    Signed {
        key: DecodingKey,
        issuer: Option<String>,
        audience: Option<String>,
        skew: u64,
    },
}
impl AuthPolicy {
    pub(crate) fn load(options: &ProxyOptions) -> Result<Self> {
        let capability = options.ws_token_file.is_some()
            || options.ws_token_sha256.is_some()
            || options.ws_token_pass.is_some();
        let signed = options.ws_shared_secret_file.is_some()
            || options.ws_shared_secret_pass.is_some()
            || options.ws_issuer.is_some()
            || options.ws_audience.is_some()
            || options.ws_max_clock_skew_seconds.is_some();
        match options.ws_auth {
            None => {
                ensure!(
                    !capability && !signed,
                    "ws_auth is required with authentication options"
                );
                Ok(Self::None)
            }
            Some(WsAuthMode::CapabilityToken) => {
                ensure!(
                    !signed,
                    "signed bearer options require ws_auth signed-bearer-token"
                );
                let hash = match (
                    &options.ws_token_file,
                    &options.ws_token_sha256,
                    &options.ws_token_pass,
                ) {
                    (Some(path), None, None) => Sha256::digest(secret(path)?.as_bytes()).into(),
                    (None, Some(hex), None) => {
                        ensure!(
                            hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
                            "ws_token_sha256 must contain 64 hex digits"
                        );
                        let mut digest = [0; 32];
                        for (i, byte) in digest.iter_mut().enumerate() {
                            *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)?;
                        }
                        digest
                    }
                    (None, None, Some(entry)) => {
                        Sha256::digest(agentix_secrets::key(entry)?.as_bytes()).into()
                    }
                    _ => anyhow::bail!(
                        "capability-token requires exactly one of ws_token_file, ws_token_sha256 or ws_token_pass"
                    ),
                };
                Ok(Self::Capability(hash))
            }
            Some(WsAuthMode::SignedBearerToken) => {
                ensure!(
                    !capability,
                    "capability options require ws_auth capability-token"
                );
                let secret = match (
                    &options.ws_shared_secret_file,
                    &options.ws_shared_secret_pass,
                ) {
                    (Some(path), None) => secret(path)?,
                    (None, Some(entry)) => agentix_secrets::key(entry)?,
                    _ => anyhow::bail!(
                        "exactly one of ws_shared_secret_file or ws_shared_secret_pass is required"
                    ),
                };
                ensure!(
                    secret.len() >= 32,
                    "shared secret must contain at least 32 bytes"
                );
                Ok(Self::Signed {
                    key: DecodingKey::from_secret(secret.as_bytes()),
                    issuer: normalized(options.ws_issuer.as_deref()),
                    audience: normalized(options.ws_audience.as_deref()),
                    skew: options.ws_max_clock_skew_seconds.unwrap_or(30),
                })
            }
        }
    }
    pub(crate) fn enabled(&self) -> bool {
        !matches!(self, Self::None)
    }
    #[allow(clippy::result_large_err)]
    pub(crate) fn upgrade(
        &self,
        request: &Request,
        response: Response,
    ) -> Result<Response, ErrorResponse> {
        if request.headers().contains_key("Origin") {
            return Err(reject(StatusCode::FORBIDDEN));
        }
        if !self.enabled() {
            return Ok(response);
        }
        let headers = request.headers();
        let token = (headers.get_all("Authorization").iter().count() == 1)
            .then(|| headers.get("Authorization")?.to_str().ok()?.split_once(' '))
            .flatten()
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
            .map(|(_, token)| token.trim())
            .filter(|token| !token.is_empty());
        if token.is_some_and(|token| self.accepts(token)) {
            Ok(response)
        } else {
            Err(reject(StatusCode::UNAUTHORIZED))
        }
    }
    fn accepts(&self, token: &str) -> bool {
        match self {
            Self::None => true,
            Self::Capability(expected) => {
                bool::from(expected.ct_eq(&Sha256::digest(token.as_bytes())))
            }
            Self::Signed {
                key,
                issuer,
                audience,
                skew,
            } => {
                let mut validation = Validation::new(Algorithm::HS256);
                validation.validate_exp = false;
                validation.validate_nbf = false;
                validation.validate_aud = false;
                let Ok(data) = jsonwebtoken::decode::<Claims>(token, key, &validation) else {
                    return false;
                };
                let claims = data.claims;
                let now = i64::try_from(jsonwebtoken::get_current_timestamp()).unwrap_or(i64::MAX);
                let skew = i64::try_from(*skew).unwrap_or(i64::MAX);
                now <= claims.exp.saturating_add(skew)
                    && claims.nbf.is_none_or(|nbf| now >= nbf.saturating_sub(skew))
                    && issuer
                        .as_ref()
                        .is_none_or(|expected| claims.iss.as_ref() == Some(expected))
                    && audience.as_ref().is_none_or(|expected| match &claims.aud {
                        Some(Audience::Single(value)) => value == expected,
                        Some(Audience::Multiple(values)) => values.contains(expected),
                        None => false,
                    })
            }
        }
    }
}
#[derive(Clone, Deserialize)]
struct Claims {
    exp: i64,
    nbf: Option<i64>,
    iss: Option<String>,
    aud: Option<Audience>,
}
#[derive(Clone, Deserialize)]
#[serde(untagged)]
enum Audience {
    Single(String),
    Multiple(Vec<String>),
}
fn normalized(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}
fn secret(path: &Path) -> Result<String> {
    ensure!(
        path.is_absolute(),
        "WebSocket secret file path must be absolute"
    );
    let value = std::fs::read_to_string(path).context("read WebSocket secret file")?;
    let value = value.trim();
    ensure!(!value.is_empty(), "WebSocket secret file is empty");
    Ok(value.to_owned())
}
fn reject(status: StatusCode) -> ErrorResponse {
    Response::builder()
        .status(status)
        .body(Some("WebSocket access denied".into()))
        .unwrap()
}

#[cfg(all(test, unix))]
mod pass_tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, process::Command};

    #[test]
    fn pass_authentication_accepts_only_resolved_credentials() {
        if std::env::var_os("AGENTIX_PROXY_PASS_TEST").is_some() {
            let options = |value| serde_json::from_value::<ProxyOptions>(value).unwrap();
            let capability = AuthPolicy::load(&options(serde_json::json!({
                "ws_auth":"capability-token", "ws_token_pass":"test/proxy"
            })))
            .unwrap();
            let key = "test-secret-with-at-least-32-bytes";
            assert!(capability.accepts(key));
            assert!(!capability.accepts("wrong"));
            let signed = AuthPolicy::load(&options(serde_json::json!({
                "ws_auth":"signed-bearer-token", "ws_shared_secret_pass":"test/proxy"
            })))
            .unwrap();
            let token = jsonwebtoken::encode(
                &jsonwebtoken::Header::default(),
                &serde_json::json!({"exp": jsonwebtoken::get_current_timestamp() + 60}),
                &jsonwebtoken::EncodingKey::from_secret(key.as_bytes()),
            )
            .unwrap();
            assert!(signed.accepts(&token));
            assert!(!signed.accepts(key));
            for invalid in [
                serde_json::json!({"ws_token_pass":"test/proxy"}),
                serde_json::json!({"ws_auth":"capability-token", "ws_token_pass":"test/proxy", "ws_token_file":"/unused"}),
                serde_json::json!({"ws_auth":"capability-token", "ws_token_pass":"test/proxy", "ws_token_sha256":"00".repeat(32)}),
                serde_json::json!({"ws_auth":"signed-bearer-token", "ws_shared_secret_pass":"test/proxy", "ws_shared_secret_file":"/unused"}),
                serde_json::json!({"ws_auth":"capability-token", "ws_shared_secret_pass":"test/proxy"}),
                serde_json::json!({"ws_auth":"signed-bearer-token", "ws_token_pass":"test/proxy"}),
            ] {
                assert!(AuthPolicy::load(&options(invalid)).is_err());
            }
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pass");
        fs::write(&path, "#!/bin/sh\n[ \"$1\" = show ] && [ \"$2\" = -- ] && [ \"$3\" = test/proxy ] || exit 3\nprintf 'test-secret-with-at-least-32-bytes\\n'\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "proxy_auth::pass_tests::pass_authentication_accepts_only_resolved_credentials",
                "--nocapture",
            ])
            .env("AGENTIX_PROXY_PASS_TEST", "1")
            .env("PATH", dir.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
