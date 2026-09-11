//! Bounded HTTP response forwarding before a WebSocket connection is established.
use crate::{proxy::IoStream, proxy_wire::HandshakeStream};
use anyhow::{Context, Result, ensure};
use std::time::Duration;
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio_tungstenite::tungstenite::http::{HeaderMap, StatusCode};

pub(super) async fn write_response<W: AsyncWrite + Unpin>(
    writer: &mut W,
    bytes: &[u8],
) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(10), async {
        writer.write_all(bytes).await?;
        writer.flush().await
    })
    .await??;
    Ok(())
}

enum Body {
    Empty,
    Length(u64),
    Chunked,
    Eof,
}
fn body(status: StatusCode, headers: &HeaderMap) -> Result<Body> {
    if status.is_informational()
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED
    {
        return Ok(Body::Empty);
    }
    if headers.contains_key("transfer-encoding") {
        let values = headers
            .get_all("transfer-encoding")
            .iter()
            .map(|v| v.to_str())
            .collect::<std::result::Result<Vec<_>, _>>()?
            .join(",");
        return Ok(
            if values
                .split(',')
                .next_back()
                .is_some_and(|v| v.trim().eq_ignore_ascii_case("chunked"))
            {
                Body::Chunked
            } else {
                Body::Eof
            },
        );
    }
    let mut length = None;
    for value in headers.get_all("content-length") {
        for value in value.to_str()?.split(',') {
            let value = value.trim();
            ensure!(
                !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()),
                "invalid Content-Length"
            );
            let parsed: u64 = value.parse()?;
            ensure!(
                length.is_none_or(|n| n == parsed),
                "conflicting Content-Length"
            );
            length = Some(parsed);
        }
    }
    Ok(length.map_or(Body::Eof, Body::Length))
}

pub(super) async fn forward_rejection(
    client: &mut Box<dyn IoStream>,
    server: HandshakeStream,
    status: StatusCode,
    headers: &HeaderMap,
) -> Result<()> {
    let mode = body(status, headers)?;
    let header_len = server.header().len();
    let (server, received) = server.into_received();
    tokio::time::timeout(Duration::from_secs(10), async {
        client.write_all(&received[..header_len]).await?;
        let mut reader = BufReader::new(received[header_len..].chain(server));
        match mode {
            Body::Empty => {}
            Body::Length(n) => exact(&mut reader, client, n).await?,
            Body::Chunked => chunks(&mut reader, client).await?,
            Body::Eof => {
                tokio::io::copy(&mut reader, client).await?;
            }
        }
        client.flush().await?;
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    Ok(())
}

async fn exact<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
    n: u64,
) -> Result<()> {
    let copied = tokio::io::copy(&mut reader.take(n), writer).await?;
    ensure!(copied == n, "truncated HTTP body");
    Ok(())
}

async fn line<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(8193).read_until(b'\n', &mut bytes).await?;
    ensure!(
        bytes.len() <= 8192 && bytes.ends_with(b"\r\n"),
        "invalid or oversized HTTP chunk line"
    );
    Ok(bytes)
}

async fn chunks<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin>(
    reader: &mut R,
    writer: &mut W,
) -> Result<()> {
    loop {
        let header = line(reader).await?;
        let size = std::str::from_utf8(&header[..header.len() - 2])?
            .split(';')
            .next()
            .context("missing chunk size")?;
        ensure!(
            !size.is_empty() && size.bytes().all(|c| c.is_ascii_hexdigit()),
            "invalid chunk size"
        );
        let size = u64::from_str_radix(size, 16)?;
        writer.write_all(&header).await?;
        if size == 0 {
            let mut total = 0;
            loop {
                let trailer = line(reader).await?;
                total += trailer.len();
                ensure!(total <= 65536, "oversized HTTP trailers");
                writer.write_all(&trailer).await?;
                if trailer == b"\r\n" {
                    return Ok(());
                }
            }
        }
        exact(reader, writer, size).await?;
        let mut end = [0; 2];
        reader.read_exact(&mut end).await?;
        ensure!(&end == b"\r\n", "invalid chunk terminator");
        writer.write_all(&end).await?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn chunked_copy_preserves_fragmented_body_and_stops_before_next_message() {
        let raw = b"3;test=yes\r\nabc\r\n0\r\nX-End: yes\r\n\r\n";
        let (mut sender, receiver) = tokio::io::duplex(1);
        let task = tokio::spawn(async move {
            sender.write_all(raw).await.unwrap();
            sender.write_all(b"next").await.unwrap();
        });
        let mut reader = BufReader::new(receiver);
        let mut output = Vec::new();
        chunks(&mut reader, &mut output).await.unwrap();
        assert_eq!(output, raw);
        let mut next = [0; 4];
        reader.read_exact(&mut next).await.unwrap();
        assert_eq!(&next, b"next");
        task.await.unwrap();
    }

    #[tokio::test]
    async fn truncated_and_malformed_bodies_are_rejected() {
        for raw in [&b"3\r\nab"[..], b"z\r\n", b"1\r\naXX", b"0\r\nX: y\r\n"] {
            assert!(
                chunks(&mut BufReader::new(raw), &mut Vec::new())
                    .await
                    .is_err()
            );
        }
        assert!(exact(&mut &b"ab"[..], &mut Vec::new(), 3).await.is_err());
        let oversized = vec![b'x'; 8193];
        assert!(
            line(&mut BufReader::new(oversized.as_slice()))
                .await
                .is_err()
        );
    }

    #[test]
    fn ambiguous_lengths_are_rejected_and_transfer_encoding_takes_precedence() {
        let mut headers = HeaderMap::new();
        headers.append("content-length", "3".parse().unwrap());
        headers.append("content-length", "4".parse().unwrap());
        assert!(body(StatusCode::FORBIDDEN, &headers).is_err());
        headers.insert("transfer-encoding", "gzip, chunked".parse().unwrap());
        assert!(matches!(
            body(StatusCode::FORBIDDEN, &headers).unwrap(),
            Body::Chunked
        ));
        assert!(matches!(
            body(StatusCode::NO_CONTENT, &headers).unwrap(),
            Body::Empty
        ));
    }
}
