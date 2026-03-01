use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokend::config::TokenizerSource;
use tokend::payloads::{TEXT_LONG, TEXT_MEDIUM, TEXT_SHORT};
use tokend::tokenizer::TokenizerRegistry;

const BENCH_TOKENIZER_DIR: &str = "/tmp/tokend-bench";
const BENCH_TOKENIZER_PATH: &str = "/tmp/tokend-bench/tokenizer.json";
const MODEL_NAME: &str = "bench-bert-base-cased";

/// Attempt to provision a tokenizer at BENCH_TOKENIZER_PATH.
///
/// Strategy:
///   1. Return immediately if the file already exists.
///   2. Try `Tokenizer::from_pretrained("bert-base-cased", None)` and save to disk.
///   3. If that fails (no network, no HF token), return None and callers skip gracefully.
fn ensure_tokenizer() -> Option<PathBuf> {
    let path = PathBuf::from(BENCH_TOKENIZER_PATH);

    if path.exists() {
        return Some(path);
    }

    eprintln!(
        "[bench] tokenizer not found at {BENCH_TOKENIZER_PATH}; \
         attempting to download bert-base-cased from HuggingFace Hub"
    );

    let dir = Path::new(BENCH_TOKENIZER_DIR);
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("[bench] failed to create {BENCH_TOKENIZER_DIR}: {e}; skipping benchmarks");
        return None;
    }

    match tokenizers::Tokenizer::from_pretrained("bert-base-cased", None) {
        Ok(tok) => {
            if let Err(e) = tok.save(&path, false) {
                eprintln!(
                    "[bench] failed to save tokenizer to {BENCH_TOKENIZER_PATH}: {e}; skipping benchmarks"
                );
                return None;
            }
            eprintln!("[bench] tokenizer saved to {BENCH_TOKENIZER_PATH}");
            Some(path)
        }
        Err(e) => {
            eprintln!(
                "[bench] from_pretrained failed: {e}\n\
                 [bench] To run benchmarks, place a tokenizer.json at {BENCH_TOKENIZER_PATH}"
            );
            None
        }
    }
}

/// Build a registry with the bench tokenizer pre-loaded under MODEL_NAME.
/// Returns None if no tokenizer is available.
fn build_registry() -> Option<Arc<TokenizerRegistry>> {
    let tokenizer_path = ensure_tokenizer()?;

    let registry = TokenizerRegistry::new(Path::new(BENCH_TOKENIZER_DIR));
    registry
        .load(
            MODEL_NAME,
            &TokenizerSource::Local,
            Some(tokenizer_path.to_str().unwrap()),
            None,
        )
        .expect("loading local tokenizer must not fail once the file exists");

    Some(Arc::new(registry))
}

// ---------------------------------------------------------------------------
// Single-text benchmarks
// ---------------------------------------------------------------------------

fn bench_single(c: &mut Criterion) {
    let registry = match build_registry() {
        Some(r) => r,
        None => {
            eprintln!("[bench] skipping single-text benchmarks: no tokenizer available");
            return;
        }
    };

    let cases: &[(&str, &str)] = &[
        ("short (~20 tokens)", TEXT_SHORT),
        ("medium (~200 tokens)", TEXT_MEDIUM),
        ("long (~2000 tokens)", TEXT_LONG),
    ];

    let mut group = c.benchmark_group(MODEL_NAME);

    for (label, text) in cases {
        group.throughput(Throughput::Bytes(text.len() as u64));
        group.bench_with_input(BenchmarkId::new("single", label), text, |b, &input| {
            b.iter(|| {
                registry
                    .tokenize(
                        black_box(MODEL_NAME),
                        black_box(&[input]),
                        true,
                        false,
                    )
                    .expect("tokenize must not fail")
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Batch benchmarks
// ---------------------------------------------------------------------------

fn bench_batch(c: &mut Criterion) {
    let registry = match build_registry() {
        Some(r) => r,
        None => {
            eprintln!("[bench] skipping batch benchmarks: no tokenizer available");
            return;
        }
    };

    let batch_10: Vec<&str> = std::iter::repeat_n(TEXT_SHORT, 10).collect();
    let batch_100: Vec<&str> = std::iter::repeat_n(TEXT_SHORT, 100).collect();

    let cases: &[(&str, &[&str])] = &[
        ("batch-10 short", &batch_10),
        ("batch-100 short", &batch_100),
    ];

    let mut group = c.benchmark_group(MODEL_NAME);

    for (label, texts) in cases {
        group.throughput(Throughput::Elements(texts.len() as u64));
        group.bench_with_input(BenchmarkId::new("batch", label), texts, |b, &input| {
            b.iter(|| {
                registry
                    .tokenize(
                        black_box(MODEL_NAME),
                        black_box(input),
                        true,
                        false,
                    )
                    .expect("tokenize must not fail")
            });
        });
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Concurrent benchmarks — measures DashMap contention and Arc<Tokenizer>
// cloning under parallel access (the production hot path).
// ---------------------------------------------------------------------------

fn bench_concurrent(c: &mut Criterion) {
    let registry = match build_registry() {
        Some(r) => r,
        None => {
            eprintln!("[bench] skipping concurrent benchmarks: no tokenizer available");
            return;
        }
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(8)
        .build()
        .unwrap();

    let cases: &[(&str, &str)] = &[
        ("short (~20 tokens)", TEXT_SHORT),
        ("medium (~200 tokens)", TEXT_MEDIUM),
        ("long (~2000 tokens)", TEXT_LONG),
    ];

    for concurrency in [4, 16, 64] {
        let mut group = c.benchmark_group(format!("concurrent-{concurrency}"));

        for (label, text) in cases {
            group.throughput(Throughput::Elements(concurrency));
            group.bench_with_input(
                BenchmarkId::new("tokenize", label),
                text,
                |b, &input| {
                    b.iter(|| {
                        rt.block_on(async {
                            let mut handles = Vec::with_capacity(concurrency as usize);
                            for _ in 0..concurrency {
                                let reg = registry.clone();
                                handles.push(tokio::spawn(async move {
                                    reg.tokenize(
                                        black_box(MODEL_NAME),
                                        black_box(&[input]),
                                        true,
                                        false,
                                    )
                                    .expect("tokenize must not fail");
                                }));
                            }
                            for h in handles {
                                h.await.unwrap();
                            }
                        });
                    });
                },
            );
        }

        group.finish();
    }
}

criterion_group!(benches, bench_single, bench_batch, bench_concurrent);
criterion_main!(benches);
