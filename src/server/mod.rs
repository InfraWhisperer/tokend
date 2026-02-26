pub mod grpc;
pub mod http;

use crate::config::Config;
use crate::metrics::Metrics;
use crate::tokenizer::TokenizerRegistry;
use std::sync::Arc;
use tokio::signal;
use tracing::info;

/// Shared application state across all transport layers.
#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<TokenizerRegistry>,
    pub metrics: Metrics,
    pub config: Arc<Config>,
}

/// Run all servers (HTTP, UDS, gRPC) concurrently.
pub async fn run(config: Config) -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(true)
        .init();

    let metrics = Metrics::new();
    let registry = Arc::new(TokenizerRegistry::new(&config.cache_dir));

    info!("loading tokenizers from config");
    registry.load_from_config(&config);
    metrics.set_loaded_models(registry.model_count() as f64);

    let loaded = registry.list_models();
    if loaded.is_empty() {
        tracing::warn!("no tokenizers loaded — server will start but /ready will return 503");
    } else {
        info!(count = loaded.len(), models = ?loaded, "tokenizers ready");
    }

    let state = AppState {
        registry,
        metrics,
        config: Arc::new(config),
    };

    let http_state = state.clone();
    let uds_state = state.clone();
    let grpc_state = state.clone();

    let http_port = state.config.server.http_port;
    let grpc_port = state.config.server.grpc_port;
    let uds_path = state.config.server.uds_path.clone();

    // Spawn all servers
    let http_handle = tokio::spawn(async move {
        if let Err(e) = http::serve_http(http_state, http_port).await {
            tracing::error!(error = %e, "HTTP server failed");
        }
    });

    let uds_handle = tokio::spawn(async move {
        if let Err(e) = http::serve_uds(uds_state, &uds_path).await {
            tracing::error!(error = %e, "UDS server failed");
        }
    });

    let grpc_handle = tokio::spawn(async move {
        if let Err(e) = grpc::serve_grpc(grpc_state, grpc_port).await {
            tracing::error!(error = %e, "gRPC server failed");
        }
    });

    info!(http_port, grpc_port, "tokend serving");

    // Wait for shutdown signal
    shutdown_signal().await;
    info!("shutdown signal received, draining...");

    http_handle.abort();
    uds_handle.abort();
    grpc_handle.abort();

    info!("tokend stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
