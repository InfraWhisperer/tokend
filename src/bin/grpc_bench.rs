use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use clap::Parser;
use tokio::sync::mpsc;
use tonic::transport::Channel;

use tokend::payloads;

mod proto {
    tonic::include_proto!("tokend.v1");
}

use proto::tokenizer_service_client::TokenizerServiceClient;
use proto::{ChatMessage, ChatTokenizeRequest, HealthRequest, TokenizeRequest};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(
    name = "tokend-bench",
    about = "gRPC load generator for tokend (oha-style output)"
)]
struct Cli {
    /// tokend gRPC endpoint
    #[arg(short, long, default_value = "http://localhost:8766")]
    target: String,

    /// Model name to tokenize against
    #[arg(short, long, default_value = "meta-llama/Llama-3.1-70B-Instruct")]
    model: String,

    /// Concurrent client tasks
    #[arg(short, long, default_value_t = 32)]
    concurrency: u32,

    /// Total requests to send (0 = use --duration instead)
    #[arg(short = 'n', long, default_value_t = 0)]
    requests: u64,

    /// Test duration in seconds (ignored if --requests > 0)
    #[arg(short, long, default_value_t = 10)]
    duration: u64,

    /// Text payload size: short, medium, long
    #[arg(short = 's', long, default_value = "medium")]
    text_size: String,

    /// Number of texts per request (batch size)
    #[arg(short, long, default_value_t = 1)]
    batch_size: u32,

    /// Warmup requests before measurement begins
    #[arg(long, default_value_t = 50)]
    warmup: u64,

    /// Seconds to wait for server health before giving up
    #[arg(long, default_value_t = 60)]
    wait_ready: u64,

    /// Use ChatTokenize RPC instead of Tokenize
    #[arg(long, default_value_t = false)]
    chat: bool,

    /// Number of conversation turns in chat mode (system + N-1 user/assistant pairs)
    #[arg(long, default_value_t = 5)]
    turns: usize,

    /// Output format: text, json
    #[arg(long, default_value = "text")]
    output: String,
}

// ---------------------------------------------------------------------------
// Result collection
// ---------------------------------------------------------------------------

struct RequestResult {
    latency: Duration,
    token_count: u64,
    server_latency_us: u64,
    is_error: bool,
    status: String,
}

struct Stats {
    total_requests: u64,
    total_errors: u64,
    total_tokens: u64,
    elapsed: Duration,
    latencies: Vec<Duration>,
    server_latency_sum_us: u64,
    status_counts: Vec<(String, u64)>,
}

impl Stats {
    fn success_rate(&self) -> f64 {
        if self.total_requests == 0 {
            return 0.0;
        }
        (self.total_requests - self.total_errors) as f64 / self.total_requests as f64 * 100.0
    }

    fn requests_per_sec(&self) -> f64 {
        self.total_requests as f64 / self.elapsed.as_secs_f64()
    }

    fn tokens_per_sec(&self) -> f64 {
        self.total_tokens as f64 / self.elapsed.as_secs_f64()
    }

    fn percentile(&self, p: f64) -> Duration {
        if self.latencies.is_empty() {
            return Duration::ZERO;
        }
        let idx = ((p / 100.0) * (self.latencies.len() - 1) as f64).round() as usize;
        self.latencies[idx.min(self.latencies.len() - 1)]
    }

    fn mean(&self) -> Duration {
        if self.latencies.is_empty() {
            return Duration::ZERO;
        }
        let sum: Duration = self.latencies.iter().sum();
        sum / self.latencies.len() as u32
    }

    fn min(&self) -> Duration {
        self.latencies.first().copied().unwrap_or(Duration::ZERO)
    }

    fn max(&self) -> Duration {
        self.latencies.last().copied().unwrap_or(Duration::ZERO)
    }

    fn avg_server_latency_us(&self) -> f64 {
        if self.total_requests == 0 {
            return 0.0;
        }
        self.server_latency_sum_us as f64 / (self.total_requests - self.total_errors) as f64
    }
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

fn format_duration(d: Duration) -> String {
    let us = d.as_micros();
    if us < 1_000 {
        format!("{us} us")
    } else if us < 1_000_000 {
        format!("{:.3} ms", us as f64 / 1_000.0)
    } else {
        format!("{:.3} secs", d.as_secs_f64())
    }
}

fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

fn format_float(n: f64) -> String {
    // Format with 2 decimal places, then add thousand separators to the integer part
    let formatted = format!("{n:.2}");
    let parts: Vec<&str> = formatted.split('.').collect();
    let int_part = parts[0];
    let dec_part = parts[1];

    let mut result = String::new();
    for (i, c) in int_part.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 && c != '-' {
            result.push(',');
        }
        result.push(c);
    }
    let int_formatted: String = result.chars().rev().collect();
    format!("{int_formatted}.{dec_part}")
}

