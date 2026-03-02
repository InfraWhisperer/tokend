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

// ---------------------------------------------------------------------------
// Chat conversation payloads for benchmarking chat template + tokenize
// ---------------------------------------------------------------------------

use crate::chat_template::ChatMessage;

fn msg(role: &str, content: &str) -> ChatMessage {
    ChatMessage {
        role: role.to_string(),
        content: Some(content.to_string()),
        tool_calls: None,
        tool_call_id: None,
        name: None,
    }
}

/// 2-turn conversation: system + user. ~50 tokens after template application.
pub fn chat_short() -> Vec<ChatMessage> {
    vec![
        msg("system", "You are a helpful assistant."),
        msg("user", TEXT_SHORT),
    ]
}

/// 5-turn conversation. ~500 tokens after template application.
pub fn chat_medium() -> Vec<ChatMessage> {
    vec![
        msg(
            "system",
            "You are a helpful assistant specializing in distributed systems.",
        ),
        msg("user", "Explain how Kubernetes scheduling works."),
        msg("assistant", TEXT_MEDIUM),
        msg("user", "How does the HPA interact with custom metrics?"),
        msg(
            "assistant",
            "HPA queries the metrics API server on each reconciliation loop, comparing \
             the current metric value against the target. When the ratio exceeds 1.0, \
             it scales up by ceil(currentReplicas * ratio). The metrics pipeline \
             typically flows from the application through Prometheus, then the \
             prometheus-adapter exposes them as custom.metrics.k8s.io resources.",
        ),
    ]
}

/// 20-turn conversation. ~3000 tokens after template application.
pub fn chat_long() -> Vec<ChatMessage> {
    vec![
        msg(
            "system",
            "You are an expert in LLM inference infrastructure.",
        ),
        msg("user", "What is disaggregated LLM serving?"),
        msg("assistant", &TEXT_LONG[..600]),
        msg("user", "How does prefix-cache-aware routing work?"),
        msg("assistant", &TEXT_LONG[600..1200]),
        msg(
            "user",
            "What about KV-cache coordination in split-brain scenarios?",
        ),
        msg("assistant", &TEXT_LONG[1200..1800]),
        msg(
            "user",
            "How does attention kernel selection depend on sequence length?",
        ),
        msg("assistant", &TEXT_LONG[1800..]),
        msg(
            "user",
            "Can you elaborate on the observability requirements?",
        ),
        msg("assistant", TEXT_MEDIUM),
        msg("user", "How does the scheduler assign pods to nodes?"),
        msg(
            "assistant",
            "The scheduler runs a two-phase pipeline: filtering and scoring. \
             Filtering removes nodes that don't meet hard constraints (resource \
             requests, taints, affinity rules). Scoring ranks remaining candidates \
             using weighted plugins — ImageLocality, NodeAffinity, PodTopologySpread, \
             and others. The highest-scoring node wins.",
        ),
        msg("user", "What about topology-aware scheduling?"),
        msg(
            "assistant",
            "TopologySpreadConstraints let you distribute pods across failure domains \
             (zones, racks, nodes) with configurable maxSkew. The scheduler enforces \
             these during both filtering and scoring phases, preventing hot spots \
             while respecting the skew budget you define.",
        ),
        msg("user", "How does VPA interact with resource limits?"),
        msg(
            "assistant",
            "VPA observes container resource usage over a rolling window (typically 8 \
             days) and recommends target, lower-bound, and upper-bound values. In Auto \
             mode it evicts pods to apply new requests/limits. The recommendation is \
             capped by any LimitRange in the namespace and bounded by the container's \
             max allowed resource policy.",
        ),
        msg("user", "What are the tradeoffs between HPA and VPA?"),
        msg(
            "assistant",
            "HPA scales horizontally (more replicas) based on metrics, while VPA scales \
             vertically (bigger containers). Running both on the same resource (CPU/memory) \
             creates a feedback loop — HPA adds replicas, reducing per-pod utilization, \
             causing VPA to shrink them. The standard pattern is HPA on CPU and VPA on \
             memory, or using multidimensional pod autoscaler (MPA) to coordinate both.",
        ),
        msg("user", "Summarize the key takeaways."),
    ]
}

/// Human-readable label for a chat size.
pub fn chat_size_label(size: &str) -> &'static str {
    match size {
        "short" => "short (2 turns, ~50 tokens)",
        "medium" => "medium (5 turns, ~500 tokens)",
        "long" => "long (20 turns, ~3000 tokens)",
        _ => "unknown",
    }
}

/// Return a chat payload for a given size name.
pub fn chat_for_size(size: &str) -> Option<Vec<ChatMessage>> {
    match size {
        "short" => Some(chat_short()),
        "medium" => Some(chat_medium()),
        "long" => Some(chat_long()),
        _ => None,
    }
}

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
