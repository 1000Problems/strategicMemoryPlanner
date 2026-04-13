use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;

use super::DigestMessage;

/// Raw message parsed from Claude Code JSONL logs.
/// This handles the known structure from community reverse-engineering.
/// The format is: one JSON object per line, with fields varying by message type.
#[derive(Debug, Deserialize)]
struct RawLogEntry {
    #[serde(default)]
    r#type: String,          // "human", "assistant", "tool_use", "tool_result", "system"

    #[serde(default)]
    role: String,            // Alternative: "user", "assistant"

    #[serde(default)]
    content: Value,          // String or array of content blocks

    #[serde(default)]
    message: Option<Value>,  // Some formats nest the message

    // Tool-specific fields
    #[serde(default)]
    tool_name: String,
    #[serde(default)]
    name: String,            // Alternative tool name field

    #[serde(default)]
    tool_input: Option<Value>,
    #[serde(default)]
    input: Option<Value>,    // Alternative

    // Metadata we might use later
    #[serde(default)]
    timestamp: Option<String>,
}

/// Parse Claude Code JSONL (one JSON object per line).
pub fn parse_jsonl(content: &str) -> Result<Vec<DigestMessage>> {
    let mut messages = Vec::new();

    for (line_num, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        match serde_json::from_str::<RawLogEntry>(line) {
            Ok(entry) => {
                if let Some(msg) = convert_entry(entry) {
                    messages.push(msg);
                }
            }
            Err(e) => {
                // Try parsing as a nested message format
                if let Ok(value) = serde_json::from_str::<Value>(line) {
                    if let Some(msgs) = try_extract_from_value(&value) {
                        messages.extend(msgs);
                        continue;
                    }
                }
                tracing::trace!(line = line_num + 1, error = %e, "Skipping unparseable line");
            }
        }
    }

    Ok(messages)
}

/// Parse a single JSON array of messages (alternative format).
pub fn parse_json_array(content: &str) -> Result<Vec<DigestMessage>> {
    let values: Vec<Value> = serde_json::from_str(content)?;
    let mut messages = Vec::new();

    for value in values {
        if let Ok(entry) = serde_json::from_value::<RawLogEntry>(value.clone()) {
            if let Some(msg) = convert_entry(entry) {
                messages.push(msg);
            }
        } else if let Some(msgs) = try_extract_from_value(&value) {
            messages.extend(msgs);
        }
    }

    Ok(messages)
}

/// Parse plain text transcripts (fallback — just split on role markers).
pub fn parse_plain_text(content: &str) -> Vec<DigestMessage> {
    let mut messages = Vec::new();
    let mut current_role = String::new();
    let mut current_content = String::new();

    for line in content.lines() {
        // Detect role markers: "Human:", "Assistant:", "[user]", "[assistant]", etc.
        let detected_role = detect_role_marker(line);
        if let Some(role) = detected_role {
            // Save previous message
            if !current_content.is_empty() {
                messages.push(DigestMessage {
                    role: current_role.clone(),
                    content: current_content.trim().to_string(),
                    tool_name: String::new(),
                    files_mentioned: extract_file_paths(&current_content),
                });
            }
            current_role = role;
            current_content.clear();
            // Strip the role prefix from the line
            if let Some(after_colon) = line.split_once(':') {
                current_content.push_str(after_colon.1.trim());
                current_content.push('\n');
            }
        } else {
            current_content.push_str(line);
            current_content.push('\n');
        }
    }

    // Don't forget the last message
    if !current_content.is_empty() {
        messages.push(DigestMessage {
            role: current_role,
            content: current_content.trim().to_string(),
            tool_name: String::new(),
            files_mentioned: extract_file_paths(&current_content),
        });
    }

    messages
}