// ---------------------------------------------------------------------------
// Report output
// ---------------------------------------------------------------------------

fn print_text_report(stats: &Stats, cli: &Cli) {
    println!("tokend-bench — gRPC load generator for tokend\n");

    // Summary
    println!("Summary:");
    println!("  Success rate:  {:.2}%", stats.success_rate());
    println!("  Total:         {:.3} secs", stats.elapsed.as_secs_f64());
    println!("  Slowest:       {}", format_duration(stats.max()));
    println!("  Fastest:       {}", format_duration(stats.min()));
    println!("  Average:       {}", format_duration(stats.mean()));
    println!(
        "  Requests/sec:  {}",
        format_float(stats.requests_per_sec())
    );
    println!("  Tokens/sec:    {}", format_float(stats.tokens_per_sec()));
    println!();
    println!("  Model:         {}", cli.model);
    if cli.chat {
        println!(
            "  Chat size:     {}",
            payloads::chat_size_label(&cli.text_size)
        );
        println!("  Turns:         {}", cli.turns);
    } else {
        println!(
            "  Text size:     {}",
            payloads::text_size_label(&cli.text_size)
        );
        println!("  Batch size:    {}", cli.batch_size);
    }
    println!("  Concurrency:   {}", cli.concurrency);
    println!();

    // Latency histogram
    if !stats.latencies.is_empty() {
        print_histogram(stats);
        println!();
    }

    // Latency distribution
    if !stats.latencies.is_empty() {
        println!("Latency distribution:");
        for p in [10.0, 25.0, 50.0, 75.0, 90.0, 95.0, 99.0, 99.9, 99.99] {
            println!("  {:>6.2}% in {}", p, format_duration(stats.percentile(p)));
        }
        println!();
    }

    // Status distribution
    println!("Status distribution:");
    for (status, count) in &stats.status_counts {
        println!(
            "  [{status}] {:>width$} responses",
            format_number(*count),
            width = 10
        );
    }
    println!();

    // Server-reported latency
    let successful = stats.total_requests - stats.total_errors;
    if successful > 0 {
        println!("Details (server-reported):");
        println!(
            "  Avg tokenize latency:  {:.3} ms (server-side only)",
            stats.avg_server_latency_us() / 1_000.0
        );
    }
}

fn print_histogram(stats: &Stats) {
    const BUCKETS: usize = 10;
    const BAR_WIDTH: usize = 32;

    let min_us = stats.min().as_micros() as f64;
    let max_us = stats.max().as_micros() as f64;

    if (max_us - min_us) < 1.0 {
        // All latencies are the same; nothing interesting to plot
        return;
    }

    let step = (max_us - min_us) / BUCKETS as f64;
    let mut counts = [0u64; BUCKETS];

    for lat in &stats.latencies {
        let us = lat.as_micros() as f64;
        let bucket = ((us - min_us) / step).floor() as usize;
        let bucket = bucket.min(BUCKETS - 1);
        counts[bucket] += 1;
    }

    let max_count = *counts.iter().max().unwrap_or(&1);

    println!("Latency histogram:");
    for (i, &count) in counts.iter().enumerate() {
        let bucket_start_us = min_us + (i as f64 * step);
        let bar_len = if max_count > 0 {
            (count as f64 / max_count as f64 * BAR_WIDTH as f64).round() as usize
        } else {
            0
        };
        let bar: String = "■".repeat(bar_len);

        // Format the bucket start as a duration
        let bucket_dur = Duration::from_micros(bucket_start_us as u64);
        println!(
            "  {:<10} [{:>7}] |{bar}",
            format_duration(bucket_dur),
            format_number(count),
        );
    }
}

