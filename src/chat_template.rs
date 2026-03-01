use minijinja::{Environment, ErrorKind};
use minijinja_contrib::pycompat;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// OpenAI-compatible message types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunction {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Chat template engine
// ---------------------------------------------------------------------------

/// Compiled chat template for a single model. Constructed once at model load
/// time; `render()` is called on the hot path.
pub struct ChatTemplate {
    env: &'static Environment<'static>,
    template_name: &'static str,
    bos_token: Option<String>,
    eos_token: Option<String>,
}

// Safety: Environment is compiled once and never mutated after construction.
// All references are 'static (leaked). Render takes &self and is reentrant.
unsafe impl Send for ChatTemplate {}
unsafe impl Sync for ChatTemplate {}

fn raise_exception(err_text: String) -> Result<String, minijinja::Error> {
    Err(minijinja::Error::new(ErrorKind::InvalidOperation, err_text))
}

impl ChatTemplate {
    /// Compile a chat template from the raw Jinja2 source string.
    ///
    /// The environment and template source are leaked to obtain 'static
    /// lifetimes, matching the TGI pattern — each model gets one allocation
    /// that lives for the process lifetime.
    pub fn new(
        template_source: &str,
        bos_token: Option<String>,
        eos_token: Option<String>,
    ) -> Result<Self, minijinja::Error> {
        let mut env = Environment::new();

        // Python method compatibility (.strip(), .items(), .keys(), etc.)
        env.set_unknown_method_callback(pycompat::unknown_method_callback);

        // Custom functions required by HF templates
        env.add_function("raise_exception", raise_exception);

        // Apply known template fixups (from TGI experience):
        // - Python slice reversal → minijinja reverse filter
        // - Training-only generation tags → strip
        let patched = template_source
            .replace("[::-1]", "|reverse")
            .replace("{% generation %}", "")
            .replace("{% endgeneration %}", "");

        let leaked_source: &'static str = Box::leak(patched.into_boxed_str());
        let leaked_name: &'static str = Box::leak("chat_template".to_string().into_boxed_str());

        let env = Box::leak(Box::new(env));
        env.add_template(leaked_name, leaked_source)?;

        Ok(Self {
            env,
            template_name: leaked_name,
            bos_token,
            eos_token,
        })
    }

    /// Render the chat template with the given messages and options.
    pub fn render(
        &self,
        messages: &[ChatMessage],
        add_generation_prompt: bool,
        tools: Option<&[Tool]>,
    ) -> Result<String, minijinja::Error> {
        let tmpl = self.env.get_template(self.template_name)?;

        let ctx = minijinja::context! {
            messages => messages,
            bos_token => self.bos_token.as_deref().unwrap_or(""),
            eos_token => self.eos_token.as_deref().unwrap_or(""),
            add_generation_prompt => add_generation_prompt,
            tools => tools,
        };

        tmpl.render(ctx)
    }
}

// ---------------------------------------------------------------------------
// tokenizer_config.json parsing
// ---------------------------------------------------------------------------

/// Extract the chat_template string from a tokenizer_config.json value.
///
/// The `chat_template` field is typically a string, but some models use an
/// array of `{name, template}` objects (e.g., for tool-use variants). We
/// take the first "default" template or fall back to the first entry.
pub fn extract_chat_template(config: &serde_json::Value) -> Option<String> {
    match config.get("chat_template")? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(arr) => {
            // Look for a "default" named template first
            for entry in arr {
                if entry.get("name").and_then(|n| n.as_str()) == Some("default") {
                    return entry.get("template").and_then(|t| t.as_str()).map(|s| s.to_string());
                }
            }
            // Fall back to first entry
            arr.first()
                .and_then(|e| e.get("template"))
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
        }
        _ => None,
    }
}

/// Extract a special token (bos_token / eos_token) from tokenizer_config.json.
///
/// These can be either a plain string or an object with a "content" field.
pub fn extract_special_token(config: &serde_json::Value, key: &str) -> Option<String> {
    match config.get(key)? {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(obj) => {
            obj.get("content").and_then(|v| v.as_str()).map(|s| s.to_string())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_chatml_template() {
        let template = "\
{%- for message in messages %}\
<|im_start|>{{ message.role }}\n\
{{ message.content }}<|im_end|>\n\
{%- endfor %}\
{%- if add_generation_prompt %}\
<|im_start|>assistant\n\
{%- endif %}";

        let ct = ChatTemplate::new(template, None, Some("</s>".to_string())).unwrap();

        let messages = vec![
            ChatMessage {
                role: "system".into(),
                content: Some("You are helpful.".into()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
            ChatMessage {
                role: "user".into(),
                content: Some("Hello".into()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            },
        ];

        let rendered = ct.render(&messages, true, None).unwrap();
        assert!(rendered.contains("<|im_start|>system"));
        assert!(rendered.contains("You are helpful."));
        assert!(rendered.contains("<|im_start|>user"));
        assert!(rendered.contains("Hello"));
        assert!(rendered.contains("<|im_start|>assistant"));
    }

    #[test]
    fn test_extract_chat_template_string() {
        let config = serde_json::json!({
            "chat_template": "{{ bos_token }}{% for m in messages %}{{ m.content }}{% endfor %}"
        });
        let tmpl = extract_chat_template(&config).unwrap();
        assert!(tmpl.contains("bos_token"));
    }

    #[test]
    fn test_extract_chat_template_array() {
        let config = serde_json::json!({
            "chat_template": [
                {"name": "tool_use", "template": "tool template"},
                {"name": "default", "template": "default template"}
            ]
        });
        let tmpl = extract_chat_template(&config).unwrap();
        assert_eq!(tmpl, "default template");
    }

    #[test]
    fn test_extract_special_token_string() {
        let config = serde_json::json!({ "bos_token": "<s>" });
        assert_eq!(extract_special_token(&config, "bos_token").unwrap(), "<s>");
    }

    #[test]
    fn test_extract_special_token_object() {
        let config = serde_json::json!({
            "eos_token": { "content": "</s>", "__type": "AddedToken" }
        });
        assert_eq!(
            extract_special_token(&config, "eos_token").unwrap(),
            "</s>"
        );
    }
}