/// Convert a raw log entry into a DigestMessage.
fn convert_entry(entry: RawLogEntry) -> Option<DigestMessage> {
    let role = normalize_role(&entry.r#type, &entry.role);

    // Skip system messages and thinking blocks
    if role == "system" || role == "thinking" {
        return None;
    }

    let content = extract_text_content(&entry.content, entry.message.as_ref());

    // Skip empty messages
    if content.trim().is_empty() {
        return None;
    }

    let tool_name = if !entry.tool_name.is_empty() {
        entry.tool_name
    } else {
        entry.name
    };

    Some(DigestMessage {
        files_mentioned: extract_file_paths(&content),
        role,
        content,
        tool_name,
    })
}

/// Try to extract messages from a generic JSON value.
/// Handles nested formats like { "message": { "role": ..., "content": ... } }
/// and content block arrays.
fn try_extract_from_value(value: &Value) -> Option<Vec<DigestMessage>> {
    let mut messages = Vec::new();

    // Try { "message": { ... } } wrapper
    if let Some(msg) = value.get("message") {
        if let Ok(entry) = serde_json::from_value::<RawLogEntry>(msg.clone()) {
            if let Some(m) = convert_entry(entry) {
                messages.push(m);
                return Some(messages);
            }
        }
    }

    // Try content block array: { "role": "assistant", "content": [...] }
    if let Some(role) = value.get("role").and_then(|r| r.as_str()) {
        if let Some(content_blocks) = value.get("content").and_then(|c| c.as_array()) {
            for block in content_blocks {
                let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");

                match block_type {
                    "text" => {
                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                            messages.push(DigestMessage {
                                role: role.to_string(),
                                content: text.to_string(),
                                tool_name: String::new(),
                                files_mentioned: extract_file_paths(text),
                            });
                        }
                    }
                    "tool_use" => {
                        let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("unknown");
                        let input = block.get("input")
                            .map(|i| serde_json::to_string_pretty(i).unwrap_or_default())
                            .unwrap_or_default();
                        messages.push(DigestMessage {
                            role: "tool_use".to_string(),
                            content: input.clone(),
                            tool_name: name.to_string(),
                            files_mentioned: extract_file_paths(&input),
                        });
                    }
                    "tool_result" => {
                        let text = extract_text_from_value(block);
                        if !text.is_empty() {
                            messages.push(DigestMessage {
                                role: "tool_result".to_string(),
                                content: text.clone(),
                                tool_name: String::new(),
                                files_mentioned: extract_file_paths(&text),
                            });
                        }
                    }
                    "thinking" => {
                        // Skip thinking blocks entirely — they're expensive and
                        // the decisions are restated in the assistant text.
                    }
                    _ => {}
                }
            }
            if !messages.is_empty() {
                return Some(messages);
            }
        }
    }

    None
}

/// Normalize role strings from various formats.
fn normalize_role(type_field: &str, role_field: &str) -> String {
    let raw = if !type_field.is_empty() { type_field } else { role_field };
    match raw.to_lowercase().as_str() {
        "human" | "user" => "user".to_string(),
        "assistant" | "ai" => "assistant".to_string(),
        "tool_use" | "tooluse" => "tool_use".to_string(),
        "tool_result" | "toolresult" => "tool_result".to_string(),
        "system" => "system".to_string(),
        "thinking" => "thinking".to_string(),
        other => other.to_string(),
    }
}

/// Extract text from various content formats.
fn extract_text_content(content: &Value, message: Option<&Value>) -> String {
    // Direct string content
    if let Some(s) = content.as_str() {
        return s.to_string();
    }

    // Array of content blocks — extract text blocks only
    if let Some(blocks) = content.as_array() {
        let texts: Vec<String> = blocks.iter()
            .filter_map(|block| {
                let block_type = block.get("type").and_then(|t| t.as_str());
                match block_type {
                    Some("text") => block.get("text").and_then(|t| t.as_str()).map(String::from),
                    Some("thinking") => None, // Skip thinking
                    _ => None,
                }
            })
            .collect();
        if !texts.is_empty() {
            return texts.join("\n");
        }
    }

    // Nested message object
    if let Some(msg) = message {
        if let Some(s) = msg.get("content").and_then(|c| c.as_str()) {
            return s.to_string();
        }
    }

    String::new()
}

