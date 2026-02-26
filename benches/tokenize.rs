use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::path::{Path, PathBuf};
use tokend::config::TokenizerSource;
use tokend::tokenizer::TokenizerRegistry;

const BENCH_TOKENIZER_DIR: &str = "/tmp/tokend-bench";
const BENCH_TOKENIZER_PATH: &str = "/tmp/tokend-bench/tokenizer.json";
const MODEL_NAME: &str = "bench-bert-base-cased";

// ~20 tokens
const TEXT_SHORT: &str = "The quick brown fox jumps over the lazy dog near the riverbank.";

// ~200 tokens — a paragraph of technical prose
const TEXT_MEDIUM: &str = "\
Kubernetes implements a declarative control loop: the API server stores \
desired state in etcd, and controllers continuously reconcile observed state \
toward it. The scheduler assigns pods to nodes by scoring candidates against \
resource requests, affinity rules, and topology constraints. kubelet on each \
node runs pod sandboxes via the CRI, mounts volumes via the CSI, and wires \
networking via the CNI. Health probes drive readiness and liveness gating; \
failing liveness restarts the container while failing readiness pulls it from \
load-balancer endpoints. HPA adjusts replica count against custom metrics \
exposed through the metrics pipeline, while VPA right-sizes container \
resource limits based on observed usage histograms collected over a rolling \
window.";

// ~2000 tokens — extended technical content repeated to reach target length
const TEXT_LONG: &str = "\
Disaggregated LLM serving separates the prefill and decode phases onto \
distinct worker pools, exploiting the asymmetry between the two: prefill is \
compute-bound and parallelizes well across prompt tokens, whereas decode is \
memory-bandwidth-bound and benefits from batching many in-flight sequences. \
Routing the initial prefill request to a prefill worker, then migrating the \
populated KV-cache entries to a decode worker via RDMA or NVLink fabric, \
allows each pool to be sized and scheduled independently. This enables \
heterogeneous clusters where high-FLOPS GPUs handle prefill and \
high-HBM-bandwidth GPUs handle decode without stranding capacity on either. \
\n\
Prefix-cache-aware routing extends this by hashing the prompt prefix and \
directing requests to the worker that already holds matching KV entries in \
its GPU SRAM. A hit avoids recomputing the prefix entirely, trading a \
network round-trip for O(n) attention work. The routing layer maintains a \
distributed prefix-hash index — either a consistent-hash ring or a \
centralized coordinator — and falls back to the least-loaded worker on a \
miss. Cache eviction follows LRU with capacity limits derived from available \
HBM, and invalidation is triggered on model weight updates or context-window \
exhaustion. \
\n\
The KV-cache coordination plane must handle split-brain scenarios where \
multiple workers believe they hold the authoritative copy of an in-flight \
sequence's state. Fencing tokens issued at sequence-creation time and \
incremented on each migration prevent stale workers from committing \
speculative decode steps after a handoff. Workers that fail to renew their \
fencing token within the heartbeat window are preempted and their sequences \
are rescheduled. The sequence-state journal, replicated to at least two \
workers before acknowledgement, provides durability against single-node \
failure without sacrificing the sub-millisecond decode latency budget. \
\n\
Attention kernel selection is a function of sequence length, batch size, and \
hardware capability. For long-context sequences where the KV cache exceeds \
L2, paged attention kernels amortize HBM bandwidth by fetching only the \
pages referenced in the current decode step. Flash-attention variants \
prefetch the next page asynchronously while the CUDA cores are occupied with \
the current GEMM, hiding memory latency behind compute. Dedicated \
accelerator cores can offload prefix-hash lookups and cache-line fetches, \
reducing PCIe traffic on the hot path. NCCL AllReduce is replaced \
by NIXL point-to-point transfers for KV-cache migration between tensor \
parallel ranks, avoiding the barrier synchronization overhead that would \
otherwise serialize the decode pipeline. \
\n\
Observability in this architecture requires per-sequence tracing that spans \
the prefill and decode workers, capturing enqueue time, time-to-first-token, \
per-step decode latency, cache-hit ratio, and migration duration. The trace \
context is propagated in the sequence metadata alongside the fencing token, \
allowing a distributed trace collector to reconstruct the full request \
timeline across the heterogeneous fleet. Anomaly detection on the \
time-to-first-token distribution identifies pathological prompt patterns that \
defeat prefix caching and warrant rate limiting or prompt rewriting at the \
gateway layer. SLO burn-rate alerts derived from the error budget framework \
fire when p99 decode latency exceeds the agreed envelope, triggering \
autoscaler events that provision additional decode capacity within the \
cluster before the budget is exhausted.";

