# Envoy ext_proc Demo

Demonstrates tokend as an Envoy external processor. Envoy intercepts
`/v1/chat/completions` and `/v1/completions` requests, streams headers and body to
tokend via ext_proc, and tokend injects token counts and IDs transparently — headers,
body mutation, or both.

## Architecture

```
curl → Envoy (:10000) → ext_proc → tokend (:8767) → tokenize + inject
                       → route    → httpbin (:80)  → echoes the modified request
```

Envoy buffers the full request body (`request_body_mode: BUFFERED`), sends it to
tokend's ext_proc service, which parses the JSON, tokenizes the input, and returns
mutations. httpbin echoes the final request so you can see the injected headers and
body fields.

tokend dispatches on the body shape:
- `messages` field → applies chat template, then tokenizes (for `/v1/chat/completions`)
- `prompt` field → raw tokenization, no template (for `/v1/completions`)

## Setup

```bash
cd examples
HF_TOKEN=hf_xxx docker compose up --build
```

Wait for tokend to log `tokenizers ready` before sending requests.

## Usage

### Chat completions (messages → chat template → tokenize)

```bash
curl -s http://localhost:10000/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "Qwen/Qwen3-8B",
    "messages": [
      {"role": "system", "content": "You are helpful."},
      {"role": "user", "content": "Explain KV caching in transformer inference."}
    ]
  }' | jq .
```

### Completions (prompt → raw tokenize)

```bash
curl -s http://localhost:10000/v1/completions \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "Qwen/Qwen3-8B",
    "prompt": "The quick brown fox"
  }' | jq .
```

httpbin echoes the request. Look for:
- **Headers**: `X-Tokend-Token-Count` and `X-Tokend-Model`
- **Body**: `token_count` field and `token_ids` array injected into the JSON payload

The demo config uses `mode: "both"` with `inject_tokens: true`, so headers, token count,
and the full token ID array are all injected. The chat completions request produces token
IDs for the full chat-templated prompt (including special tokens and role markers). The
completions request produces token IDs for the raw prompt string only.

## Configuration

See `tokend-demo.yaml` for the tokend configuration. Key `ext_proc` settings:

| Field | Description |
|-------|-------------|
| `mode` | `"headers"` — inject token count as request headers only |
|         | `"body"` — mutate the JSON body to add a `token_count` field |
|         | `"both"` — headers + body mutation |
| `intercept_paths` | Request paths to intercept. Non-matching paths pass through untouched. |
| `token_count_header` | Header name for the token count (default: `x-tokend-token-count`) |
| `model_header` | Header name for the resolved model (default: `x-tokend-model`) |
| `body_field` | JSON field name for the token count in body mode (default: `token_count`) |
| `inject_tokens` | When `true`, inject the full `token_ids` array into the body (default: `false`) |
| `token_ids_field` | JSON field name for the token IDs array (default: `token_ids`) |

## Non-matching requests

Requests to paths not in `intercept_paths` pass through with no ext_proc processing:

```bash
curl -s http://localhost:10000/get | jq .headers
# No x-tokend-* headers — request was not intercepted
```
