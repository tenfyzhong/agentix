use agentix_domain::ChannelKind;
#[test]
fn channel_metadata_roundtrips_all_supported_transports() {
    for (id, name) in [
        ("telegram", "Telegram"),
        ("feishu", "Feishu"),
        ("slack", "Slack"),
    ] {
        let kind: ChannelKind = id.parse().unwrap();
        assert_eq!(kind.to_string(), id);
        assert_eq!(kind.display_name(), name);
        assert_eq!(
            serde_json::from_str::<ChannelKind>(&format!("\"{id}\"")).unwrap(),
            kind
        );
    }
}
