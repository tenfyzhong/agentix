use std::time::Duration;

use agentix::NetworkConfig;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

async fn read_headers(stream: &mut TcpStream) -> String {
    let mut reader = BufReader::new(stream);
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).await.unwrap() > 0);
        headers.push_str(&line);
        if line == "\r\n" {
            return headers;
        }
    }
}

async fn respond(stream: &mut TcpStream) {
    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nproxied")
        .await
        .unwrap();
}

fn client(network: &NetworkConfig) -> reqwest::Client {
    network
        .http_client(reqwest::Client::builder().timeout(Duration::from_secs(2)))
        .unwrap()
}

#[tokio::test]
async fn global_http_proxy_overrides_existing_routing_and_authenticates() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let headers = read_headers(&mut stream).await;
        assert!(headers.starts_with("GET http://origin.invalid/resource HTTP/1.1\r\n"));
        // Decoded credentials are user:p@ss.
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("proxy-authorization: basic dxnlcjpwqhnz")
        );
        respond(&mut stream).await;
    });
    let network = NetworkConfig {
        proxy: Some(format!("http://user:p%40ss@{address}")),
    };
    let builder = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all("http://127.0.0.1:1").unwrap())
        .timeout(Duration::from_secs(2));
    let response = network
        .http_client(builder)
        .unwrap()
        .get("http://origin.invalid/resource")
        .send()
        .await
        .unwrap();
    assert_eq!(response.text().await.unwrap(), "proxied");
    server.await.unwrap();
}

#[tokio::test]
async fn https_uses_connect_and_does_not_fall_back_when_the_proxy_rejects_it() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let headers = read_headers(&mut stream).await;
        assert!(headers.starts_with("CONNECT origin.invalid:443 HTTP/1.1\r\n"));
        stream
            .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
    });
    let network = NetworkConfig {
        proxy: Some(format!("http://{address}")),
    };
    assert!(
        client(&network)
            .get("https://origin.invalid/")
            .send()
            .await
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn socks5_proxies_route_requests_and_socks5h_resolves_names_at_the_proxy() {
    for (scheme, origin, expected_address) in [
        ("socks5", "127.0.0.1", vec![1, 127, 0, 0, 1]),
        (
            "socks5h",
            "origin.invalid",
            [vec![3, 14], b"origin.invalid".to_vec()].concat(),
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut greeting = [0; 2];
            stream.read_exact(&mut greeting).await.unwrap();
            assert_eq!(greeting[0], 5);
            let mut methods = vec![0; usize::from(greeting[1])];
            stream.read_exact(&mut methods).await.unwrap();
            assert!(methods.contains(&0));
            stream.write_all(&[5, 0]).await.unwrap();
            let mut request = vec![0; 3 + expected_address.len() + 2];
            stream.read_exact(&mut request).await.unwrap();
            assert_eq!(&request[..3], &[5, 1, 0]);
            assert_eq!(&request[3..request.len() - 2], &expected_address);
            assert_eq!(&request[request.len() - 2..], &80_u16.to_be_bytes());
            stream
                .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])
                .await
                .unwrap();
            assert!(
                read_headers(&mut stream)
                    .await
                    .starts_with("GET /resource HTTP/1.1\r\n")
            );
            respond(&mut stream).await;
        });
        let network = NetworkConfig {
            proxy: Some(format!("{scheme}://{address}")),
        };
        let response = client(&network)
            .get(format!("http://{origin}/resource"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.text().await.unwrap(), "proxied");
        server.await.unwrap();
    }
}

#[tokio::test]
async fn absent_global_proxy_preserves_the_client_routing() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        assert!(
            read_headers(&mut stream)
                .await
                .starts_with("GET /resource HTTP/1.1\r\n")
        );
        respond(&mut stream).await;
    });
    let client = NetworkConfig::default()
        .http_client(
            reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(2)),
        )
        .unwrap();
    assert!(
        client
            .get(format!("http://{address}/resource"))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    );
    server.await.unwrap();
}

#[test]
fn network_debug_output_redacts_proxy_credentials() {
    let network = NetworkConfig {
        proxy: Some("http://user:private-password@proxy.example:7890".into()),
    };
    let debug = format!("{network:?}");
    assert!(!debug.contains("private-password"));
    assert!(!debug.contains("user"));
}
