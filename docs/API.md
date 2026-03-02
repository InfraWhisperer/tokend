# tokend API Reference

## Configuration

tokend reads a single YAML file. The path defaults to `tokend.yaml` and is overridden
with `-c <path>`. Environment variables in `${VAR}` syntax expand at parse time;
unset variables expand to empty string.

### Full reference

```yaml
# List of tokenizers to load at startup. At least one entry is expected
# for /ready to return 200. Additional tokenizers can be loaded at runtime
# via POST /tokenizers/load.
tokenizers:
  - model: "meta-llama/Llama-3.1-70B-Instruct"  # required; HuggingFace repo ID or local alias
    source: "huggingface"                          # "huggingface" (default) | "local"
    # path: not used for huggingface source

  - model: "my-local-model"
    source: "local"
    path: "/models/my-local-model/tokenizer.json" # required when source is "local"

server:
  uds_path: "/var/run/tokend.sock"  # Unix domain socket path; default: /var/run/tokend.sock
  http_port: 8765                   # TCP port for HTTP; default: 8765
  grpc_port: 8766                   # TCP port for gRPC; default: 8766

# Envoy external processor configuration. When enabled, tokend runs a gRPC
# service that Envoy's ext_proc filter connects to for in-band tokenization.
ext_proc:
  enabled: false                    # default: false
  port: 8767                        # ext_proc gRPC listen port; default: 8767
  intercept_paths:                  # request paths to intercept and tokenize
    - "/v1/chat/completions"        # default paths
    - "/chat/completions"
    - "/v1/completions"
    - "/completions"
  mode: "headers"                   # "headers", "body", or "both"; default: "headers"
  token_count_header: "x-tokend-token-count"  # default
  model_header: "x-tokend-model"              # default
  body_field: "token_count"         # JSON field for token count in body mode; default
  inject_tokens: false              # inject token_ids array into body; default: false
  token_ids_field: "token_ids"      # JSON field for token IDs array; default

# Tokenizer disk cache. Downloaded HuggingFace tokenizers are written here
# and read on subsequent starts. Supports ~ expansion.
# Default: ~/.cache/tokend (falls back to /tmp/tokend/cache if $HOME is unset)
cache_dir: "~/.cache/tokend"

# HuggingFace API token for gated models (Llama, Gemma, etc.).
# Typically set via environment variable substitution rather than a literal value.
# If the variable is unset or empty, unauthenticated requests are made.
hf_token: "${HF_TOKEN}"
```

### Field summary

| Field | Type | Default | Description |
|---|---|---|---|
| `tokenizers` | list | required | Models to pre-load at startup |
| `tokenizers[].model` | string | required | HuggingFace repo ID or local model name |
| `tokenizers[].source` | `huggingface` \| `local` | `huggingface` | Where to fetch the tokenizer |
| `tokenizers[].path` | string | — | Path to `tokenizer.json`; required for `source: local` |
| `server.uds_path` | string | `/var/run/tokend.sock` | Unix domain socket path |
| `server.http_port` | uint16 | `8765` | HTTP listen port (0.0.0.0) |
| `server.grpc_port` | uint16 | `8766` | gRPC listen port (0.0.0.0) |
| `ext_proc.enabled` | bool | `false` | Enable the Envoy ext_proc gRPC server |
| `ext_proc.port` | uint16 | `8767` | ext_proc gRPC listen port |
| `ext_proc.intercept_paths` | list | see above | Request paths to intercept for tokenization |
| `ext_proc.mode` | string | `"headers"` | Mutation mode: `"headers"`, `"body"`, or `"both"` |
| `ext_proc.token_count_header` | string | `x-tokend-token-count` | Header name for injected token count |
| `ext_proc.model_header` | string | `x-tokend-model` | Header name for resolved model |
| `ext_proc.body_field` | string | `token_count` | JSON field name for token count in body mutations |
| `ext_proc.inject_tokens` | bool | `false` | Inject full `token_ids` array into body mutations |
| `ext_proc.token_ids_field` | string | `token_ids` | JSON field name for the token IDs array |
| `cache_dir` | path | `~/.cache/tokend` | Disk cache for downloaded tokenizers |
| `hf_token` | string | — | HuggingFace token; `${ENV_VAR}` expansion supported |