fn print_json_report(stats: &Stats, cli: &Cli) {
    let status_map: serde_json::Map<String, serde_json::Value> = stats
        .status_counts
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::Value::Number((*v).into())))
        .collect();

    let report = serde_json::json!({
        "target": cli.target,
        "model": cli.model,
        "text_size": cli.text_size,
        "batch_size": cli.batch_size,
        "concurrency": cli.concurrency,
        "success_rate_pct": stats.success_rate(),
        "total_secs": stats.elapsed.as_secs_f64(),
        "total_requests": stats.total_requests,
        "total_errors": stats.total_errors,
        "total_tokens": stats.total_tokens,
        "requests_per_sec": stats.requests_per_sec(),
        "tokens_per_sec": stats.tokens_per_sec(),
        "latency": {
            "min_us": stats.min().as_micros() as u64,
            "max_us": stats.max().as_micros() as u64,
            "mean_us": stats.mean().as_micros() as u64,
            "p10_us": stats.percentile(10.0).as_micros() as u64,
            "p25_us": stats.percentile(25.0).as_micros() as u64,
            "p50_us": stats.percentile(50.0).as_micros() as u64,
            "p75_us": stats.percentile(75.0).as_micros() as u64,
            "p90_us": stats.percentile(90.0).as_micros() as u64,
            "p95_us": stats.percentile(95.0).as_micros() as u64,
            "p99_us": stats.percentile(99.0).as_micros() as u64,
            "p999_us": stats.percentile(99.9).as_micros() as u64,
            "p9999_us": stats.percentile(99.99).as_micros() as u64,
        },
        "server_avg_latency_us": stats.avg_server_latency_us(),
        "status_distribution": status_map,
    });

    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}

// ---------------------------------------------------------------------------
// Core benchmark logic
// ---------------------------------------------------------------------------

