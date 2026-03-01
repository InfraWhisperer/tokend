use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub tokenizers: Vec<TokenizerConfig>,
    pub server: ServerConfig,
    #[serde(default = "default_cache_dir")]
    pub cache_dir: PathBuf,
    #[serde(default)]
    pub hf_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenizerConfig {
    pub model: String,
    #[serde(default = "default_source")]
    pub source: TokenizerSource,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TokenizerSource {
    Huggingface,
    Local,
}

fn default_source() -> TokenizerSource {
    TokenizerSource::Huggingface
}

fn default_cache_dir() -> PathBuf {
    dirs_next().unwrap_or_else(|| PathBuf::from("/tmp/tokend/cache"))
}

fn dirs_next() -> Option<PathBuf> {
    home::home_dir().map(|h| h.join(".cache").join("tokend"))
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    #[serde(default)]
    pub uds_path: Option<String>,
    #[serde(default = "default_http_port")]
    pub http_port: u16,
    #[serde(default = "default_grpc_port")]
    pub grpc_port: u16,
}

fn default_http_port() -> u16 {
    8765
}

fn default_grpc_port() -> u16 {
    8766
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("failed to read config {path}: {e}"))?;
        let expanded = expand_env_vars(&raw);
        let mut config: Config = serde_yaml::from_str(&expanded)
            .map_err(|e| anyhow::anyhow!("failed to parse config {path}: {e}"))?;

        // Expand ~ in cache_dir
        if let Some(stripped) = config.cache_dir.to_str().and_then(|s| s.strip_prefix("~/"))
            && let Some(home) = home::home_dir()
        {
            config.cache_dir = home.join(stripped);
        }

        // If hf_token is empty string after expansion, treat as None
        if config.hf_token.as_deref() == Some("") {
            config.hf_token = None;
        }

        Ok(config)
    }
}

/// Expand ${VAR} patterns in a string using environment variables.
/// Unset variables expand to empty string.
fn expand_env_vars(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '$' && chars.peek() == Some(&'{') {
            chars.next(); // consume '{'
            let mut var_name = String::new();
            for ch in chars.by_ref() {
                if ch == '}' {
                    break;
                }
                var_name.push(ch);
            }
            if let Ok(val) = std::env::var(&var_name) {
                result.push_str(&val);
            }
        } else {
            result.push(c);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_env_vars() {
        // SAFETY: test is single-threaded, no concurrent env access
        unsafe { std::env::set_var("TOKEND_TEST_VAR", "hello") };
        assert_eq!(expand_env_vars("${TOKEND_TEST_VAR}"), "hello");
        assert_eq!(
            expand_env_vars("pre_${TOKEND_TEST_VAR}_post"),
            "pre_hello_post"
        );
        assert_eq!(expand_env_vars("${TOKEND_NONEXISTENT}"), "");
        assert_eq!(expand_env_vars("no vars here"), "no vars here");
        unsafe { std::env::remove_var("TOKEND_TEST_VAR") };
    }

    #[test]
    fn test_parse_minimal_config() {
        let yaml = r#"
tokenizers: []
server:
  http_port: 9999
  grpc_port: 9998
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.tokenizers.is_empty());
        assert_eq!(config.server.http_port, 9999);
        assert_eq!(config.server.grpc_port, 9998);
        assert!(config.server.uds_path.is_none());
    }

    #[test]
    fn test_parse_full_config() {
        let yaml = r#"
tokenizers:
  - model: "test/model-1"
    source: "huggingface"
  - model: "local-model"
    source: "local"
    path: "/models/local/tokenizer.json"
server:
  uds_path: "/tmp/test.sock"
  http_port: 8765
  grpc_port: 8766
cache_dir: "/tmp/tokend-test-cache"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.tokenizers.len(), 2);
        assert_eq!(config.tokenizers[0].source, TokenizerSource::Huggingface);
        assert_eq!(config.tokenizers[1].source, TokenizerSource::Local);
        assert_eq!(
            config.tokenizers[1].path.as_deref(),
            Some("/models/local/tokenizer.json")
        );
    }
}