---

## HTTP API

All endpoints bind on both TCP (`:8765`) and UDS (`/var/run/tokend.sock`).
Request and response bodies are JSON. Latency reported in response bodies is
wall-clock microseconds spent inside the tokenizer, excluding network overhead.

### POST /tokenize

Tokenize one or more texts. `text` accepts a single string or an array of strings.

**Request fields**

| Field | Type | Default | Description |
|---|---|---|---|
| `model` | string | required | Model name as registered in tokend |
| `text` | string or array | required | Text(s) to tokenize |
| `add_special_tokens` | bool | `true` | Prepend/append BOS/EOS tokens |
| `return_tokens` | bool | `false` | Include decoded token strings in response |

**Single text — flat response**

```bash
curl -s -X POST http://localhost:8765/tokenize \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "meta-llama/Llama-3.1-70B-Instruct",
    "text": "The quick brown fox",
    "add_special_tokens": true
  }' | jq .
```

```json
{
  "model": "meta-llama/Llama-3.1-70B-Instruct",
  "token_ids": [128000, 791, 4062, 14198, 39935],
  "token_count": 5,
  "latency_us": 42
}
```

**Single text with token strings**

```bash
curl -s -X POST http://localhost:8765/tokenize \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "meta-llama/Llama-3.1-70B-Instruct",
    "text": "Hello world",
    "return_tokens": true
  }' | jq .
```

```json
{
  "model": "meta-llama/Llama-3.1-70B-Instruct",
  "token_ids": [128000, 9906, 1917],
  "token_count": 3,
  "tokens": ["<|begin_of_text|>", "Hello", " world"],
  "latency_us": 38
}
```

**Batch — array response**

When `text` is an array, the response uses a `results` array instead of flat fields.
Each element maps positionally to the input.

```bash
curl -s -X POST http://localhost:8765/tokenize \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "Qwen/Qwen3-32B",
    "text": [
      "Summarize the following document:",
      "What is the capital of France?"
    ]
  }' | jq .
```

```json
{
  "model": "Qwen/Qwen3-32B",
  "results": [
    {
      "token_ids": [100279, 279, 2494, 4201, 510],
      "token_count": 5
    },
    {
      "token_ids": [3838, 374, 279, 6864, 315, 9822, 30],
      "token_count": 7
    }
  ],
  "latency_us": 61
}
```

**Error responses**

| Condition | HTTP status | Body |
|---|---|---|
| Model not loaded | 404 | `{"error": "model not loaded: <name>"}` |
| Tokenization failure | 500 | `{"error": "<message>"}` |

---

### POST /v1/chat/tokenize

Apply a model's chat template to a conversation, then tokenize the rendered prompt.
Automatically loads the chat template from the model's `tokenizer_config.json` on
HuggingFace Hub.

**Request fields**

| Field | Type | Default | Description |
|---|---|---|---|
| `model` | string | required | Model name as registered in tokend |
| `messages` | array | required | Chat messages (`role`, `content`) |
| `add_generation_prompt` | bool | `true` | Append the assistant turn start marker |
| `tools` | array | — | Tool definitions for function-calling templates |
| `add_special_tokens` | bool | `true` | Prepend/append BOS/EOS tokens |
| `return_tokens` | bool | `false` | Include decoded token strings in response |

**Example**

```bash
curl -s -X POST http://localhost:8765/v1/chat/tokenize \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "Qwen/Qwen3-8B",
    "messages": [
      {"role": "system", "content": "You are helpful."},
      {"role": "user", "content": "Explain KV caching."}
    ]
  }' | jq .
```

```json
{
  "model": "Qwen/Qwen3-8B",
  "token_count": 18,
  "token_ids": [151644, 872, 198, 6023, 151645, 198, 151644, 77091, 198, ...],
  "latency_us": 55,
  "render_us": 8
}
```

The `render_us` field reports time spent in Jinja2 template rendering, separate from
tokenization. This helps diagnose whether latency comes from template complexity or
tokenizer vocabulary size.

