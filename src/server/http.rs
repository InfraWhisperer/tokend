use crate::chat_template::{ChatMessage, Tool};
use crate::config::TokenizerSource;
use crate::server::AppState;
use crate::tokenizer::TokenizerError;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::info;

// --- Request/Response types ---

#[derive(Debug, Deserialize)]
pub struct TokenizeRequest {
    pub model: String,
    #[serde(deserialize_with = "deserialize_text")]
    pub text: Vec<String>,
    #[serde(default = "default_true")]
    pub add_special_tokens: bool,
    #[serde(default)]
    pub return_tokens: bool,
}

fn default_true() -> bool {
    true
}

/// Accept "text" as either a single string or array of strings.
fn deserialize_text<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrVec {
        Single(String),
        Multiple(Vec<String>),
    }

    match StringOrVec::deserialize(deserializer)? {
        StringOrVec::Single(s) => Ok(vec![s]),
        StringOrVec::Multiple(v) => Ok(v),
    }
}

#[derive(Debug, Serialize)]
pub struct TokenizeResponse {
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_ids: Option<Vec<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results: Option<Vec<TokenResultJson>>,
    pub latency_us: u64,
}

#[derive(Debug, Serialize)]
pub struct TokenResultJson {
    pub token_ids: Vec<u32>,
    pub token_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct LoadTokenizerRequest {
    pub model: String,
    #[serde(default = "default_hf_source")]
    pub source: String,
    pub path: Option<String>,
}

fn default_hf_source() -> String {
    "huggingface".to_string()
}

#[derive(Debug, Deserialize)]
pub struct ChatTokenizeRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default = "default_true")]
    pub add_generation_prompt: bool,
    #[serde(default)]
    pub tools: Option<Vec<Tool>>,
    #[serde(default = "default_true")]
    pub add_special_tokens: bool,
    #[serde(default)]
    pub return_tokens: bool,
}

#[derive(Debug, Serialize)]
pub struct ChatTokenizeResponse {
    pub model: String,
    pub token_count: u32,
    pub token_ids: Vec<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Vec<String>>,
    pub latency_us: u64,
    pub render_us: u64,
}

// --- Handlers ---

