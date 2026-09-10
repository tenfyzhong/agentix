//! Opt-in live verification; updates the explicitly selected Slack app.
use agentix_core::ChannelCommand;
use agentix_slack::SlackCommandSync;

#[tokio::test]
#[ignore = "requires Slack CLI login and SLACK_LIVE_APP_ID/SLACK_LIVE_TEAM_ID; updates the app"]
async fn sync_live_slack_command_catalog_and_verify_idempotence() {
    let app = std::env::var("SLACK_LIVE_APP_ID").expect("SLACK_LIVE_APP_ID");
    let team = std::env::var("SLACK_LIVE_TEAM_ID").expect("SLACK_LIVE_TEAM_ID");
    let mut commands = agentix_core::command_menu(true).commands;
    commands.extend(
        [
            ("dashboard", "Browse projects and task boards"),
            ("board", "Show this session's task board"),
            ("jobs", "Browse this session's jobs"),
            ("inboxes", "Browse this project's inbox"),
            ("inbox", "Append a requirement to this project's inbox"),
        ]
        .map(|(name, description)| ChannelCommand::new(name, description)),
    );
    let sync = SlackCommandSync::new("slack".into(), app, commands);
    sync.sync(&team).await.unwrap();
    assert!(!sync.sync(&team).await.unwrap());
}
