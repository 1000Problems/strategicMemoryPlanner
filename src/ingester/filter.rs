use crate::config::IngesterConfig;
use super::DigestMessage;

/// Filter and compress parsed messages into a lean digest.
///
/// This is where the real token savings happen — before the LLM ever sees the text.
/// A typical session has 70-90% noise (retries, verbose tool output, system fluff).
pub fn filter_messages(messages: Vec<DigestMessage>, config: &IngesterConfig) -> Vec<DigestMessage> {
    let mut filtered: Vec<DigestMessage> = Vec::new();

    let mut last_edit_file: Option<String> = None;
    let mut edit_count: usize = 0;

    for msg in messages {
        // === SKIP: Empty content ===
        if msg.content.trim().is_empty() {
            continue;
        }

        // === SKIP: Thinking-only messages that leaked through ===
        if msg.role == "thinking" {
            continue;
        }

        // === TRUNCATE: Long tool results ===
        if msg.role == "tool_result" {
            let token_est = msg.content.len() / 4;
            if token_est > config.max_tool_result_tokens {
                let truncated = truncate_to_tokens(&msg.content, config.max_tool_result_tokens);
                filtered.push(DigestMessage {
                    content: format!("{}\n[... truncated from ~{} tokens]", truncated, token_est),
                    ..msg
                });
                continue;
            }
        }

        // === COLLAPSE: Repeated edits to the same file ===
        if config.collapse_repeated_edits && msg.role == "tool_use" && is_edit_tool(&msg.tool_name) {
            let target_file = extract_edit_target(&msg.content);
            if let Some(ref target) = target_file {
                if last_edit_file.as_ref() == Some(target) {
                    edit_count += 1;
                    // Replace the last message with a summary
                    if let Some(last) = filtered.last_mut() {
                        if is_edit_tool(&last.tool_name) {
                            last.content = format!(
                                "[{} edits to {} — showing latest]\n{}",
                                edit_count + 1,
                                target,
                                msg.content
                            );
                            continue;
                        }
                    }
                } else {
                    edit_count = 0;
                    last_edit_file = Some(target.clone());
                }
            }
        } else if msg.role != "tool_result" {
            // Reset edit tracking when we see a non-edit message
            last_edit_file = None;
            edit_count = 0;
        }

        // === DEDUPLICATE: Retry messages ===
        // If the assistant says the exact same thing twice in a row, keep only the last
        if msg.role == "assistant" {
            if let Some(last) = filtered.last() {
                if last.role == "assistant" && last.content == msg.content {
                    continue; // Skip duplicate
                }
            }
        }

        // === COMPRESS: Strip excessive whitespace and formatting noise ===
        let cleaned_content = compress_content(&msg.content);

        filtered.push(DigestMessage {
            content: cleaned_content,
            ..msg
        });
    }

    filtered
}

/// Truncate text to approximately N tokens (1 token ≈ 4 chars).
fn truncate_to_tokens(text: &str, max_tokens: usize) -> String {
    let max_chars = max_tokens * 4;
    if text.len() <= max_chars {
        return text.to_string();
    }

    // Find a clean line break near the limit
    let slice = &text[..max_chars];
    if let Some(last_newline) = slice.rfind('\n') {
        text[..last_newline].to_string()
    } else {
        slice.to_string()
    }
}

/// Check if a tool name is an edit/write tool.
fn is_edit_tool(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("edit")
        || lower.contains("write")
        || lower.contains("replace")
        || lower.contains("patch")
}

/// Try to extract the target file from an edit tool's content.
fn extract_edit_target(content: &str) -> Option<String> {
    // Try JSON parse for structured tool input
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(content) {
        // Common field names for file paths in tool inputs
        for key in &["file_path", "path", "file", "filename"] {
            if let Some(path) = value.get(key).and_then(|v| v.as_str()) {
                return Some(path.to_string());
            }
        }
    }

    // Fallback: look for file path patterns in the content
    for word in content.split_whitespace() {
        let clean = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != '.' && c != '_' && c != '-');
        if clean.contains('/') && clean.contains('.') && !clean.starts_with("http") {
            return Some(clean.to_string());
        }
    }

    None
}

/// Compress content: collapse multiple blank lines, strip trailing spaces.
fn compress_content(content: &str) -> String {
    let mut result = String::new();
    let mut blank_count = 0;

    for line in content.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_count += 1;
            if blank_count <= 1 {
                result.push('\n');
            }
            // Skip additional blank lines
        } else {
            blank_count = 0;
            result.push_str(trimmed);
            result.push('\n');
        }
    }

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> IngesterConfig {
        IngesterConfig {
            max_tool_result_tokens: 100,
            collapse_repeated_edits: true,
        }
    }

    fn msg(role: &str, content: &str) -> DigestMessage {
        DigestMessage {
            role: role.to_string(),
            content: content.to_string(),
            tool_name: String::new(),
            files_mentioned: Vec::new(),
        }
    }

    fn tool_msg(role: &str, tool: &str, content: &str) -> DigestMessage {
        DigestMessage {
            role: role.to_string(),
            content: content.to_string(),
            tool_name: tool.to_string(),
            files_mentioned: Vec::new(),
        }
    }

    #[test]
    fn test_skip_empty() {
        let messages = vec![msg("user", "hello"), msg("assistant", "   "), msg("assistant", "world")];
        let result = filter_messages(messages, &default_config());
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_truncate_long_tool_result() {
        let long_output = "x".repeat(2000); // ~500 tokens
        let messages = vec![msg("tool_result", &long_output)];
        let result = filter_messages(messages, &default_config());
        assert_eq!(result.len(), 1);
        assert!(result[0].content.contains("truncated"));
        assert!(result[0].content.len() < long_output.len());
    }

    #[test]
    fn test_dedup_assistant() {
        let messages = vec![
            msg("assistant", "I'll fix the bug"),
            msg("assistant", "I'll fix the bug"),  // retry duplicate
            msg("assistant", "Actually, different approach"),
        ];
        let result = filter_messages(messages, &default_config());
        assert_eq!(result.len(), 2); // deduped to 2
    }

    #[test]
    fn test_collapse_edits() {
        let messages = vec![
            tool_msg("tool_use", "Edit", r#"{"file_path": "src/main.rs", "old": "a", "new": "b"}"#),
            tool_msg("tool_use", "Edit", r#"{"file_path": "src/main.rs", "old": "c", "new": "d"}"#),
            tool_msg("tool_use", "Edit", r#"{"file_path": "src/main.rs", "old": "e", "new": "f"}"#),
        ];
        let result = filter_messages(messages, &default_config());
        // Should collapse to 1 entry showing latest
        assert_eq!(result.len(), 1);
        assert!(result[0].content.contains("3 edits"));
    }
}
