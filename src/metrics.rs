use prometheus::{
    register_counter_vec, register_gauge, register_histogram_vec, CounterVec, Encoder, Gauge,
    HistogramVec, TextEncoder,
};

#[derive(Clone)]
pub struct Metrics {
    pub tokenize_latency_us: HistogramVec,
    pub tokens_total: CounterVec,
    pub requests_total: CounterVec,
    pub loaded_models: Gauge,
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
            // Buckets: 10us to 10ms covers sub-ms through slow tokenizations
            vec![10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0, 10000.0]
        )
        .expect("failed to register tokend_tokenize_latency_us");

        let tokens_total = register_counter_vec!(
            "tokend_tokens_total",
            "Total tokens produced",
            &["model"]
        )
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
            tokens_total,
            requests_total,
            loaded_models,
        }
    }

    pub fn record_tokenize(&self, model: &str, latency_us: f64, token_count: u64) {
        self.tokenize_latency_us
            .with_label_values(&[model])
            .observe(latency_us);
        self.tokens_total
            .with_label_values(&[model])
            .inc_by(token_count as f64);
        self.requests_total
            .with_label_values(&[model, "ok"])
            .inc();
    }

    pub fn record_error(&self, model: &str) {
        self.requests_total
            .with_label_values(&[model, "error"])
            .inc();
    }

    pub fn set_loaded_models(&self, count: f64) {
        self.loaded_models.set(count);
    }

    pub fn encode(&self) -> String {
        let encoder = TextEncoder::new();
        let metric_families = prometheus::gather();
        let mut buffer = Vec::new();
        encoder.encode(&metric_families, &mut buffer).unwrap();
        String::from_utf8(buffer).unwrap()
    }
}