async fn tokenize_handler(
    State(state): State<AppState>,
    Json(req): Json<TokenizeRequest>,
) -> impl IntoResponse {
    let start = Instant::now();
    let texts: Vec<&str> = req.text.iter().map(|s| s.as_str()).collect();

    match state.registry.tokenize(
        &req.model,
        &texts,
        req.add_special_tokens,
        req.return_tokens,
    ) {
        Ok(results) => {
            let latency_us = start.elapsed().as_micros() as u64;
            let total_tokens: u64 = results.iter().map(|r| r.token_count as u64).sum();

            state
                .metrics
                .record_tokenize(&req.model, latency_us as f64, total_tokens);

            let response = if results.len() == 1 {
                let r = results.into_iter().next().unwrap();
                TokenizeResponse {
                    model: req.model,
                    token_ids: Some(r.token_ids),
                    token_count: Some(r.token_count),
                    tokens: r.tokens,
                    results: None,
                    latency_us,
                }
            } else {
                let json_results: Vec<TokenResultJson> = results
                    .into_iter()
                    .map(|r| TokenResultJson {
                        token_ids: r.token_ids,
                        token_count: r.token_count,
                        tokens: r.tokens,
                    })
                    .collect();
                TokenizeResponse {
                    model: req.model,
                    token_ids: None,
                    token_count: None,
                    tokens: None,
                    results: Some(json_results),
                    latency_us,
                }
            };

            (
                StatusCode::OK,
                Json(serde_json::to_value(response).unwrap()),
            )
                .into_response()
        }
        Err(TokenizerError::ModelNotFound(model)) => {
            state.metrics.record_error(&model);
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": format!("model not loaded: {model}") })),
            )
                .into_response()
        }
        Err(e) => {
            state.metrics.record_error(&req.model);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

async fn chat_tokenize_handler(
    State(state): State<AppState>,
    Json(req): Json<ChatTokenizeRequest>,
) -> impl IntoResponse {
    let start = Instant::now();

    match state.registry.chat_tokenize(
        &req.model,
        &req.messages,
        req.add_generation_prompt,
        req.tools.as_deref(),
        req.add_special_tokens,
        req.return_tokens,
    ) {
        Ok(result) => {
            let latency_us = start.elapsed().as_micros() as u64;

            state.metrics.record_chat_tokenize(
                &req.model,
                latency_us as f64,
                result.render_us as f64,
                result.token_count as u64,
            );

            (
                StatusCode::OK,
                Json(serde_json::to_value(ChatTokenizeResponse {
                    model: req.model,
                    token_count: result.token_count,
                    token_ids: result.token_ids,
                    tokens: result.tokens,
                    latency_us,
                    render_us: result.render_us,
                })
                .unwrap()),
            )
                .into_response()
        }
        Err(TokenizerError::ModelNotFound(model)) => {
            state.metrics.record_error(&model);
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": format!("model not loaded: {model}") })),
            )
                .into_response()
        }
        Err(TokenizerError::ChatTemplateNotAvailable(model)) => (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({ "error": format!("chat template not available for {model}") }),
            ),
        )
            .into_response(),
        Err(e) => {
            state.metrics.record_error(&req.model);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    }
}

async fn load_tokenizer_handler(
    State(state): State<AppState>,
    Json(req): Json<LoadTokenizerRequest>,
) -> impl IntoResponse {
    let source = match req.source.as_str() {
        "huggingface" => TokenizerSource::Huggingface,
        "local" => TokenizerSource::Local,
        other => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("unknown source: {other}") })),
            )
                .into_response();
        }
    };

    match state.registry.load(
        &req.model,
        &source,
        req.path.as_deref(),
        state.config.hf_token.as_deref(),
    ) {
        Ok(()) => {
            state
                .metrics
                .set_loaded_models(state.registry.model_count() as f64);
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "success": true,
                    "message": format!("loaded {}", req.model)
                })),
            )
                .into_response()
        }
        Err(TokenizerError::AlreadyLoaded(model)) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": format!("model already loaded: {model}") })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn unload_tokenizer_handler(
    State(state): State<AppState>,
    Path(model): Path<String>,
) -> impl IntoResponse {
    // URL-decode the model name (slashes are encoded as %2F)
    let model = urlencoding::decode(&model)
        .unwrap_or_else(|_| model.clone().into())
        .into_owned();

    match state.registry.unload(&model) {
        Ok(()) => {
            state
                .metrics
                .set_loaded_models(state.registry.model_count() as f64);
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "success": true,
                    "message": format!("unloaded {}", model)
                })),
            )
                .into_response()
        }
        Err(TokenizerError::ModelNotFound(m)) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("model not loaded: {m}") })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" })))
}

async fn ready_handler(State(state): State<AppState>) -> impl IntoResponse {
    let loaded = state.registry.model_count();
    let expected = state.config.tokenizers.len();

    if loaded >= expected && expected > 0 {
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "ready": true,
                "loaded_models": loaded,
                "expected_models": expected
            })),
        )
            .into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "ready": false,
                "loaded_models": loaded,
                "expected_models": expected
            })),
        )
            .into_response()
    }
}

async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        state.metrics.encode(),
    )
}

// --- Router ---

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/tokenize", post(tokenize_handler))
        .route("/v1/chat/tokenize", post(chat_tokenize_handler))
        .route("/tokenizers/load", post(load_tokenizer_handler))
        .route("/tokenizers/{model}", delete(unload_tokenizer_handler))
        .route("/health", get(health_handler))
        .route("/ready", get(ready_handler))
        .route("/metrics", get(metrics_handler))
        .with_state(state)
}

/// Start HTTP server on TCP.
pub async fn serve_http(state: AppState, port: u16) -> anyhow::Result<()> {
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
    info!(port, "HTTP server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Start HTTP server on Unix Domain Socket.
pub async fn serve_uds(state: AppState, path: &str) -> anyhow::Result<()> {
    // Remove stale socket file
    let _ = tokio::fs::remove_file(path).await;

    // Ensure parent directory exists
    if let Some(parent) = std::path::Path::new(path).parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }

    let app = router(state);
    let listener = tokio::net::UnixListener::bind(path)?;
    info!(path, "UDS server listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c().await.ok();
}
