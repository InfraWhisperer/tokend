# tokend

Multi-model tokenizer daemon for LLM inference infrastructure.

---

## What is tokend?

Inference gateways need token IDs before they can route: prefix-cache-aware routing,
token-based rate limiting, and request costing all require tokenization upstream of the
inference backend. tokend is a standalone process that tokenizes text and returns token
IDs over HTTP, gRPC, and Unix domain sockets — decoupled from any specific inference
engine.

## Features

- **Multi-model** — load any number of HuggingFace or local tokenizers concurrently
- **Chat template support** — apply model-specific chat templates (Jinja2/minijinja) before tokenization
- **Envoy ext_proc** — transparent tokenization as an Envoy external processor; inject token counts and IDs into requests in-band
- **Four transports** — HTTP over TCP, HTTP over UDS (sidecar-optimized), gRPC, Envoy ext_proc
- **Hot-load / unload** — add or remove tokenizers at runtime without restart
- **Sub-millisecond latency** — lock-minimized `DashMap` registry, `Arc<Tokenizer>` zero-copy on hot path
- **Batch support** — tokenize multiple texts in a single request
- **HuggingFace Hub** — download tokenizers and chat templates on first use, cache to disk, run offline thereafter
- **Prometheus metrics** — latency histograms, token counters, request counters, loaded-model gauge
- **Health / readiness probes** — Kubernetes-native liveness and readiness endpoints
- **Sidecar-ready** — designed to run alongside Envoy or any inference gateway

## Quickstart

### Build from source

Requirements: Rust 1.85+ (edition 2024), protoc 3.x.

```bash
git clone https://github.com/your-org/tokend && cd tokend
cargo build --release
```

### Run

```bash
export HF_TOKEN=hf_...
./target/release/tokend -c tokend.yaml serve
```

### Docker

```bash
docker run --rm \
  -e HF_TOKEN=hf_... \
  -v $(pwd)/tokend.yaml:/etc/tokend/tokend.yaml \
  -p 8765:8765 -p 8766:8766 \
  ghcr.io/your-org/tokend:latest
```

### Tokenize

```bash
# Raw tokenization
curl -s -X POST http://localhost:8765/tokenize \
  -H 'Content-Type: application/json' \
  -d '{"model": "meta-llama/Llama-3.1-70B-Instruct", "text": "Hello world"}' | jq .

# Chat template + tokenize
curl -s -X POST http://localhost:8765/v1/chat/tokenize \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "Qwen/Qwen3-8B",
    "messages": [
      {"role": "user", "content": "Explain KV caching."}
    ]
  }' | jq .
```

### Envoy ext_proc

tokend can run as an Envoy external processor, intercepting `/v1/chat/completions` and
`/v1/completions` requests to inject token counts and IDs transparently. See
[examples/](examples/) for a Docker Compose demo with Envoy.

```yaml
ext_proc:
  enabled: true
  port: 8767
  mode: "both"          # "headers", "body", or "both"
  inject_tokens: true   # inject token_ids array into body
```

## Documentation

- [API Reference](docs/API.md) — HTTP endpoints, gRPC RPCs, ext_proc, configuration, CLI, metrics
- [Design](docs/DESIGN.md) — architecture, registry internals, ext_proc protocol, deployment patterns
- [Envoy ext_proc Demo](examples/README.md) — Docker Compose setup with Envoy

## Contributing

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/my-feature`)
3. Make your changes
4. Run checks:

   ```bash
   cargo check
   cargo test
   cargo clippy -- -D warnings
   ```

5. Commit and open a pull request

## License

MIT
