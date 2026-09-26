//! The measurement behind RD-130-22: one release of many files, downloaded with 1, 2, 3 and 4
//! files at once and in automatic mode, everything else equal.
//!
//! The files are dispatched the way the scheduler dispatches them - a pass every
//! [`DISPATCH_TICK`], the capacity read once per pass, a finished file's place free only at
//! the next pass - so the gap at a file boundary is the gap the service has. Per mode it
//! prints the throughput and how much it varies between 100 ms samples, how long each
//! connection sat idle (measured at the fixture, from its first command to the end), how long
//! the assembly waited for the database writer per article, the most files and the most
//! resident memory it took.
//!
//! `RD_BENCH_FILES` and `RD_BENCH_ARTICLES` change the release (24 files of 12 articles of
//! 256 KiB by default: every file smaller than two request windows of ten connections, which
//! is the case automatic mode is for); `RD_BENCH_RTT_MS` and `RD_BENCH_RATE_MIB` the line.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

use rd_core::NzbFileStatus;
use rd_db::Database;
use tokio_util::sync::CancellationToken;

use super::{CONNECTIONS, DEFAULT_RATE_MIB, DEFAULT_RTT_MS, setting};
use crate::{
    NntpPool, NntpServerConfig,
    parallel::FileLoad,
    test_support::{
        Articles, FixtureTiming, import_single_file, multipart_article, payload, run_limits,
        spawn_fixture,
    },
    worker::{FileOutcome, download_file_counted},
};

const SWEEP_FILES: u64 = 24;
const SWEEP_ARTICLES: u64 = 12;
const SWEEP_ARTICLE_BYTES: usize = 256 * 1024;
/// The scheduler's dispatch interval (`SchedulerHandle::supervise`).
const DISPATCH_TICK: Duration = Duration::from_millis(500);
const SAMPLE: Duration = Duration::from_millis(100);

/// `0` is automatic, as in the setting.
const MODES: [usize; 5] = [1, 2, 3, 4, 0];

#[tokio::test(flavor = "multi_thread")]
#[ignore = "prints a table of throughput numbers; run on purpose, on a quiet machine"]
async fn parallel_files_sweep() {
    let files = usize::try_from(setting("RD_BENCH_FILES", SWEEP_FILES)).expect("file count");
    let per_file =
        usize::try_from(setting("RD_BENCH_ARTICLES", SWEEP_ARTICLES)).expect("article count");
    let rtt = Duration::from_millis(setting("RD_BENCH_RTT_MS", DEFAULT_RTT_MS));
    let rate_mib = setting("RD_BENCH_RATE_MIB", DEFAULT_RATE_MIB);
    let (articles, segments) = release(files, per_file);
    println!(
        "parallel_files_sweep: {files} files x {per_file} articles x {} KiB over {CONNECTIONS} connections, RTT {} ms, {rate_mib} MiB/s per connection (0 = unpaced)",
        SWEEP_ARTICLE_BYTES / 1024,
        rtt.as_millis(),
    );
    println!(
        "{:>5} | {:>7} | {:>7} | {:>7} | {:>18} | {:>17} | {:>5} | {:>8}",
        "files",
        "seconds",
        "MB/s",
        "spread",
        "idle/conn mean/max",
        "writer wait/art.",
        "peak",
        "peak RSS"
    );
    for mode in MODES {
        let result = pass(mode, &articles, &segments, rtt, rate_mib).await;
        let total_bytes = (files * per_file * SWEEP_ARTICLE_BYTES) as f64;
        let idle_ms: Vec<f64> = result
            .idle
            .iter()
            .map(|idle| idle.as_secs_f64() * 1000.0)
            .collect();
        let idle_mean = idle_ms.iter().sum::<f64>() / idle_ms.len().max(1) as f64;
        let idle_max = idle_ms.iter().copied().fold(0.0, f64::max);
        let (spread, samples) = coefficient_of_variation(&result.samples);
        let wait_per_article =
            result.writer_wait.as_secs_f64() * 1e6 / result.confirmed.max(1) as f64;
        println!(
            "{:>5} | {:>7.2} | {:>7.1} | {:>5.0} % | {:>8.0} / {:>6.0} ms | {:>6.0} us in {:>4} | {:>5} | {:>5} MiB",
            if mode == 0 {
                "auto".to_owned()
            } else {
                mode.to_string()
            },
            result.elapsed.as_secs_f64(),
            total_bytes / 1_000_000.0 / result.elapsed.as_secs_f64(),
            spread * 100.0,
            idle_mean,
            idle_max,
            wait_per_article,
            result.batches,
            result.peak_files,
            result
                .peak_resident_kib
                .map_or_else(|| "n/a".to_owned(), |kib| (kib / 1024).to_string()),
        );
        println!(
            "        ({samples} samples of {} ms; writer batches after the per-article wait)",
            SAMPLE.as_millis()
        );
    }
}