**Error responses**

| Condition | HTTP status | Body |
|---|---|---|
| Model not loaded | 404 | `{"error": "model not loaded: <name>"}` |
| Chat template not available | 400 | `{"error": "chat template not available for <model>"}` |
| Template render failure | 500 | `{"error": "<message>"}` |

---

### POST /tokenizers/load

Load a tokenizer at runtime without restarting.

**Request fields**

| Field | Type | Default | Description |
|---|---|---|---|
| `model` | string | required | HuggingFace repo ID or local alias |
| `source` | `"huggingface"` \| `"local"` | `"huggingface"` | Tokenizer source |
| `path` | string | — | Path to `tokenizer.json`; required for `source: "local"` |

```bash
curl -s -X POST http://localhost:8765/tokenizers/load \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "mistralai/Mistral-7B-Instruct-v0.3",
    "source": "huggingface"
  }' | jq .
```

```json
{
  "success": true,
  "message": "loaded mistralai/Mistral-7B-Instruct-v0.3"
}
```

Loading a local tokenizer:

```bash
curl -s -X POST http://localhost:8765/tokenizers/load \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "my-finetuned-llama",
    "source": "local",
    "path": "/models/my-finetuned-llama/tokenizer.json"
  }' | jq .
```

**Error responses**

| Condition | HTTP status | Body |
|---|---|---|
| Model already loaded | 409 | `{"error": "model already loaded: <name>"}` |
| Unknown source value | 400 | `{"error": "unknown source: <value>"}` |
| Download/parse failure | 500 | `{"error": "<message>"}` |

---

### DELETE /tokenizers/{model}

Unload a tokenizer and free its memory. Model names containing `/` must be
percent-encoded as `%2F`.

```bash
curl -s -X DELETE \
  'http://localhost:8765/tokenizers/mistralai%2FMistral-7B-Instruct-v0.3' | jq .
```

```json
{
  "success": true,
  "message": "unloaded mistralai/Mistral-7B-Instruct-v0.3"
}
```

**Error responses**

| Condition | HTTP status | Body |
|---|---|---|
| Model not loaded | 404 | `{"error": "model not loaded: <name>"}` |

---

### GET /health

Liveness probe. Returns 200 if the process is running. Does not check whether
any tokenizers are loaded.

```bash
curl -s http://localhost:8765/health | jq .
```

```json
{"status": "ok"}
```

---

### GET /ready

Readiness probe. Returns 200 when the number of loaded models meets or exceeds
the number declared in the config file. Returns 503 during startup (tokenizers
still loading from HuggingFace Hub) or if all tokenizer loads failed.

```bash
curl -s http://localhost:8765/ready | jq .
```

200 (ready):
```json
{
  "ready": true,
  "loaded_models": 3,
  "expected_models": 3
}
```

503 (not ready):
```json
{
  "ready": false,
  "loaded_models": 1,
  "expected_models": 3
}
```

---

### GET /metrics

Prometheus text exposition format (content-type `text/plain; version=0.0.4`).

```bash
curl -s http://localhost:8765/metrics
```

```
# HELP tokend_tokenize_latency_us Tokenization latency in microseconds
# TYPE tokend_tokenize_latency_us histogram
tokend_tokenize_latency_us_bucket{model="meta-llama/Llama-3.1-70B-Instruct",le="10"} 0
tokend_tokenize_latency_us_bucket{model="meta-llama/Llama-3.1-70B-Instruct",le="25"} 4
tokend_tokenize_latency_us_bucket{model="meta-llama/Llama-3.1-70B-Instruct",le="50"} 91
...
# HELP tokend_chat_template_render_us Chat template rendering latency in microseconds
# TYPE tokend_chat_template_render_us histogram
tokend_chat_template_render_us_bucket{model="Qwen/Qwen3-8B",le="1"} 0
tokend_chat_template_render_us_bucket{model="Qwen/Qwen3-8B",le="5"} 42
...
# HELP tokend_tokens_total Total tokens produced
# TYPE tokend_tokens_total counter
tokend_tokens_total{model="meta-llama/Llama-3.1-70B-Instruct"} 142857
# HELP tokend_requests_total Total tokenize requests
# TYPE tokend_requests_total counter
tokend_requests_total{model="meta-llama/Llama-3.1-70B-Instruct",status="ok"} 8192
tokend_requests_total{model="meta-llama/Llama-3.1-70B-Instruct",status="error"} 0
# HELP tokend_loaded_models Number of currently loaded tokenizer models
# TYPE tokend_loaded_models gauge
tokend_loaded_models 3
```

