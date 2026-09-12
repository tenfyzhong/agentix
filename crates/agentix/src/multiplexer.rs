//! Configuration intent is resolved once against live services at startup.
use agentix_core::MultiplexerKind;
use serde::Deserialize;
use std::future::Future;

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MultiplexerMode {
    #[default]
    Auto,
    Rmux,
    Tmux,
}

impl MultiplexerMode {
    /// Only polls probes required by the configured mode, in priority order.
    pub async fn resolve_with(
        self,
        rmux: impl Future<Output = bool>,
        tmux: impl Future<Output = bool>,
    ) -> Option<MultiplexerKind> {
        match self {
            Self::Auto => {
                if rmux.await {
                    Some(MultiplexerKind::Rmux)
                } else {
                    tmux.await.then_some(MultiplexerKind::Tmux)
                }
            }
            Self::Rmux => rmux.await.then_some(MultiplexerKind::Rmux),
            Self::Tmux => tmux.await.then_some(MultiplexerKind::Tmux),
        }
    }
}

impl super::MultiplexerConfig {
    /// Discover existing servers without starting either daemon.
    pub async fn detect(&mut self) {
        self.resolved_kind = self
            .kind
            .resolve_with(
                async { available("rmux", agentix_rmux::RmuxDriver::probe().await) },
                async { available("tmux", agentix_tmux::TmuxDriver::default().probe().await) },
            )
            .await;
        tracing::info!(configured = ?self.kind, resolved = ?self.resolved_kind, "terminal multiplexer detection completed");
    }
}

fn available(kind: &str, result: Result<bool, impl std::fmt::Display>) -> bool {
    match result {
        Ok(available) => available,
        Err(error) => {
            tracing::warn!(kind, %error, "terminal multiplexer probe failed");
            false
        }
    }
}
