//! Run the Codex proxy without an IM channel and print live client registrations.
//! ```sh
//! cargo run -p agentix-codex --example session_proxy -- unix:///private/tmp/proxy.sock unix:///private/tmp/upstream.sock
//! ```
#[cfg(unix)]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    use agentix_codex::{CodexClient, CodexEndpoint};
    let args: Vec<_> = std::env::args().collect();
    anyhow::ensure!(args.len() == 3, "usage: session_proxy LISTEN UPSTREAM");
    let client = CodexClient::connect_with_proxy(
        &args[1],
        CodexEndpoint::parse(&args[2])?,
        std::path::Path::new("codex"),
        std::path::Path::new("/tmp"),
        false,
    )
    .await?;
    eprintln!("ready: {} -> {}", args[1], args[2]);
    let mut last = String::new();
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            () = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                let current=serde_json::to_string(&client.client_bindings())?;
                if current != last { eprintln!("{current}"); last=current; }
            }
        }
    }
    Ok(())
}
#[cfg(not(unix))]
fn main() {
    eprintln!("Codex proxy requires macOS or Linux");
}
