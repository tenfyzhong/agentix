use agentix_domain::{MultiplexerKind, TerminalLocation};

#[test]
fn rex_terminal_locations_round_trip_without_changing_existing_defaults() {
    let value = serde_json::json!({
        "multiplexer": "rex", "session": "work", "windowIndex": "1",
        "windowName": "agent", "paneIndex": "1", "paneId": "block:unique"
    });
    let location: TerminalLocation = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(location.multiplexer.as_str(), "rex");
    assert_eq!(serde_json::to_value(location).unwrap(), value);
    assert_eq!(MultiplexerKind::default().as_str(), "rmux");
}
