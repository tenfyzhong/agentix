use agentix_domain::SessionId;

#[test]
fn opencode_native_identity_matches_only_its_own_taskix_lease() {
    let session = SessionId::new("opencode:ses_native:opaque");
    assert_eq!(session.native_str(), "ses_native:opaque");
    assert_eq!(session.task_executor(), "agent:opencode");
    assert!(session.matches_task_lease("ses_native:opaque", "agent:opencode:ses_native:opaque"));
    assert!(!session.matches_task_lease("ses_native:opaque", "agent:codex:ses_native:opaque"));
    assert!(!session.matches_task_lease("other", "agent:opencode:ses_native:opaque"));
}

#[test]
fn antigravity_native_identity_matches_only_its_own_taskix_lease() {
    let session = SessionId::new("antigravity:conversation:opaque");
    assert_eq!(session.native_str(), "conversation:opaque");
    assert_eq!(session.task_executor(), "agent:antigravity");
    assert!(session.matches_task_lease("conversation:opaque", "agent:antigravity"));
    assert!(!session.matches_task_lease("conversation:opaque", "agent:claude"));
}

#[test]
fn antigravity_session_identity_round_trips() {
    let session = SessionId::new("antigravity:conversation:opaque");
    let key = agentix_domain::SessionRef::decode(&session).expect("recognized backend");
    assert_eq!(key.encode(), session);
    assert_eq!(key.agent.as_str(), "antigravity");
    assert_eq!(key.agent.display_name(), "Antigravity");
}
