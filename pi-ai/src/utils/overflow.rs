use once_cell::sync::Lazy;
use regex::Regex;

use crate::types::AssistantMessage;

static OVERFLOW_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    [
        r"(?i)prompt is too long",
        r"(?i)input is too long for requested model",
        r"(?i)exceeds the context window",
        r"(?i)input token count.*exceeds the maximum",
        r"(?i)maximum prompt length is \d+",
        r"(?i)reduce the length of the messages",
        r"(?i)maximum context length is \d+ tokens",
        r"(?i)exceeds the limit of \d+",
        r"(?i)exceeds the available context size",
        r"(?i)greater than the context length",
        r"(?i)context window exceeds limit",
        r"(?i)exceeded model token limit",
        r"(?i)context[_ ]length[_ ]exceeded",
        r"(?i)too many tokens",
        r"(?i)token limit exceeded",
    ]
    .iter()
    .map(|p| Regex::new(p).unwrap())
    .collect()
});

static EMPTY_BODY_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^4(00|13)\s*(status code)?\s*\(no body\)").unwrap());

pub fn is_context_overflow(message: &AssistantMessage, context_window: Option<u64>) -> bool {
    if message.stop_reason == crate::types::StopReason::Error {
        if let Some(ref err_msg) = message.error_message {
            if OVERFLOW_PATTERNS.iter().any(|p| p.is_match(err_msg)) {
                return true;
            }
            if EMPTY_BODY_PATTERN.is_match(err_msg) {
                return true;
            }
        }
    }

    if let Some(cw) = context_window {
        if message.stop_reason == crate::types::StopReason::Stop {
            let input_tokens = message.usage.input + message.usage.cache_read;
            if input_tokens > cw {
                return true;
            }
        }
    }

    false
}
