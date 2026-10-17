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