/// Attempt to provision a tokenizer at BENCH_TOKENIZER_PATH.
///
/// Strategy:
///   1. Return immediately if the file already exists.
///   2. Try `Tokenizer::from_pretrained("bert-base-cased", None)` and save to disk.
///   3. If that fails (no network, no HF token), return None and callers skip gracefully.
fn ensure_tokenizer() -> Option<PathBuf> {
    let path = PathBuf::from(BENCH_TOKENIZER_PATH);

    if path.exists() {
        return Some(path);
    }

    eprintln!(
        "[bench] tokenizer not found at {BENCH_TOKENIZER_PATH}; \
         attempting to download bert-base-cased from HuggingFace Hub"
    );

    let dir = Path::new(BENCH_TOKENIZER_DIR);
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("[bench] failed to create {BENCH_TOKENIZER_DIR}: {e}; skipping benchmarks");
        return None;
    }

    match tokenizers::Tokenizer::from_pretrained("bert-base-cased", None) {
        Ok(tok) => {
            if let Err(e) = tok.save(&path, false) {
                eprintln!("[bench] failed to save tokenizer to {BENCH_TOKENIZER_PATH}: {e}; skipping benchmarks");
                return None;
            }
            eprintln!("[bench] tokenizer saved to {BENCH_TOKENIZER_PATH}");
            Some(path)
        }
        Err(e) => {
            eprintln!(
                "[bench] from_pretrained failed: {e}\n\
                 [bench] To run benchmarks, place a tokenizer.json at {BENCH_TOKENIZER_PATH}"
            );
            None
        }
    }
}

/// Build a registry with the bench tokenizer pre-loaded under MODEL_NAME.
/// Returns None if no tokenizer is available.
fn build_registry() -> Option<TokenizerRegistry> {
    let tokenizer_path = ensure_tokenizer()?;

    let registry = TokenizerRegistry::new(Path::new(BENCH_TOKENIZER_DIR));
    registry
        .load(
            MODEL_NAME,
            &TokenizerSource::Local,
            Some(tokenizer_path.to_str().unwrap()),
            None,
        )
        .expect("loading local tokenizer must not fail once the file exists");

    Some(registry)
}

// ---------------------------------------------------------------------------
// Single-text benchmarks
// ---------------------------------------------------------------------------

fn bench_single(c: &mut Criterion) {
    let registry = match build_registry() {
        Some(r) => r,
        None => {
            eprintln!("[bench] skipping single-text benchmarks: no tokenizer available");
            return;
        }
    };

    let cases: &[(&str, &str)] = &[
        ("short (~20 tokens)", TEXT_SHORT),
        ("medium (~200 tokens)", TEXT_MEDIUM),
        ("long (~2000 tokens)", TEXT_LONG),
    ];

    let mut group = c.benchmark_group(MODEL_NAME);

    for (label, text) in cases {
        group.throughput(Throughput::Bytes(text.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("single", label),
            text,
            |b, &input| {
                b.iter(|| {
                    registry
                        .tokenize(
                            black_box(MODEL_NAME),
                            black_box(&[input]),
                            /*add_special_tokens=*/ true,
                            /*return_tokens=*/ false,
                        )
                        .expect("tokenize must not fail")
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Batch benchmarks
// ---------------------------------------------------------------------------

fn bench_batch(c: &mut Criterion) {
    let registry = match build_registry() {
        Some(r) => r,
        None => {
            eprintln!("[bench] skipping batch benchmarks: no tokenizer available");
            return;
        }
    };

    let batch_10: Vec<&str> = std::iter::repeat(TEXT_SHORT).take(10).collect();
    let batch_100: Vec<&str> = std::iter::repeat(TEXT_SHORT).take(100).collect();

    let cases: &[(&str, &[&str])] = &[
        ("batch-10 short", &batch_10),
        ("batch-100 short", &batch_100),
    ];

    let mut group = c.benchmark_group(MODEL_NAME);

    for (label, texts) in cases {
        // Throughput in terms of number of sequences so criterion reports seqs/sec.
        group.throughput(Throughput::Elements(texts.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("batch", label),
            texts,
            |b, &input| {
                b.iter(|| {
                    registry
                        .tokenize(
                            black_box(MODEL_NAME),
                            black_box(input),
                            /*add_special_tokens=*/ true,
                            /*return_tokens=*/ false,
                        )
                        .expect("tokenize must not fail")
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_single, bench_batch);
criterion_main!(benches);
