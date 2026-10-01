//! Environment-sensitive network checks run in separate test subprocesses.
use std::time::Duration;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    process::Command,
};

pub fn fixture_command(test: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--ignored", "--exact", test, "--nocapture"]);
    for name in [
        "http_proxy",
        "HTTP_PROXY",
        "https_proxy",
        "HTTPS_PROXY",
        "no_proxy",
        "NO_PROXY",
        "all_proxy",
        "ALL_PROXY",
        "REQUEST_METHOD",
        "TELOXIDE_PROXY",
    ] {
        command.env_remove(name);
    }
    command.kill_on_drop(true);
    command
}

pub async fn assert_success(command: &mut Command) {
    let output = tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .expect("proxy fixture timed out")
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub async fn check_routing(
    test: &str,
    variable: &str,
    https: bool,
    bypass: Option<&str>,
    requests: usize,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let bypass_enabled = bypass.is_some();
    let target = if bypass_enabled {
        format!("http://{address}")
    } else if https {
        "https://origin.invalid".into()
    } else {
        "http://origin.invalid".into()
    };
    let server = tokio::spawn(async move {
        for _ in 0..requests {
            let (mut socket, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .expect("expected request did not reach the local fixture")
                .unwrap();
            let mut reader = BufReader::new(&mut socket);
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).await.unwrap() > 0);
                headers.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            if https {
                assert!(headers.starts_with("CONNECT origin.invalid:443 HTTP/1.1\r\n"));
            } else if bypass_enabled {
                assert!(headers.starts_with("GET /"));
                assert!(!headers.to_ascii_lowercase().contains("proxy-authorization"));
            } else {
                assert!(headers.starts_with("GET http://origin.invalid/"));
                assert!(
                    headers
                        .to_ascii_lowercase()
                        .contains("proxy-authorization: basic dxnlcjpwqhnz")
                );
            }
            let response = if https {
                b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .as_slice()
            } else {
                b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nproxied"
                    .as_slice()
            };
            socket.write_all(response).await.unwrap();
        }
    });
    let mut command = fixture_command(test);
    command.env("PROXY_TEST_URL", target);
    command.env(
        variable,
        if bypass_enabled {
            "http://127.0.0.1:1".into()
        } else {
            format!("http://user:p%40ss@{address}")
        },
    );
    // A variable for the other scheme must not affect this request.
    command.env(
        if https { "HTTP_PROXY" } else { "HTTPS_PROXY" },
        "http://127.0.0.1:1",
    );
    if let Some(bypass_variable) = bypass {
        command.env(bypass_variable, "127.0.0.1");
    }
    assert_success(&mut command).await;
    server.await.unwrap();
}
