//! Just enough HTTP/1.1 for a loopback OpenAI-compatible endpoint: one
//! request per connection, `Connection: close`, SSE for streamed replies.
//! A dependency-free server keeps the relay inside Nexora's single binary.

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub struct Request {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
}

pub async fn read_request(stream: &mut TcpStream) -> Result<Request> {
    let mut buffer = Vec::with_capacity(4_096);
    let header_end = loop {
        let mut chunk = [0_u8; 4_096];
        let read = stream
            .read(&mut chunk)
            .await
            .context("request read failed")?;
        if read == 0 {
            bail!("connection closed before a full request arrived");
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = find_header_end(&buffer) {
            break position;
        }
        if buffer.len() > 64 * 1024 {
            bail!("request headers too large");
        }
    };

    let head = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();

    let mut content_length = 0_usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }
    if content_length > 16 * 1024 * 1024 {
        bail!("request body too large");
    }

    let mut body = buffer[header_end + 4..].to_vec();
    while body.len() < content_length {
        let mut chunk = vec![0_u8; (content_length - body.len()).min(64 * 1024)];
        let read = stream.read(&mut chunk).await.context("body read failed")?;
        if read == 0 {
            bail!("connection closed mid-body");
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);
    Ok(Request { method, path, body })
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

pub async fn respond_json(stream: &mut TcpStream, status: u16, body: &str) -> Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "Internal Server Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.flush().await.context("response write failed")
}

/// Start a Server-Sent Events response; follow with `sse_event` calls.
pub async fn start_sse(stream: &mut TcpStream) -> Result<()> {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        )
        .await
        .context("SSE header write failed")
}

pub async fn sse_event(stream: &mut TcpStream, data: &str) -> Result<()> {
    stream
        .write_all(format!("data: {data}\n\n").as_bytes())
        .await?;
    stream.flush().await.context("SSE write failed")
}
