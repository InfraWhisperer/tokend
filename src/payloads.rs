//! Canonical benchmark text payloads at three size points.
//!
//! Reused across the criterion in-process bench (`benches/tokenize.rs`)
//! and the gRPC load generator (`src/bin/grpc_bench.rs`).

/// ~20 tokens after BPE.
pub const TEXT_SHORT: &str = "The quick brown fox jumps over the lazy dog near the riverbank.";

/// ~200 tokens — a paragraph of technical prose.
pub const TEXT_MEDIUM: &str = "\
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

/// ~2000 tokens — extended technical content.
pub const TEXT_LONG: &str = "\
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

/// Human-readable label for a text size.
pub fn text_size_label(size: &str) -> &'static str {
    match size {
        "short" => "short (~20 tokens)",
        "medium" => "medium (~200 tokens)",
        "long" => "long (~2000 tokens)",
        _ => "unknown",
    }
}

/// Return the text payload for a given size name.
pub fn text_for_size(size: &str) -> Option<&'static str> {
    match size {
        "short" => Some(TEXT_SHORT),
        "medium" => Some(TEXT_MEDIUM),
        "long" => Some(TEXT_LONG),
        _ => None,
    }
}
