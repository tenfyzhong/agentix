#[aquamarine::aquamarine]
/// Verify that the attribute can expand in a downstream crate and keep the documented item.
///
/// ```mermaid
/// sequenceDiagram
///     participant Consumer
///     participant Listener
///     Consumer->>Listener: Next update
/// ```
struct PollingListener(u64);

#[test]
fn sequence_diagram_preserves_the_documented_item() {
    assert_eq!(PollingListener(42).0, 42);
}
