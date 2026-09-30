//! The origin Axis B downloads from: plain HTTP with ranges, fixed validators and a gate.

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

/// What the origin writes at once between two looks at its gate.
const CHUNK: u64 = 64 * 1024;

/// Every byte depends on where it sits, so a resume at the wrong offset cannot look right.
pub fn payload(len: u64) -> Vec<u8> {
    (0..len)
        .map(|index| ((index % 251) ^ (index / 4096 % 256)) as u8)
        .collect()
}

/// A plain HTTP origin with ranges, fixed validators and a gate.
pub struct Origin {
    address: SocketAddr,
    shared: Arc<Shared>,
}

struct Shared {
    payload: Vec<u8>,
    /// Payload bytes that may go out in total before every connection stalls.
    gate: AtomicU64,
    /// Which run of the service a new connection counts for.
    run: AtomicUsize,
    /// Payload bytes sent, per run.
    sent: [AtomicU64; 2],
}

impl Shared {
    fn sent_total(&self) -> u64 {
        self.sent[0].load(Ordering::SeqCst) + self.sent[1].load(Ordering::SeqCst)
    }
}

impl Origin {
    pub async fn start(payload: Vec<u8>, gate: u64) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        let shared = Arc::new(Shared {
            payload,
            gate: AtomicU64::new(gate),
            run: AtomicUsize::new(0),
            sent: [AtomicU64::new(0), AtomicU64::new(0)],
        });
        let serving = Arc::clone(&shared);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let shared = Arc::clone(&serving);
                tokio::spawn(async move {
                    let _ = answer(stream, shared).await;
                });
            }
        });
        Self { address, shared }
    }

    pub fn url(&self) -> String {
        format!("http://{}/payload.bin", self.address)
    }

    pub fn sent(&self, run: usize) -> u64 {
        self.shared.sent[run].load(Ordering::SeqCst)
    }

    /// Opens the gate for good; connections from here on count for the second run.
    pub fn release(&self) {
        self.shared.run.store(1, Ordering::SeqCst);
        self.shared.gate.store(u64::MAX, Ordering::SeqCst);
    }
}

async fn read_head(stream: &mut TcpStream) -> std::io::Result<String> {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") && head.len() < 16 * 1024 {
        stream.read_exact(&mut byte).await?;
        head.push(byte[0]);
    }
    Ok(String::from_utf8_lossy(&head).into_owned())
}

/// `bytes=START-` or `bytes=START-END`, clamped to the payload.
fn parse_range(value: &str, total: u64) -> Option<(u64, u64)> {
    let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    let end = match end.trim() {
        "" => total - 1,
        end => end.parse::<u64>().ok()?.min(total - 1),
    };
    (start <= end).then_some((start, end))
}

async fn answer(mut stream: TcpStream, shared: Arc<Shared>) -> std::io::Result<()> {
    let run = shared.run.load(Ordering::SeqCst);
    let head = read_head(&mut stream).await?;
    let total = shared.payload.len() as u64;
    let range = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("range"))
        .and_then(|(_, value)| parse_range(value.trim(), total));
    let (status, start, end) = match range {
        Some((start, end)) => ("206 Partial Content", start, end),
        None => ("200 OK", 0, total - 1),
    };
    let length = end + 1 - start;
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/octet-stream\r\n\
         Content-Length: {length}\r\nAccept-Ranges: bytes\r\nETag: \"axis-b\"\r\n\
         Last-Modified: Wed, 30 Sep 2026 00:00:00 GMT\r\nConnection: close\r\n"
    );
    if range.is_some() {
        response.push_str(&format!("Content-Range: bytes {start}-{end}/{total}\r\n"));
    }
    response.push_str("\r\n");
    stream.write_all(response.as_bytes()).await?;
    if head.starts_with("HEAD ") {
        return Ok(());
    }
    let mut offset = start;
    while offset <= end {
        let size = (end + 1 - offset).min(CHUNK);
        while shared.sent_total() + size > shared.gate.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let from = usize::try_from(offset).expect("offset");
        let to = usize::try_from(offset + size).expect("offset");
        stream.write_all(&shared.payload[from..to]).await?;
        shared.sent[run].fetch_add(size, Ordering::SeqCst);
        offset += size;
    }
    Ok(())
}