/// Extract text from a content value (handles string, array of blocks, etc.)
fn extract_text_from_value(value: &Value) -> String {
    if let Some(s) = value.get("content").and_then(|c| c.as_str()) {
        return s.to_string();
    }
    if let Some(s) = value.get("text").and_then(|t| t.as_str()) {
        return s.to_string();
    }
    if let Some(s) = value.get("output").and_then(|o| o.as_str()) {
        return s.to_string();
    }
    String::new()
}

/// Detect role markers in plain text transcripts.
fn detect_role_marker(line: &str) -> Option<String> {
    let trimmed = line.trim().to_lowercase();
    if trimmed.starts_with("human:") || trimmed.starts_with("[user]") {
        Some("user".to_string())
    } else if trimmed.starts_with("assistant:") || trimmed.starts_with("[assistant]") {
        Some("assistant".to_string())
    } else if trimmed.starts_with("tool:") || trimmed.starts_with("[tool") {
        Some("tool_result".to_string())
    } else {
        None
    }
}

/// Extract file paths from text content.
/// Looks for common patterns: src/..., ./..., /path/to/file, *.rs, etc.
fn extract_file_paths(text: &str) -> Vec<String> {
    let mut paths = Vec::new();

    for word in text.split_whitespace() {
        let clean = word.trim_matches(|c: char| c == '`' || c == '"' || c == '\'' || c == ',' || c == ')' || c == '(');

        // Match file-like patterns
        if (clean.contains('/') && clean.contains('.') && !clean.starts_with("http"))
            || clean.ends_with(".rs")
            || clean.ends_with(".ts")
            || clean.ends_with(".tsx")
            || clean.ends_with(".js")
            || clean.ends_with(".py")
            || clean.ends_with(".toml")
            || clean.ends_with(".json")
            || clean.ends_with(".sql")
            || clean.ends_with(".md")
        {
            if clean.len() > 3 && clean.len() < 200 {
                paths.push(clean.to_string());
            }
        }
    }

    paths.sort();
    paths.dedup();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_jsonl_basic() {
        let input = r#"{"role": "user", "content": "Fix the bug in src/main.rs"}
{"role": "assistant", "content": "I'll fix the null check in the handler"}
{"role": "system", "content": "System prompt here"}
"#;
        let messages = parse_jsonl(input).unwrap();
        assert_eq!(messages.len(), 2); // system filtered out
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[1].role, "assistant");
    }

    #[test]
    fn test_parse_content_blocks() {
        let input = r#"{"role": "assistant", "content": [
            {"type": "thinking", "thinking": "Let me analyze..."},
            {"type": "text", "text": "I'll use exponential backoff for the reconnection logic."},
            {"type": "tool_use", "name": "Edit", "input": {"file": "src/ws.rs", "content": "..."}}
        ]}"#;
        let messages = parse_jsonl(input).unwrap();
        // Should get: text block + tool_use, no thinking
        assert!(messages.iter().any(|m| m.role == "assistant" && m.content.contains("exponential")));
        assert!(messages.iter().any(|m| m.role == "tool_use" && m.tool_name == "Edit"));
        assert!(!messages.iter().any(|m| m.content.contains("Let me analyze")));
    }

    #[test]
    fn test_extract_file_paths() {
        let text = "I'll modify `src/ws/client.rs` and create tests/ws_test.rs for the new logic";
        let paths = extract_file_paths(text);
        assert!(paths.contains(&"src/ws/client.rs".to_string()));
        assert!(paths.contains(&"tests/ws_test.rs".to_string()));
    }

    #[test]
    fn test_parse_plain_text() {
        let input = "Human: Fix the auth bug\nThe login is broken on mobile\nAssistant: I see the issue in the token refresh logic.";
        let messages = parse_plain_text(input);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert!(messages[0].content.contains("login is broken"));
        assert_eq!(messages[1].role, "assistant");
    }
}