**Metric reference**

| Metric | Type | Labels | Description |
|---|---|---|---|
| `tokend_tokenize_latency_us` | histogram | `model` | Tokenizer wall-clock time in microseconds; buckets at 10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000 |
| `tokend_chat_template_render_us` | histogram | `model` | Chat template rendering time in microseconds; buckets at 1, 2, 5, 10, 25, 50, 100, 250, 500, 1000 |
| `tokend_tokens_total` | counter | `model` | Cumulative tokens produced across all requests |
| `tokend_requests_total` | counter | `model`, `status` (`ok`\|`error`) | Cumulative tokenize requests by outcome |
| `tokend_loaded_models` | gauge | — | Currently loaded tokenizer count |

ext_proc requests are recorded against the same `tokend_tokenize_latency_us`,
`tokend_tokens_total`, and `tokend_requests_total` metrics as direct API calls.
The ext_proc path is not a separate metric namespace — it uses the same tokenizer
registry and produces the same observable outputs.

---

## Envoy ext_proc

tokend implements the Envoy
[External Processing](https://www.envoyproxy.io/docs/envoy/latest/api-v3/extensions/filters/http/ext_proc/v3/ext_proc.proto)
gRPC service. When enabled, Envoy streams request headers and body to tokend, which
parses the JSON payload, tokenizes it, and returns mutations — injecting token counts
and IDs back into the request before it reaches the upstream backend.

### Protocol flow

1. Envoy sends `RequestHeaders` — tokend checks `:path` against `intercept_paths`
2. If the path matches, tokend responds CONTINUE; Envoy buffers and sends the full body
3. tokend parses the JSON body, dispatching on payload shape:
   - `messages` field present → chat completions: apply chat template, then tokenize
   - `prompt` field present → completions: raw tokenize (no template)
4. tokend returns mutations based on the configured `mode`
5. If tokenization fails, the request passes through with an `x-tokend-error` header (fail-open)

### Mutation modes

| Mode | Behavior |
|---|---|
| `headers` | Inject `x-tokend-token-count` and `x-tokend-model` request headers |
| `body` | Mutate the JSON body to add `token_count` (and `token_ids` if `inject_tokens: true`) |
| `both` | Headers + body mutation |

### Envoy configuration

The ext_proc filter must be configured with `request_body_mode: BUFFERED` so tokend
receives the full request body for JSON parsing. See [examples/envoy.yaml](examples/envoy.yaml)
for a working configuration and [examples/README.md](examples/README.md) for a Docker
Compose demo.

---

## gRPC API

Package: `tokend.v1`. Service: `TokenizerService`. Port: `8766` by default.
The proto file is at `proto/tokend.proto`.

### Tokenize

```bash
grpcurl -plaintext \
  -d '{
    "model": "meta-llama/Llama-3.1-70B-Instruct",
    "texts": ["The quick brown fox", "Hello world"],
    "add_special_tokens": true,
    "return_tokens": false
  }' \
  localhost:8766 tokend.v1.TokenizerService/Tokenize
```

```json
{
  "model": "meta-llama/Llama-3.1-70B-Instruct",
  "results": [
    {
      "tokenIds": [128000, 791, 4062, 14198, 39935],
      "tokenCount": 5
    },
    {
      "tokenIds": [128000, 9906, 1917],
      "tokenCount": 3
    }
  ],
  "latencyUs": "58"
}
```

The gRPC Tokenize RPC always returns an array in `results`, regardless of whether one
or multiple texts were passed. This differs from the HTTP API, which returns flat fields
for the single-text case.

**Error codes**

| Condition | gRPC status |
|---|---|
| `texts` is empty | `INVALID_ARGUMENT` |
| Model not loaded | `NOT_FOUND` |
| Tokenization failure | `INTERNAL` |

### LoadTokenizer

```bash
grpcurl -plaintext \
  -d '{
    "model": "Qwen/Qwen3-32B",
    "source": "TOKENIZER_SOURCE_HUGGINGFACE"
  }' \
  localhost:8766 tokend.v1.TokenizerService/LoadTokenizer
```

```json
{
  "success": true,
  "message": "loaded Qwen/Qwen3-32B"
}
```

Local file:

```bash
grpcurl -plaintext \
  -d '{
    "model": "my-finetuned-llama",
    "source": "TOKENIZER_SOURCE_LOCAL",
    "path": "/models/my-finetuned-llama/tokenizer.json"
  }' \
  localhost:8766 tokend.v1.TokenizerService/LoadTokenizer
```

**Error codes**

| Condition | gRPC status |
|---|---|
| Model already loaded | `ALREADY_EXISTS` |
| Invalid source enum value | `INVALID_ARGUMENT` |
| Download/parse failure | `INTERNAL` |

### UnloadTokenizer

```bash
grpcurl -plaintext \
  -d '{"model": "Qwen/Qwen3-32B"}' \
  localhost:8766 tokend.v1.TokenizerService/UnloadTokenizer
```

```json
{
  "success": true,
  "message": "unloaded Qwen/Qwen3-32B"
}
```

**Error codes**

| Condition | gRPC status |
|---|---|
| Model not loaded | `NOT_FOUND` |

### Health

Liveness probe over gRPC.

```bash
grpcurl -plaintext \
  -d '{}' \
  localhost:8766 tokend.v1.TokenizerService/Health
```

```json
{"serving": true}
```

---

## CLI Reference

### tokend serve

Start the server. Reads config, loads tokenizers, then listens on HTTP TCP, HTTP UDS,
gRPC, and optionally ext_proc concurrently. Shuts down on SIGTERM or SIGINT.

```
tokend [OPTIONS] serve

OPTIONS:
  -c, --config <PATH>    Config file path [default: tokend.yaml]
  -h, --help             Print help
  -V, --version          Print version
```

```bash
tokend serve                        # uses tokend.yaml in current directory
tokend -c /etc/tokend/tokend.yaml serve
```

Log level is controlled by the `RUST_LOG` environment variable (default: `info`).

```bash
RUST_LOG=debug tokend -c tokend.yaml serve
RUST_LOG=tokend=trace tokend -c tokend.yaml serve
```

### tokend bench

Run a throughput benchmark against all models currently declared in the config.
Prints per-model latency (us/call) and throughput (tokens/sec).

```
tokend [OPTIONS] bench [OPTIONS]

OPTIONS (global):
  -c, --config <PATH>           Config file path [default: tokend.yaml]

OPTIONS (bench):
  -n, --iterations <N>          Iterations per model [default: 1000]
  -t, --text <TEXT>             Input text to tokenize
                                [default: "The quick brown fox jumps over the
                                lazy dog. This is a benchmark sentence for
                                tokenizer throughput testing."]
  -h, --help                    Print help
```

```bash
# Default: 1000 iterations, default text
tokend bench

# Custom workload
tokend -c tokend.yaml bench -n 5000 -t "$(cat my-prompt.txt)"
```

The bench command loads tokenizers synchronously before timing begins. HuggingFace
downloads count against startup time, not against the measured iterations.

---

## Environment Variables

| Variable | Description |
|---|---|
| `HF_TOKEN` | HuggingFace API token; referenced in config as `hf_token: "${HF_TOKEN}"` |
| `RUST_LOG` | Log level filter (e.g., `info`, `debug`, `tokend=trace`); parsed by `tracing-subscriber` |

---

## Ports

| Port | Protocol | Description |
|---|---|---|
| 8765 | HTTP | REST API, health/readiness probes, Prometheus metrics |
| 8766 | gRPC | TokenizerService RPCs |
| 8767 | gRPC | Envoy ext_proc (when `ext_proc.enabled: true`) |
