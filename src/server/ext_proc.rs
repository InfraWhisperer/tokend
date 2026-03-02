use crate::chat_template::ChatMessage;
use crate::server::AppState;
use envoy_types::pb::envoy::config::core::v3::{HeaderValue, HeaderValueOption};
use envoy_types::pb::envoy::service::ext_proc::v3::external_processor_server::{
    ExternalProcessor, ExternalProcessorServer,
};
use envoy_types::pb::envoy::service::ext_proc::v3::processing_request::Request as ExtRequest;
use envoy_types::pb::envoy::service::ext_proc::v3::processing_response::Response as ExtResponse;
use envoy_types::pb::envoy::service::ext_proc::v3::{
    BodyMutation, BodyResponse, CommonResponse, HeaderMutation, HeadersResponse, HttpBody,
    HttpHeaders, ProcessingRequest, ProcessingResponse,
};
use std::time::Instant;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, info, warn};

pub struct ExtProcService {
    state: AppState,
}

impl ExtProcService {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl ExternalProcessor for ExtProcService {
    type ProcessStream = ReceiverStream<Result<ProcessingResponse, Status>>;

    async fn process(
        &self,
        request: Request<Streaming<ProcessingRequest>>,
    ) -> Result<Response<Self::ProcessStream>, Status> {
        let mut stream = request.into_inner();
        let (tx, rx) = mpsc::channel(4);
        let state = self.state.clone();

        tokio::spawn(async move {
            let mut intercept = false;

            while let Some(msg) = stream.next().await {
                let req = match msg {
                    Ok(r) => r,
                    Err(e) => {
                        warn!(error = %e, "ext_proc stream error");
                        break;
                    }
                };

                let resp = match req.request {
                    Some(ExtRequest::RequestHeaders(ref headers)) => {
                        intercept = should_intercept(headers, &state);
                        if intercept {
                            debug!("ext_proc: intercepting request");
                        } else {
                            state.metrics.record_ext_proc_passthrough();
                        }
                        continue_headers_response()
                    }
                    Some(ExtRequest::RequestBody(ref body)) if intercept => {
                        handle_request_body(body, &state)
                    }
                    Some(ExtRequest::RequestBody(_)) => continue_body_response(),
                    Some(ExtRequest::ResponseHeaders(_)) => ProcessingResponse {
                        response: Some(ExtResponse::ResponseHeaders(HeadersResponse {
                            response: Some(CommonResponse::default()),
                        })),
                        ..Default::default()
                    },
                    Some(ExtRequest::ResponseBody(_)) => ProcessingResponse {
                        response: Some(ExtResponse::ResponseBody(BodyResponse {
                            response: Some(CommonResponse::default()),
                        })),
                        ..Default::default()
                    },
                    _ => continue_headers_response(),
                };

                if tx.send(Ok(resp)).await.is_err() {
                    break;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

/// Check if the request path matches any configured intercept paths.
fn should_intercept(headers: &HttpHeaders, state: &AppState) -> bool {
    if let Some(ref entries) = headers.headers {
        for header in &entries.headers {
            if header.key == ":path" {
                let path = if !header.raw_value.is_empty() {
                    String::from_utf8_lossy(&header.raw_value).to_string()
                } else if !header.value.is_empty() {
                    header.value.clone()
                } else {
                    String::new()
                };

                // Strip query string for matching
                let path_only = path.split('?').next().unwrap_or(&path);
                return state
                    .config
                    .ext_proc
                    .intercept_paths
                    .iter()
                    .any(|p| p == path_only);
            }
        }
    }
    false
}

/// Parse the buffered request body, detect endpoint format, tokenize,
/// and build the appropriate mutation response.
///
/// Dispatches on payload shape:
///   - `messages` array  → /chat/completions path: chat template + tokenize
///   - `prompt` string   → /completions path: raw tokenize (no template)
fn handle_request_body(body: &HttpBody, state: &AppState) -> ProcessingResponse {
    let start = Instant::now();

    let body_bytes = &body.body;
    let parsed: serde_json::Value = match serde_json::from_slice(body_bytes) {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "ext_proc: failed to parse request body as JSON");
            state.metrics.record_ext_proc_error();
            return error_response(&format!("JSON parse error: {e}"));
        }
    };

    let model = match parsed.get("model").and_then(|v| v.as_str()) {
        Some(m) => m.to_string(),
        None => {
            warn!("ext_proc: request body missing 'model' field");
            state.metrics.record_ext_proc_error();
            return error_response("missing 'model' field");
        }
    };

    // Dispatch: chat completions (messages[]) vs completions (prompt string)
    let tokenize_result = if parsed.get("messages").is_some() {
        tokenize_chat_request(&parsed, &model, state)
    } else if parsed.get("prompt").is_some() {
        tokenize_completions_request(&parsed, &model, state)
    } else {
        warn!("ext_proc: request body has neither 'messages' nor 'prompt'");
        state.metrics.record_ext_proc_error();
        return error_response("missing 'messages' or 'prompt' field");
    };

    match tokenize_result {
        Ok((token_count, token_ids)) => {
            let latency_us = start.elapsed().as_micros() as f64;
            state
                .metrics
                .record_ext_proc(&model, latency_us, token_count as u64);

            build_mutation_response(
                &state.config.ext_proc,
                &model,
                token_count,
                &token_ids,
                &parsed,
            )
        }
        Err(resp) => *resp,
    }
}

/// Chat completions path: parse messages, apply chat template, tokenize.
fn tokenize_chat_request(
    parsed: &serde_json::Value,
    model: &str,
    state: &AppState,
) -> Result<(u32, Vec<u32>), Box<ProcessingResponse>> {
    let messages_val = parsed.get("messages").unwrap();
    let messages: Vec<ChatMessage> = match serde_json::from_value(messages_val.clone()) {
        Ok(m) => m,
        Err(e) => {
            warn!(error = %e, "ext_proc: failed to parse 'messages'");
            state.metrics.record_ext_proc_error();
            return Err(Box::new(error_response(&format!(
                "messages parse error: {e}"
            ))));
        }
    };

    let add_generation_prompt = parsed
        .get("add_generation_prompt")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    match state
        .registry
        .chat_tokenize(model, &messages, add_generation_prompt, None, true, false)
    {
        Ok(result) => Ok((result.token_count, result.token_ids)),
        Err(e) => {
            warn!(model = %model, error = %e, "ext_proc: chat tokenization failed");
            state.metrics.record_ext_proc_error();
            Err(Box::new(error_response(&format!(
                "tokenization failed: {e}"
            ))))
        }
    }
}

/// Completions path: extract prompt string, tokenize directly (no chat template).
fn tokenize_completions_request(
    parsed: &serde_json::Value,
    model: &str,
    state: &AppState,
) -> Result<(u32, Vec<u32>), Box<ProcessingResponse>> {
    let prompt = match parsed.get("prompt").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => {
            warn!("ext_proc: 'prompt' field is not a string");
            state.metrics.record_ext_proc_error();
            return Err(Box::new(error_response("'prompt' must be a string")));
        }
    };

    match state.registry.tokenize(model, &[prompt], true, false) {
        Ok(mut results) => {
            let result = results.remove(0);
            Ok((result.token_count, result.token_ids))
        }
        Err(e) => {
            warn!(model = %model, error = %e, "ext_proc: completions tokenization failed");
            state.metrics.record_ext_proc_error();
            Err(Box::new(error_response(&format!(
                "tokenization failed: {e}"
            ))))
        }
    }
}

/// Build a ProcessingResponse with header and/or body mutations based on mode.
fn build_mutation_response(
    ext_proc: &crate::config::ExtProcConfig,
    model: &str,
    token_count: u32,
    token_ids: &[u32],
    parsed_body: &serde_json::Value,
) -> ProcessingResponse {
    let mode = ext_proc.mode.as_str();

    let header_mutation = if mode == "headers" || mode == "both" {
        Some(HeaderMutation {
            set_headers: vec![
                HeaderValueOption {
                    header: Some(HeaderValue {
                        key: ext_proc.token_count_header.clone(),
                        raw_value: token_count.to_string().into_bytes(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                HeaderValueOption {
                    header: Some(HeaderValue {
                        key: ext_proc.model_header.clone(),
                        raw_value: model.as_bytes().to_vec(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
            ..Default::default()
        })
    } else {
        None
    };

    let (body_mutation, content_length_update) = if mode == "body" || mode == "both" {
        let mut modified = parsed_body.clone();
        if let Some(obj) = modified.as_object_mut() {
            obj.insert(
                ext_proc.body_field.clone(),
                serde_json::Value::Number(serde_json::Number::from(token_count)),
            );
            if ext_proc.inject_tokens {
                let ids: Vec<serde_json::Value> = token_ids
                    .iter()
                    .map(|&id| serde_json::Value::Number(serde_json::Number::from(id)))
                    .collect();
                obj.insert(
                    ext_proc.token_ids_field.clone(),
                    serde_json::Value::Array(ids),
                );
            }
        }
        match serde_json::to_vec(&modified) {
            Ok(bytes) => {
                let new_len = bytes.len();
                (
                    Some(BodyMutation {
                        mutation: Some(
                            envoy_types::pb::envoy::service::ext_proc::v3::body_mutation::Mutation::Body(
                                bytes,
                            ),
                        ),
                    }),
                    Some(new_len),
                )
            }
            Err(_) => (None, None),
        }
    } else {
        (None, None)
    };

    // Merge header mutations: mode-specific headers + content-length update for body mutations.
    let merged_header_mutation = {
        let mut headers = header_mutation.map(|h| h.set_headers).unwrap_or_default();

        if let Some(len) = content_length_update {
            headers.push(HeaderValueOption {
                header: Some(HeaderValue {
                    key: "content-length".to_string(),
                    raw_value: len.to_string().into_bytes(),
                    ..Default::default()
                }),
                ..Default::default()
            });
        }

        if headers.is_empty() {
            None
        } else {
            Some(HeaderMutation {
                set_headers: headers,
                ..Default::default()
            })
        }
    };

    // Respond with BodyResponse — this is always the body phase since we
    // only reach this function after receiving the buffered request body.
    ProcessingResponse {
        response: Some(ExtResponse::RequestBody(BodyResponse {
            response: Some(CommonResponse {
                header_mutation: merged_header_mutation,
                body_mutation,
                ..Default::default()
            }),
        })),
        ..Default::default()
    }
}

/// Build a CONTINUE response for the request headers phase.
fn continue_headers_response() -> ProcessingResponse {
    ProcessingResponse {
        response: Some(ExtResponse::RequestHeaders(HeadersResponse {
            response: Some(CommonResponse::default()),
        })),
        ..Default::default()
    }
}

/// Build a CONTINUE response for the request body phase.
fn continue_body_response() -> ProcessingResponse {
    ProcessingResponse {
        response: Some(ExtResponse::RequestBody(BodyResponse {
            response: Some(CommonResponse::default()),
        })),
        ..Default::default()
    }
}

/// Build a fail-open response: pass through with an error header.
fn error_response(msg: &str) -> ProcessingResponse {
    ProcessingResponse {
        response: Some(ExtResponse::RequestBody(BodyResponse {
            response: Some(CommonResponse {
                header_mutation: Some(HeaderMutation {
                    set_headers: vec![HeaderValueOption {
                        header: Some(HeaderValue {
                            key: "x-tokend-error".to_string(),
                            raw_value: msg.as_bytes().to_vec(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            }),
        })),
        ..Default::default()
    }
}

/// Start ext_proc gRPC server.
pub async fn serve_ext_proc(state: AppState, port: u16) -> anyhow::Result<()> {
    let addr = format!("0.0.0.0:{port}").parse()?;
    let service = ExtProcService::new(state);

    info!(port, "ext_proc server listening");
    tonic::transport::Server::builder()
        .add_service(ExternalProcessorServer::new(service))
        .serve_with_shutdown(addr, async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await?;

    Ok(())
}
