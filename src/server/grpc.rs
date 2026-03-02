use crate::chat_template;
use crate::config::TokenizerSource;
use crate::server::AppState;
use crate::tokenizer::TokenizerError;
use std::time::Instant;
use tonic::{Request, Response, Status};
use tracing::info;

pub mod proto {
    tonic::include_proto!("tokend.v1");
}

use proto::tokenizer_service_server::{TokenizerService, TokenizerServiceServer};
use proto::{
    ChatTokenizeRequest, ChatTokenizeResponse, HealthRequest, HealthResponse, LoadTokenizerRequest,
    LoadTokenizerResponse, TokenResult, TokenizeRequest, TokenizeResponse,
    TokenizerSource as ProtoSource, UnloadTokenizerRequest, UnloadTokenizerResponse,
};

pub struct GrpcService {
    state: AppState,
}

impl GrpcService {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl TokenizerService for GrpcService {
    async fn tokenize(
        &self,
        request: Request<TokenizeRequest>,
    ) -> Result<Response<TokenizeResponse>, Status> {
        let req = request.into_inner();

        if req.texts.is_empty() {
            return Err(Status::invalid_argument("texts must not be empty"));
        }

        let start = Instant::now();
        let texts: Vec<&str> = req.texts.iter().map(|s| s.as_str()).collect();

        match self.state.registry.tokenize(
            &req.model,
            &texts,
            req.add_special_tokens,
            req.return_tokens,
        ) {
            Ok(results) => {
                let latency_us = start.elapsed().as_micros() as u64;
                let total_tokens: u64 = results.iter().map(|r| r.token_count as u64).sum();

                self.state
                    .metrics
                    .record_tokenize(&req.model, latency_us as f64, total_tokens);

                let proto_results: Vec<TokenResult> = results
                    .into_iter()
                    .map(|r| TokenResult {
                        token_ids: r.token_ids,
                        token_count: r.token_count,
                        tokens: r.tokens.unwrap_or_default(),
                    })
                    .collect();

                Ok(Response::new(TokenizeResponse {
                    model: req.model,
                    results: proto_results,
                    latency_us,
                }))
            }
            Err(TokenizerError::ModelNotFound(model)) => {
                self.state.metrics.record_error(&model);
                Err(Status::not_found(format!("model not loaded: {model}")))
            }
            Err(e) => {
                self.state.metrics.record_error(&req.model);
                Err(Status::internal(e.to_string()))
            }
        }
    }

    async fn chat_tokenize(
        &self,
        request: Request<ChatTokenizeRequest>,
    ) -> Result<Response<ChatTokenizeResponse>, Status> {
        let req = request.into_inner();

        if req.messages.is_empty() {
            return Err(Status::invalid_argument("messages must not be empty"));
        }

        // Convert proto messages to domain types
        let messages: Vec<chat_template::ChatMessage> = req
            .messages
            .into_iter()
            .map(|m| chat_template::ChatMessage {
                role: m.role,
                content: m.content,
                tool_calls: if m.tool_calls.is_empty() {
                    None
                } else {
                    Some(
                        m.tool_calls
                            .into_iter()
                            .map(|tc| {
                                let func = tc.function.unwrap_or_default();
                                chat_template::ToolCall {
                                    id: tc.id,
                                    call_type: tc.r#type,
                                    function: chat_template::FunctionCall {
                                        name: func.name,
                                        arguments: func.arguments,
                                    },
                                }
                            })
                            .collect(),
                    )
                },
                tool_call_id: m.tool_call_id,
                name: m.name,
            })
            .collect();

        let start = Instant::now();

        match self.state.registry.chat_tokenize(
            &req.model,
            &messages,
            req.add_generation_prompt,
            None,
            true,
            req.return_tokens,
        ) {
            Ok(result) => {
                let latency_us = start.elapsed().as_micros() as u64;

                self.state.metrics.record_chat_tokenize(
                    &req.model,
                    latency_us as f64,
                    result.render_us as f64,
                    result.token_count as u64,
                );

                Ok(Response::new(ChatTokenizeResponse {
                    model: req.model,
                    token_count: result.token_count,
                    token_ids: result.token_ids,
                    tokens: result.tokens.unwrap_or_default(),
                    latency_us,
                    render_us: result.render_us,
                }))
            }
            Err(TokenizerError::ModelNotFound(model)) => {
                self.state.metrics.record_error(&model);
                Err(Status::not_found(format!("model not loaded: {model}")))
            }
            Err(TokenizerError::ChatTemplateNotAvailable(model)) => Err(
                Status::failed_precondition(format!("chat template not available for {model}")),
            ),
            Err(e) => {
                self.state.metrics.record_error(&req.model);
                Err(Status::internal(e.to_string()))
            }
        }
    }

    async fn load_tokenizer(
        &self,
        request: Request<LoadTokenizerRequest>,
    ) -> Result<Response<LoadTokenizerResponse>, Status> {
        let req = request.into_inner();

        let source = match ProtoSource::try_from(req.source) {
            Ok(ProtoSource::Huggingface) => TokenizerSource::Huggingface,
            Ok(ProtoSource::Local) => TokenizerSource::Local,
            _ => {
                return Err(Status::invalid_argument(
                    "source must be HUGGINGFACE or LOCAL",
                ));
            }
        };

        match self.state.registry.load(
            &req.model,
            &source,
            req.path.as_deref(),
            self.state.config.hf_token.as_deref(),
        ) {
            Ok(()) => {
                self.state
                    .metrics
                    .set_loaded_models(self.state.registry.model_count() as f64);
                Ok(Response::new(LoadTokenizerResponse {
                    success: true,
                    message: format!("loaded {}", req.model),
                }))
            }
            Err(TokenizerError::AlreadyLoaded(model)) => Err(Status::already_exists(format!(
                "model already loaded: {model}"
            ))),
            Err(e) => Err(Status::internal(e.to_string())),
        }
    }

    async fn unload_tokenizer(
        &self,
        request: Request<UnloadTokenizerRequest>,
    ) -> Result<Response<UnloadTokenizerResponse>, Status> {
        let req = request.into_inner();

        match self.state.registry.unload(&req.model) {
            Ok(()) => {
                self.state
                    .metrics
                    .set_loaded_models(self.state.registry.model_count() as f64);
                Ok(Response::new(UnloadTokenizerResponse {
                    success: true,
                    message: format!("unloaded {}", req.model),
                }))
            }
            Err(TokenizerError::ModelNotFound(m)) => {
                Err(Status::not_found(format!("model not loaded: {m}")))
            }
            Err(e) => Err(Status::internal(e.to_string())),
        }
    }

    async fn health(
        &self,
        _request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        Ok(Response::new(HealthResponse { serving: true }))
    }
}

/// Start gRPC server on TCP.
pub async fn serve_grpc(state: AppState, port: u16) -> anyhow::Result<()> {
    let addr = format!("0.0.0.0:{port}").parse()?;
    let service = GrpcService::new(state);

    info!(port, "gRPC server listening");
    tonic::transport::Server::builder()
        .add_service(TokenizerServiceServer::new(service))
        .serve_with_shutdown(addr, async {
            tokio::signal::ctrl_c().await.ok();
        })
        .await?;

    Ok(())
}