async fn wait_for_health(
    client: &mut TokenizerServiceClient<Channel>,
    timeout: Duration,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(resp) = client.health(HealthRequest {}).await
            && resp.into_inner().serving
        {
            return Ok(());
        }
        if Instant::now() > deadline {
            bail!("server did not become healthy within {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn run_warmup(
    client: &mut TokenizerServiceClient<Channel>,
    count: u64,
    request: &TokenizeRequest,
) {
    for _ in 0..count {
        let _ = client.tokenize(request.clone()).await;
    }
}

/// Build a synthetic multi-turn conversation from the text payload.
fn build_chat_messages(text: &str, turns: usize) -> Vec<ChatMessage> {
    let mut messages = Vec::with_capacity(turns);
    messages.push(ChatMessage {
        role: "system".into(),
        content: Some("You are a helpful assistant.".into()),
        tool_calls: vec![],
        tool_call_id: None,
        name: None,
    });

    for i in 1..turns {
        let role = if i % 2 == 1 { "user" } else { "assistant" };
        // Vary content slightly per turn to avoid trivial caching
        let content = if text.len() > 100 {
            let offset = (i * 37) % (text.len() / 2);
            let end = (offset + text.len() / 2).min(text.len());
            &text[offset..end]
        } else {
            text
        };
        messages.push(ChatMessage {
            role: role.into(),
            content: Some(content.into()),
            tool_calls: vec![],
            tool_call_id: None,
            name: None,
        });
    }

    messages
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let text = payloads::text_for_size(&cli.text_size).with_context(|| {
        format!(
            "unknown text size '{}'; use short/medium/long",
            cli.text_size
        )
    })?;

    let texts: Vec<String> =
        std::iter::repeat_n(text.to_string(), cli.batch_size as usize).collect();

    let tokenize_request = TokenizeRequest {
        model: cli.model.clone(),
        texts,
        add_special_tokens: true,
        return_tokens: false,
    };

    // Build chat request if --chat mode
    let chat_request = if cli.chat {
        let messages = build_chat_messages(text, cli.turns);
        Some(ChatTokenizeRequest {
            model: cli.model.clone(),
            messages,
            add_generation_prompt: true,
            return_tokens: false,
        })
    } else {
        None
    };

    // Lazy channel — defers TCP connect to the first RPC so the health-check
    // retry loop can absorb startup delay while tokend loads tokenizers.
    eprintln!("Connecting to {}...", cli.target);
    let channel = Channel::from_shared(cli.target.clone())
        .context("invalid target URL")?
        .connect_lazy();

    let mut health_client = TokenizerServiceClient::new(channel.clone());
    eprintln!(
        "Waiting for server health (timeout: {}s)...",
        cli.wait_ready
    );
    wait_for_health(&mut health_client, Duration::from_secs(cli.wait_ready)).await?;
    eprintln!("Server is healthy.");

    // Warmup
    if cli.warmup > 0 {
        eprintln!("Warming up ({} requests)...", cli.warmup);
        let mut warmup_client = TokenizerServiceClient::new(channel.clone());
        run_warmup(&mut warmup_client, cli.warmup, &tokenize_request).await;
    }

    let use_chat = cli.chat;

    // Prepare shared state for workers
    let done = Arc::new(AtomicBool::new(false));
    let request_budget = Arc::new(AtomicU64::new(if cli.requests > 0 {
        cli.requests
    } else {
        u64::MAX
    }));

    let (tx, mut rx) = mpsc::unbounded_channel::<RequestResult>();

    let use_duration = cli.requests == 0;
    let test_duration = Duration::from_secs(cli.duration);

    eprintln!(
        "Running benchmark: {} concurrency, {}...",
        cli.concurrency,
        if use_duration {
            format!("{}s duration", cli.duration)
        } else {
            format!("{} requests", cli.requests)
        }
    );

    let wall_start = Instant::now();

    // If duration-based, spawn a timer to signal done
    if use_duration {
        let done_timer = done.clone();
        tokio::spawn(async move {
            tokio::time::sleep(test_duration).await;
            done_timer.store(true, Ordering::Release);
        });
    }

    // Spawn worker tasks
    let mut handles = Vec::new();
    for _ in 0..cli.concurrency {
        let channel = channel.clone();
        let tok_req = tokenize_request.clone();
        let chat_req = chat_request.clone();
        let done = done.clone();
        let budget = request_budget.clone();
        let tx = tx.clone();

        handles.push(tokio::spawn(async move {
            let mut client = TokenizerServiceClient::new(channel);
            loop {
                if done.load(Ordering::Acquire) {
                    break;
                }

                let prev = budget.fetch_sub(1, Ordering::AcqRel);
                if prev == 0 {
                    budget.fetch_add(1, Ordering::Release);
                    done.store(true, Ordering::Release);
                    break;
                }

                let start = Instant::now();

                let (token_count, server_latency_us, is_error, status) = if use_chat {
                    match client.chat_tokenize(chat_req.clone().unwrap()).await {
                        Ok(resp) => {
                            let inner = resp.into_inner();
                            (
                                inner.token_count as u64,
                                inner.latency_us,
                                false,
                                "OK".to_string(),
                            )
                        }
                        Err(e) => (0, 0, true, e.code().to_string()),
                    }
                } else {
                    match client.tokenize(tok_req.clone()).await {
                        Ok(resp) => {
                            let inner = resp.into_inner();
                            let tokens: u64 =
                                inner.results.iter().map(|r| r.token_count as u64).sum();
                            (tokens, inner.latency_us, false, "OK".to_string())
                        }
                        Err(e) => (0, 0, true, e.code().to_string()),
                    }
                };

                let latency = start.elapsed();

                let _ = tx.send(RequestResult {
                    latency,
                    token_count,
                    server_latency_us,
                    is_error,
                    status,
                });
            }
        }));
    }

    // Drop our sender so rx completes when all workers finish
    drop(tx);

    // Collect results
    let mut results = Vec::new();
    while let Some(r) = rx.recv().await {
        results.push(r);
    }

    // Wait for all workers
    for h in handles {
        let _ = h.await;
    }

    let wall_elapsed = wall_start.elapsed();

    // Compute stats
    let mut latencies: Vec<Duration> = results.iter().map(|r| r.latency).collect();
    latencies.sort();

    let total_errors: u64 = results.iter().filter(|r| r.is_error).count() as u64;
    let total_tokens: u64 = results.iter().map(|r| r.token_count).sum();
    let server_latency_sum_us: u64 = results.iter().map(|r| r.server_latency_us).sum();

    // Aggregate status counts
    let mut status_map = std::collections::BTreeMap::new();
    for r in &results {
        *status_map.entry(r.status.clone()).or_insert(0u64) += 1;
    }
    let status_counts: Vec<(String, u64)> = status_map.into_iter().collect();

    let stats = Stats {
        total_requests: results.len() as u64,
        total_errors,
        total_tokens,
        elapsed: wall_elapsed,
        latencies,
        server_latency_sum_us,
        status_counts,
    };

    // Output
    match cli.output.as_str() {
        "json" => print_json_report(&stats, &cli),
        _ => print_text_report(&stats, &cli),
    }

    Ok(())
}
