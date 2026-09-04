#[cfg(unix)]
mod unix {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    use agentix_core::{AgentAdapter, AgentEvent, SessionId};
    use agentix_pi::{PiFlavor, PiRpcAdapter};

    #[tokio::test]
    async fn subprocess_rpc_streams_events_for_the_attached_session() {
        let directory = tempfile::tempdir().unwrap();
        let session_path = directory.path().join("session.jsonl");
        fs::write(
            &session_path,
            concat!(
                "{\"type\":\"session\",\"version\":3,\"id\":\"pi-abc\",\"cwd\":\"/tmp\"}\n",
                "{\"type\":\"message\",\"message\":{\"role\":\"user\",\"content\":\"old question\"}}\n",
                "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"old answer\"}]}}\n"
            ),
        )
        .unwrap();
        let command = directory.path().join("fake-pi");
        fs::write(
            &command,
            r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  case "$line" in
    *'"type":"prompt"'*)
      printf '{"id":"%s","type":"response","command":"prompt","success":true}\n' "$id"
      printf '{"type":"agent_start"}\n'
      printf '{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"live answer"}}\n'
      printf '{"type":"agent_settled"}\n'
      ;;
    *'"type":"abort"'*)
      printf '{"id":"%s","type":"response","command":"abort","success":true}\n' "$id"
      ;;
  esac
done
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&command).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&command, permissions).unwrap();

        let adapter = Arc::new(PiRpcAdapter::new(PiFlavor::Pi, &command, directory.path()));
        let sessions = adapter.list_sessions(None, 10).await.unwrap();
        assert_eq!(sessions.sessions[0].id.as_str(), "pi-abc");
        let history = adapter
            .read_history(&SessionId::new("pi-abc"), None, 5)
            .await
            .unwrap();
        assert_eq!(history.turns[0].agent_text.as_deref(), Some("old answer"));

        adapter.attach(&SessionId::new("pi-abc")).await.unwrap();
        let mut events = adapter.subscribe();
        let turn_id = adapter
            .start_turn(&SessionId::new("pi-abc"), "new question")
            .await
            .unwrap();
        assert!(!turn_id.is_empty());

        let mut saw_delta = false;
        let mut saw_completion = false;
        for _ in 0..3 {
            match events.recv().await.unwrap() {
                AgentEvent::AgentMessageDelta { session_id, .. } => {
                    assert_eq!(session_id, "pi-abc");
                    saw_delta = true;
                }
                AgentEvent::TurnCompleted { session_id, .. } => {
                    assert_eq!(session_id, "pi-abc");
                    saw_completion = true;
                }
                _ => {}
            }
        }
        assert!(saw_delta && saw_completion);
    }
}
