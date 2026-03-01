use crate::config::{Config, TokenizerSource};
use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokenizers::Tokenizer;
use tracing::{error, info, warn};

#[derive(Debug, thiserror::Error)]
pub enum TokenizerError {
    #[error("model not loaded: {0}")]
    ModelNotFound(String),
    #[error("model already loaded: {0}")]
    AlreadyLoaded(String),
    #[error("failed to load tokenizer for {model}: {source}")]
    LoadFailed {
        model: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("tokenization failed for {model}: {source}")]
    EncodeFailed {
        model: String,
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

pub struct TokenResult {
    pub token_ids: Vec<u32>,
    pub token_count: u32,
    pub tokens: Option<Vec<String>>,
}

pub struct TokenizerRegistry {
    tokenizers: DashMap<String, Arc<Tokenizer>>,
    cache_dir: PathBuf,
}

impl TokenizerRegistry {
    pub fn new(cache_dir: &Path) -> Self {
        Self {
            tokenizers: DashMap::new(),
            cache_dir: cache_dir.to_path_buf(),
        }
    }

    /// Load a tokenizer from HuggingFace Hub or local path.
    pub fn load(
        &self,
        model: &str,
        source: &TokenizerSource,
        path: Option<&str>,
        hf_token: Option<&str>,
    ) -> Result<(), TokenizerError> {
        if self.tokenizers.contains_key(model) {
            return Err(TokenizerError::AlreadyLoaded(model.to_string()));
        }

        let tokenizer = match source {
            TokenizerSource::Huggingface => self.load_from_hub(model, hf_token)?,
            TokenizerSource::Local => {
                let p = path.ok_or_else(|| TokenizerError::LoadFailed {
                    model: model.to_string(),
                    source: "local source requires a path".into(),
                })?;
                Tokenizer::from_file(p).map_err(|e| TokenizerError::LoadFailed {
                    model: model.to_string(),
                    source: e,
                })?
            }
        };

        self.tokenizers
            .insert(model.to_string(), Arc::new(tokenizer));
        info!(model, "tokenizer loaded");
        Ok(())
    }

    fn load_from_hub(
        &self,
        model: &str,
        hf_token: Option<&str>,
    ) -> Result<Tokenizer, TokenizerError> {
        // Check local cache first
        let cache_path = self.cache_path(model);
        if cache_path.exists() {
            info!(model, path = %cache_path.display(), "loading tokenizer from cache");
            return Tokenizer::from_file(&cache_path).map_err(|e| {
                warn!(model, error = %e, "cached tokenizer failed to load, re-downloading");
                TokenizerError::LoadFailed {
                    model: model.to_string(),
                    source: e,
                }
            });
        }

        info!(model, "downloading tokenizer from HuggingFace Hub");
        let params = hf_token.map(|token| tokenizers::FromPretrainedParameters {
            token: Some(token.to_string()),
            ..Default::default()
        });

        let tokenizer =
            Tokenizer::from_pretrained(model, params).map_err(|e| TokenizerError::LoadFailed {
                model: model.to_string(),
                source: e,
            })?;

        // Cache to disk
        if let Some(parent) = cache_path.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            warn!(model, error = %e, "failed to create cache directory");
        }
        if let Err(e) = tokenizer.save(&cache_path, false) {
            warn!(model, error = %e, "failed to cache tokenizer");
        } else {
            info!(model, path = %cache_path.display(), "tokenizer cached");
        }

        Ok(tokenizer)
    }

    fn cache_path(&self, model: &str) -> PathBuf {
        // model names like "meta-llama/Llama-3.1-70B" → "meta-llama--Llama-3.1-70B"
        let safe_name = model.replace('/', "--");
        self.cache_dir.join(&safe_name).join("tokenizer.json")
    }

    /// Unload a tokenizer, freeing memory.
    pub fn unload(&self, model: &str) -> Result<(), TokenizerError> {
        self.tokenizers
            .remove(model)
            .map(|_| {
                info!(model, "tokenizer unloaded");
            })
            .ok_or_else(|| TokenizerError::ModelNotFound(model.to_string()))
    }

    /// Tokenize one or more texts using the specified model.
    pub fn tokenize(
        &self,
        model: &str,
        texts: &[&str],
        add_special_tokens: bool,
        return_tokens: bool,
    ) -> Result<Vec<TokenResult>, TokenizerError> {
        let entry = self
            .tokenizers
            .get(model)
            .ok_or_else(|| TokenizerError::ModelNotFound(model.to_string()))?;
        let tokenizer = entry.value().clone();
        // Drop the DashMap ref before doing work to avoid holding the shard lock
        drop(entry);

        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            let encoding =
                tokenizer
                    .encode(*text, add_special_tokens)
                    .map_err(|e| TokenizerError::EncodeFailed {
                        model: model.to_string(),
                        source: e,
                    })?;
            let token_ids = encoding.get_ids().to_vec();
            let token_count = token_ids.len() as u32;
            let tokens = if return_tokens {
                Some(encoding.get_tokens().to_vec())
            } else {
                None
            };
            results.push(TokenResult {
                token_ids,
                token_count,
                tokens,
            });
        }

        Ok(results)
    }

    /// Load all tokenizers from config. Logs failures but continues.
    pub fn load_from_config(&self, config: &Config) {
        for tc in &config.tokenizers {
            if let Err(e) = self.load(
                &tc.model,
                &tc.source,
                tc.path.as_deref(),
                config.hf_token.as_deref(),
            ) {
                error!(model = %tc.model, error = %e, "failed to load tokenizer, skipping");
            }
        }
    }

    pub fn list_models(&self) -> Vec<String> {
        self.tokenizers.iter().map(|e| e.key().clone()).collect()
    }

    pub fn model_count(&self) -> usize {
        self.tokenizers.len()
    }
}
