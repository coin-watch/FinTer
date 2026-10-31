//! OpenAI-compatible, read-only dashboard analysis client with automatic model discovery.
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ChatMessageRecord {
    pub prompt: String,
    pub response: String,
    #[serde(default)]
    pub failed: bool,
    #[serde(default)]
    pub timestamp: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ConversationRecord {
    pub id: String,
    pub title: String,
    pub updated_at: String,
    pub updated_at_ts: i64,
    pub messages: Vec<ChatMessageRecord>,
}

fn conversations_file_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|p| p.join("finter/agent_conversations.json"))
}

/// Loads all saved conversations sorted from newest to oldest.
pub fn load_all_conversations() -> Vec<ConversationRecord> {
    let Some(path) = conversations_file_path() else { return Vec::new(); };
    let Ok(bytes) = std::fs::read(&path) else {
        // Also check if any legacy individual files exist in agent-history/ and import them
        let mut migrated = Vec::new();
        if let Some(dir) = dirs::config_dir().map(|p| p.join("finter/agent-history")) {
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.flatten() {
                    let file_path = entry.path();
                    if file_path.extension().and_then(|s| s.to_str()) == Some("json") {
                        if let Ok(data) = std::fs::read(&file_path) {
                            #[derive(serde::Deserialize)]
                            struct LegacyMsg { prompt: String, response: String, #[serde(default)] failed: bool }
                            if let Ok(msgs) = serde_json::from_slice::<Vec<LegacyMsg>>(&data) {
                                if !msgs.is_empty() {
                                    let id = file_path.file_stem().and_then(|s| s.to_str()).unwrap_or("migrated").to_string();
                                    let title = msgs.first().map(|m| m.prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or(&m.prompt).chars().take(40).collect::<String>()).unwrap_or_else(|| "Past Conversation".into());
                                    let now = chrono::Local::now();
                                    migrated.push(ConversationRecord {
                                        id,
                                        title,
                                        updated_at: now.format("%b %d, %H:%M").to_string(),
                                        updated_at_ts: now.timestamp_millis(),
                                        messages: msgs.into_iter().map(|m| ChatMessageRecord {
                                            prompt: m.prompt,
                                            response: m.response,
                                            failed: m.failed,
                                            timestamp: None,
                                        }).collect(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        if !migrated.is_empty() {
            let _ = save_all_conversations(&migrated);
        }
        return migrated;
    };
    let mut list = serde_json::from_slice::<Vec<ConversationRecord>>(&bytes).unwrap_or_default();
    list.sort_by(|a, b| b.updated_at_ts.cmp(&a.updated_at_ts));
    list
}

pub fn load_conversation(id: &str) -> Option<ConversationRecord> {
    load_all_conversations().into_iter().find(|c| c.id == id)
}

pub fn save_conversation(conv: &ConversationRecord) -> Result<()> {
    let mut list = load_all_conversations();
    if let Some(existing) = list.iter_mut().find(|c| c.id == conv.id) {
        *existing = conv.clone();
    } else {
        list.insert(0, conv.clone());
    }
    list.sort_by(|a, b| b.updated_at_ts.cmp(&a.updated_at_ts));
    save_all_conversations(&list)
}

pub fn delete_conversation(id: &str) -> Result<()> {
    let mut list = load_all_conversations();
    list.retain(|c| c.id != id);
    save_all_conversations(&list)
}

pub fn save_all_conversations(list: &[ConversationRecord]) -> Result<()> {
    use std::io::Write;
    let path = conversations_file_path().context("Config directory unavailable")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(list)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(temporary, path)?;
    Ok(())
}

const MAX_CONTEXT_BYTES: usize = 200_000;

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct LocalConfig {
    pub base_url: String,
    pub api_key: String,
}

pub fn provider_config() -> LocalConfig {
    let saved = dirs::config_dir().and_then(|p| std::fs::read(p.join("finter/agent_config.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<LocalConfig>(&bytes).ok());
    saved.unwrap_or_else(|| LocalConfig {
        base_url: std::env::var("FINTER_AI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into()),
        api_key: std::env::var("FINTER_AI_API_KEY").unwrap_or_default(),
    })
}

pub fn save_provider_config(config: &LocalConfig) -> Result<()> {
    use std::io::Write;
    let dir = dirs::config_dir().context("Local configuration directory unavailable")?.join("finter");
    std::fs::create_dir_all(&dir)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(dir.join("agent_config.json"))?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(&serde_json::to_vec_pretty(config)?)?;
    Ok(())
}

/// Discovers available models from OpenAI, OpenRouter, 9router, Ollama, and other OpenAI-compatible proxies.
pub async fn discover_models() -> Result<Vec<String>> {
    let cfg = provider_config();
    let key = if !cfg.api_key.trim().is_empty() {
        Some(cfg.api_key)
    } else {
        std::env::var("FINTER_AI_API_KEY").ok().filter(|k| !k.trim().is_empty())
    };
    let raw_base = if !cfg.base_url.trim().is_empty() {
        cfg.base_url
    } else {
        std::env::var("FINTER_AI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into())
    };
    let clean_base = raw_base
        .trim_end_matches('/')
        .trim_end_matches("/chat/completions")
        .trim_end_matches("/models");
    let url = format!("{}/models", clean_base);
    reqwest::Url::parse(&url).context("FINTER_AI_BASE_URL is invalid")?;

    let mut request = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .build()?
        .get(&url)
        .header("User-Agent", "FinTer/0.1.4")
        .header("HTTP-Referer", "https://finter.app")
        .header("X-Title", "FinTer");

    if let Some(ref k) = key {
        request = request.bearer_auth(k);
    }

    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        let err_text = response.text().await.unwrap_or_default();
        return Err(anyhow!("Model discovery failed ({status}): {err_text}"));
    }

    let value: Value = response.json().await?;
    let mut models = Vec::new();

    // 1. Check standard OpenAI / OpenRouter / 9router `{ "data": [ ... ] }`
    if let Some(arr) = value.get("data").and_then(Value::as_array) {
        for item in arr {
            if let Some(id) = item.get("id").and_then(Value::as_str) {
                models.push(id.to_string());
            } else if let Some(id) = item.get("name").and_then(Value::as_str) {
                models.push(id.to_string());
            } else if let Some(s) = item.as_str() {
                models.push(s.to_string());
            }
        }
    } else if let Some(arr) = value.get("models").and_then(Value::as_array) {
        // 2. Check Ollama or alternate `{ "models": [ ... ] }`
        for item in arr {
            if let Some(id) = item.get("name").or_else(|| item.get("id")).and_then(Value::as_str) {
                models.push(id.to_string());
            } else if let Some(s) = item.as_str() {
                models.push(s.to_string());
            }
        }
    } else if let Some(arr) = value.as_array() {
        // 3. Top-level array `[ { "id": ... } ]`
        for item in arr {
            if let Some(id) = item.get("id").or_else(|| item.get("name")).and_then(Value::as_str) {
                models.push(id.to_string());
            } else if let Some(s) = item.as_str() {
                models.push(s.to_string());
            }
        }
    }

    models.retain(|m| !m.trim().is_empty());
    models.sort();
    models.dedup();

    if models.is_empty() {
        return Err(anyhow!("Provider returned no models. Check the API base URL and credentials in Settings, then refresh models."));
    }

    Ok(models)
}

/// Sends read-only dashboard context and user prompt to the selected model.
pub async fn analyze(prompt: &str, context: Value, selected_model: &str) -> Result<String> {
    let cfg = provider_config();
    let key = if !cfg.api_key.trim().is_empty() {
        Some(cfg.api_key)
    } else {
        std::env::var("FINTER_AI_API_KEY").ok().filter(|k| !k.trim().is_empty())
    };
    let raw_base = if !cfg.base_url.trim().is_empty() {
        cfg.base_url
    } else {
        std::env::var("FINTER_AI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".into())
    };
    let clean_base = raw_base
        .trim_end_matches('/')
        .trim_end_matches("/chat/completions")
        .trim_end_matches("/models");
    let url = format!("{}/chat/completions", clean_base);
    reqwest::Url::parse(&url).context("FINTER_AI_BASE_URL is invalid")?;

    let model = if selected_model.trim().is_empty() || selected_model == "Select model" {
        std::env::var("FINTER_AI_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into())
    } else {
        selected_model.to_string()
    };

    let mut serialized = serde_json::to_string(&context)?;
    if serialized.len() > MAX_CONTEXT_BYTES {
        let mut end = MAX_CONTEXT_BYTES;
        while !serialized.is_char_boundary(end) { end -= 1; }
        serialized.truncate(end);
        serialized.push_str("…[truncated]");
    }

    let body = json!({
        "model": model,
        // Some OpenAI-compatible proxies default to SSE streaming. We consume a
        // regular JSON response here because the UI performs its own typing effect.
        "stream": false,
        "messages": [
            {
                "role": "system",
                "content": "You are FinTer's read-only market analysis agent. Analyse only the supplied dashboard snapshot, distinguish facts from inference, explicitly identify missing/stale data, and never claim to place or execute trades. This is informational, not financial advice."
            },
            {
                "role": "user",
                "content": format!("Dashboard snapshot:\n{}\n\nUser request:\n{}", serialized, prompt)
            }
        ]
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let mut request = client
        .post(&url)
        .header("User-Agent", "FinTer/0.1.4")
        .header("HTTP-Referer", "https://finter.app")
        .header("X-Title", "FinTer")
        .json(&body);

    if let Some(ref k) = key {
        request = request.bearer_auth(k);
    }

    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        let err_text = response.text().await.unwrap_or_default();
        let is_credits_error = status.as_u16() == 402
            || (status.as_u16() == 429 && (err_text.to_lowercase().contains("quota") || err_text.to_lowercase().contains("credit")))
            || err_text.to_lowercase().contains("insufficient_quota")
            || err_text.to_lowercase().contains("insufficient_credits");

        if is_credits_error {
            crate::data::credits::CreditStore::record(
                "AI Agent",
                &format!("AI request failed: insufficient credits or quota ({status})"),
                0,
                0,
                None,
            );
        }
        return Err(anyhow!("AI request failed ({status}): {err_text}"));
    }

    let value: Value = response.json().await?;
    value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("AI provider returned no message content"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_limit_is_bounded() {
        assert!(MAX_CONTEXT_BYTES <= 200_000);
    }

    #[test]
    fn clean_base_url_logic() {
        for (raw, expected) in [
            ("https://api.openai.com/v1", "https://api.openai.com/v1/models"),
            ("https://api.openai.com/v1/", "https://api.openai.com/v1/models"),
            ("https://openrouter.ai/api/v1/chat/completions", "https://openrouter.ai/api/v1/models"),
            ("http://localhost:11434/v1", "http://localhost:11434/v1/models"),
        ] {
            let clean = raw
                .trim_end_matches('/')
                .trim_end_matches("/chat/completions")
                .trim_end_matches("/models");
            assert_eq!(format!("{}/models", clean), expected);
        }
    }
}