struct PassResult {
    elapsed: Duration,
    /// Bytes the fixture had sent at every sample.
    samples: Vec<u64>,
    idle: Vec<Duration>,
    writer_wait: Duration,
    batches: u64,
    confirmed: u64,
    peak_files: usize,
    /// Growth of the resident set over the pass; `None` where `/proc` does not exist.
    peak_resident_kib: Option<u64>,
}

type Segments = Vec<Vec<(String, u64)>>;

fn release(files: usize, per_file: usize) -> (Articles, Segments) {
    let total = (per_file * SWEEP_ARTICLE_BYTES) as u64;
    let mut articles = HashMap::new();
    let mut segments = Vec::with_capacity(files);
    for file in 0..files {
        let mut ids = Vec::with_capacity(per_file);
        for index in 0..per_file {
            let message_id = format!("sweep-{file}-{index}@bench.test");
            articles.insert(
                message_id.clone(),
                Some(multipart_article(
                    &format!("sweep-{file}.bin"),
                    (index + 1) as u64,
                    total,
                    (index * SWEEP_ARTICLE_BYTES) as u64 + 1,
                    &payload(SWEEP_ARTICLE_BYTES, file * 37 + index),
                )),
            );
            ids.push((message_id, SWEEP_ARTICLE_BYTES as u64));
        }
        segments.push(ids);
    }
    (Arc::new(articles), segments)
}

