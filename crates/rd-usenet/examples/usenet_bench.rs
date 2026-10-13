//! The Usenet throughput measurements (RD-108-25, RD-130-22), an example rather than ignored
//! tests since RD-1280-22: the worker path against the scripted fixture with a round trip and a
//! per-connection byte rate. Each prints rather than asserts, and a number is comparable only
//! with the same command on other code. On a quiet machine:
//!
//! ```bash
//! cargo run --release -p rd-usenet --features bench --example usenet_bench -- fixture
//! cargo run --release -p rd-usenet --features bench --example usenet_bench -- small-files
//! cargo run --release -p rd-usenet --features bench --example usenet_bench -- sweep
//! ```
//!
//! `RD_BENCH_RTT_MS` and `RD_BENCH_RATE_MIB` set the line, `RD_BENCH_FILES` and
//! `RD_BENCH_ARTICLES` the sweep's release (`src/throughput_bench.rs`).

use rd_usenet::throughput_bench;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("fixture") => throughput_bench::fixture_throughput().await,
        Some("small-files") => throughput_bench::many_small_files().await,
        Some("sweep") => throughput_bench::sweep::parallel_files_sweep().await,
        _ => {
            eprintln!("usage: usenet_bench fixture|small-files|sweep");
            std::process::exit(2);
        }
    }
}
