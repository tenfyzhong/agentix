use agentix_multiplexer::{
    MultiplexerError, TerminalControl, TerminalSnapshot, send_codex_exit_with,
};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Terminal {
    writes: AtomicUsize,
    draft: bool,
}
#[async_trait]
impl TerminalControl for Terminal {
    async fn snapshot(&self, _pid: u32) -> Result<TerminalSnapshot, MultiplexerError> {
        Ok(TerminalSnapshot {
            command: "codex".into(),
            cursor_x: 2,
            cursor_y: 1,
            in_mode: false,
            dead: false,
            screen: if self.draft {
                "───\n› draft\n───"
            } else {
                "Unknown dialog\nConfirm?"
            }
            .into(),
            plain_screen: if self.draft {
                "───\n› draft\n───"
            } else {
                "Unknown dialog\nConfirm?"
            }
            .into(),
        })
    }
    async fn send_key(&self, _pid: u32, _key: &str) -> Result<(), MultiplexerError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn send_text(&self, _pid: u32, _text: &str) -> Result<(), MultiplexerError> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn backend_independent_exit_never_writes_into_a_draft_or_unknown_dialog() {
    for draft in [false, true] {
        let terminal = Terminal {
            writes: AtomicUsize::new(0),
            draft,
        };
        assert!(send_codex_exit_with(&terminal, 42).await.is_err());
        assert_eq!(terminal.writes.load(Ordering::SeqCst), 0);
    }
}
