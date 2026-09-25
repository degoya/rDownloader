//! A server speaking the reference backend's line protocol, and the scaffolding one attempt
//! needs.
//!
//! The protocol is two commands wide on purpose. What these tests are about is the contract
//! between the host and a backend — resume, pacing, the allowlist, promotion — and a real FTP
//! server in the middle would only add a second thing that can be wrong.

use std::{net::SocketAddr, sync::Arc};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

/// A running test server. Dropping it stops accepting.
pub struct Server {
    pub address: SocketAddr,
    _task: tokio::task::JoinHandle<()>,
}

/// Serves `payload` to every client.
///
/// `chunk` bounds how much is written at once and `pause` is slept between chunks, which is
/// how the pacing test gets a transfer slow enough to measure without a large file.
pub async fn serve(payload: Vec<u8>, chunk: usize, pause: std::time::Duration) -> Server {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
    let address = listener.local_addr().expect("address");
    let payload = Arc::new(payload);
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let payload = Arc::clone(&payload);
            tokio::spawn(async move {
                let _ = handle(stream, payload, chunk, pause).await;
            });
        }
    });
    Server {
        address,
        _task: task,
    }
}

async fn handle(
    mut stream: TcpStream,
    payload: Arc<Vec<u8>>,
    chunk: usize,
    pause: std::time::Duration,
) -> std::io::Result<()> {
    loop {
        let Some(line) = read_line(&mut stream).await? else {
            return Ok(());
        };
        let mut parts = line.split_whitespace();
        let command = parts.next().unwrap_or_default().to_owned();
        let _path = parts.next().unwrap_or_default();
        let offset: usize = parts
            .next()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        match command.as_str() {
            "HEAD" => {
                stream
                    .write_all(format!("OK {} 2026-09-04T00:00:00Z\n", payload.len()).as_bytes())
                    .await?;
            }
            "GET" => {
                stream
                    .write_all(format!("OK {}\n", payload.len()).as_bytes())
                    .await?;
                let mut sent = offset.min(payload.len());
                while sent < payload.len() {
                    let end = (sent + chunk).min(payload.len());
                    stream.write_all(&payload[sent..end]).await?;
                    sent = end;
                    if !pause.is_zero() {
                        tokio::time::sleep(pause).await;
                    }
                }
                return Ok(());
            }
            _ => return Ok(()),
        }
    }
}

async fn read_line(stream: &mut TcpStream) -> std::io::Result<Option<String>> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        if stream.read_exact(&mut byte).await.is_err() {
            return Ok(None);
        }
        if byte[0] == b'\n' {
            return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
        }
        line.push(byte[0]);
    }
}

/// The built reference component, or a failure naming the build command.
///
/// The wasm target is not part of an ordinary `cargo test`, so the components are built
/// separately — by `scripts/build-plugins.sh` here and by the `components` job in
/// `.github/workflows/ci.yml`. A missing artefact fails rather than passing quietly, and one
/// its stamp does not tie to the current sources fails with the build command rather than with
/// an assertion about behaviour.
pub fn component() -> Vec<u8> {
    rd_plugin_host::artifact::component("rd-plugin-example-transfer")
}

/// A manifest for the reference backend that may reach the test server's actual port.
pub fn manifest(port: u16) -> String {
    format!(
        r#"manifest_version = 3
plugin_type = "transfer"
api_version = "0.9.0"
id = "019d0000-0000-7000-8000-0000000000e1"
name = "Example Transfer"
version = "0.7.0"
key_id = "fixture"
public_key = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY="
max_concurrent_downloads = 1

[capabilities]

[capabilities.net_stream]
hosts = ["127.0.0.1"]
ports = [{port}]

[transfer]
slug = "example"
schemes = ["example+tcp"]

[metadata]
description = "Reference transfer backend"
author = "rDownloader project"

[limits]
memory_bytes = 67108864
fuel = 2000000000
timeout_milliseconds = 15000
max_response_bytes = 8388608
"#
    )
}