async fn pass(
    requested: usize,
    articles: &Articles,
    segments: &Segments,
    rtt: Duration,
    rate_mib: u64,
) -> PassResult {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = Database::open(directory.path().join("sweep.sqlite"))
        .await
        .expect("database");
    let mut queue = VecDeque::with_capacity(segments.len());
    for (file, ids) in segments.iter().enumerate() {
        queue.push_back(import_single_file(&database, &format!("sweep-{file}.bin"), ids).await);
    }
    let (address, log) = spawn_fixture(
        Arc::clone(articles),
        FixtureTiming {
            rtt,
            bytes_per_second: (rate_mib > 0).then_some(rate_mib * 1024 * 1024),
        },
    )
    .await;
    let pool = NntpPool::new(vec![NntpServerConfig {
        host: address.ip().to_string(),
        port: address.port(),
        tls: false,
        custom_ca_pem: Vec::new(),
        username: None,
        password: None,
        proxy: None,
        max_article_bytes: 4 * 1024 * 1024,
        max_connections: CONNECTIONS,
    }])
    .expect("pool");
    let staging = directory.path().join("staging");
    let destination = directory.path().join("destination");
    tokio::fs::create_dir_all(&staging).await.expect("staging");
    tokio::fs::create_dir_all(&destination)
        .await
        .expect("destination");
    let load = Arc::new(FileLoad::default());
    load.set_window(pool.max_parallel_requests());
    let baseline_kib = resident_kib();
    let sampling = CancellationToken::new();
    let sampler = tokio::spawn({
        let (log, sampling) = (Arc::clone(&log), sampling.clone());
        async move {
            let mut samples = Vec::new();
            let mut peak = resident_kib();
            let mut ticker = tokio::time::interval(SAMPLE);
            loop {
                tokio::select! {
                    () = sampling.cancelled() => return (samples, peak),
                    _ = ticker.tick() => {
                        samples.push(log.bytes_sent());
                        peak = peak.max(resident_kib());
                    }
                }
            }
        }
    });

    let started = Instant::now();
    let mut running: tokio::task::JoinSet<anyhow::Result<FileOutcome>> =
        tokio::task::JoinSet::new();
    let mut peak_files = 0_usize;
    let mut ticker = tokio::time::interval(DISPATCH_TICK);
    loop {
        ticker.tick().await;
        // A finished file gives its place back at the next pass, not the moment it ends.
        while let Some(finished) = running.try_join_next() {
            let outcome = finished.expect("file task").expect("download");
            assert!(matches!(outcome, FileOutcome::Completed { missing: 0, .. }));
        }
        if queue.is_empty() && running.is_empty() {
            break;
        }
        let capacity = load.capacity(requested);
        while running.len() < capacity
            && let Some(file) = queue.pop_front()
        {
            running.spawn(run_file(
                database.clone(),
                pool.clone(),
                file,
                staging.clone(),
                destination.clone(),
                Arc::clone(&load),
            ));
        }
        peak_files = peak_files.max(running.len());
    }
    let elapsed = started.elapsed();
    sampling.cancel();
    let (mut samples, peak) = sampler.await.expect("sampler");
    samples.push(log.bytes_sent());
    let (waited, batches, confirmed) = load.writer_totals();
    PassResult {
        elapsed,
        samples,
        idle: log.idle_per_connection(Instant::now()),
        writer_wait: Duration::from_nanos(waited),
        batches,
        confirmed,
        peak_files,
        peak_resident_kib: peak
            .zip(baseline_kib)
            .map(|(peak, baseline)| peak.saturating_sub(baseline)),
    }
}

async fn run_file(
    database: Database,
    pool: NntpPool,
    file: NzbFileStatus,
    staging: std::path::PathBuf,
    destination: std::path::PathBuf,
    load: Arc<FileLoad>,
) -> anyhow::Result<FileOutcome> {
    let open = load.start(file.segments.len());
    download_file_counted(
        &database,
        &pool,
        &CancellationToken::new(),
        &file,
        &staging,
        &destination,
        &run_limits(),
        &open,
    )
    .await
}

/// Standard deviation over mean of the per-sample throughput, and the number of samples.
///
/// The last sample is cut short by the end of the run and left out, and so is everything
/// before the first byte arrived: connection setup is the same in every mode.
fn coefficient_of_variation(cumulative: &[u64]) -> (f64, usize) {
    let rates: Vec<f64> = cumulative
        .windows(2)
        .map(|pair| pair[1].saturating_sub(pair[0]) as f64)
        .skip_while(|bytes| *bytes == 0.0)
        .collect();
    let rates = &rates[..rates.len().saturating_sub(1)];
    if rates.len() < 2 {
        return (0.0, rates.len());
    }
    let mean = rates.iter().sum::<f64>() / rates.len() as f64;
    let variance = rates.iter().map(|rate| (rate - mean).powi(2)).sum::<f64>() / rates.len() as f64;
    (
        if mean > 0.0 {
            variance.sqrt() / mean
        } else {
            0.0
        },
        rates.len(),
    )
}

/// The resident set of this process in KiB, where `/proc` says.
fn resident_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

#[test]
fn the_spread_ignores_the_start_and_the_cut_off_end() {
    // Nothing for two samples, then a steady 10 per sample, then a short last one.
    let (spread, samples) = coefficient_of_variation(&[0, 0, 0, 10, 20, 30, 40, 42]);
    assert_eq!(samples, 4);
    assert!(spread.abs() < f64::EPSILON);
    let (spread, _) = coefficient_of_variation(&[0, 10, 10, 20, 20, 25]);
    assert!(
        spread > 0.5,
        "a line that stops every other sample varies a lot"
    );
}
