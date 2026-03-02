use dashmap::DashMap;
use prometheus::{
    Counter, CounterVec, Encoder, Gauge, Histogram, HistogramVec, TextEncoder,
    register_counter_vec, register_gauge, register_histogram_vec,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Pre-resolved metric handles for a single model, avoiding repeated
/// `with_label_values` hash lookups on the hot path.
struct ModelMetrics {
    latency: Histogram,
    chat_render: Histogram,
    tokens: Counter,
    requests_ok: Counter,
    requests_err: Counter,
}

#[derive(Clone)]
pub struct Metrics {
    tokenize_latency_us: HistogramVec,
    chat_template_render_us: HistogramVec,
    tokens_total: CounterVec,
    requests_total: CounterVec,
    pub loaded_models: Gauge,
    per_model: Arc<DashMap<String, ModelMetrics>>,
    ext_proc_requests: Arc<AtomicU64>,
    ext_proc_passthrough: Arc<AtomicU64>,
    ext_proc_errors: Arc<AtomicU64>,
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        let tokenize_latency_us = register_histogram_vec!(
            "tokend_tokenize_latency_us",
            "Tokenization latency in microseconds",
            &["model"],
            vec![
                10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0, 10000.0
            ]
        )
        .expect("failed to register tokend_tokenize_latency_us");

        let chat_template_render_us = register_histogram_vec!(
            "tokend_chat_template_render_us",
            "Chat template rendering latency in microseconds",
            &["model"],
            vec![1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0]
        )
        .expect("failed to register tokend_chat_template_render_us");

        let tokens_total =
            register_counter_vec!("tokend_tokens_total", "Total tokens produced", &["model"])
                .expect("failed to register tokend_tokens_total");

        let requests_total = register_counter_vec!(
            "tokend_requests_total",
            "Total tokenize requests",
            &["model", "status"]
        )
        .expect("failed to register tokend_requests_total");

        let loaded_models = register_gauge!(
            "tokend_loaded_models",
            "Number of currently loaded tokenizer models"
        )
        .expect("failed to register tokend_loaded_models");

        Self {
            tokenize_latency_us,
            chat_template_render_us,
            tokens_total,
            requests_total,
            loaded_models,
            per_model: Arc::new(DashMap::new()),
            ext_proc_requests: Arc::new(AtomicU64::new(0)),
            ext_proc_passthrough: Arc::new(AtomicU64::new(0)),
            ext_proc_errors: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Ensure cached metric handles exist for a model. Called once per model,
    /// subsequent calls are a DashMap get (no hash allocation).
    fn ensure_model(&self, model: &str) {
        if !self.per_model.contains_key(model) {
            self.per_model
                .entry(model.to_string())
                .or_insert_with(|| ModelMetrics {
                    latency: self.tokenize_latency_us.with_label_values(&[model]),
                    chat_render: self.chat_template_render_us.with_label_values(&[model]),
                    tokens: self.tokens_total.with_label_values(&[model]),
                    requests_ok: self.requests_total.with_label_values(&[model, "ok"]),
                    requests_err: self.requests_total.with_label_values(&[model, "error"]),
                });
        }
    }

    pub fn record_tokenize(&self, model: &str, latency_us: f64, token_count: u64) {
        self.ensure_model(model);
        let m = self.per_model.get(model).unwrap();
        m.latency.observe(latency_us);
        m.tokens.inc_by(token_count as f64);
        m.requests_ok.inc();
    }

    pub fn record_chat_tokenize(
        &self,
        model: &str,
        latency_us: f64,
        render_us: f64,
        token_count: u64,
    ) {
        self.ensure_model(model);
        let m = self.per_model.get(model).unwrap();
        m.latency.observe(latency_us);
        m.chat_render.observe(render_us);
        m.tokens.inc_by(token_count as f64);
        m.requests_ok.inc();
    }

    pub fn record_error(&self, model: &str) {
        self.ensure_model(model);
        self.per_model.get(model).unwrap().requests_err.inc();
    }

    pub fn set_loaded_models(&self, count: f64) {
        self.loaded_models.set(count);
    }

    pub fn record_ext_proc(&self, model: &str, latency_us: f64, token_count: u64) {
        self.ensure_model(model);
        let m = self.per_model.get(model).unwrap();
        m.latency.observe(latency_us);
        m.tokens.inc_by(token_count as f64);
        m.requests_ok.inc();
        self.ext_proc_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_ext_proc_passthrough(&self) {
        self.ext_proc_passthrough.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_ext_proc_error(&self) {
        self.ext_proc_errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn encode(&self) -> String {
        let encoder = TextEncoder::new();
        let metric_families = prometheus::gather();
        let mut buffer = Vec::new();
        encoder.encode(&metric_families, &mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    }
}
