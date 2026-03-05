use crate::types::{Model, SimpleStreamOptions, StreamOptions, ThinkingLevel};

/// Convert SimpleStreamOptions to base StreamOptions, applying default maxTokens.
pub fn build_base_options(model: &Model, options: &SimpleStreamOptions) -> StreamOptions {
    let mut base = options.base.clone();
    if base.max_tokens.is_none() {
        base.max_tokens = Some(model.max_tokens.min(32000));
    }
    base
}

/// Clamp reasoning level: xhigh -> high unless the model supports it.
pub fn clamp_reasoning(level: ThinkingLevel) -> ThinkingLevel {
    match level {
        ThinkingLevel::Xhigh => ThinkingLevel::High,
        other => other,
    }
}

/// Default thinking budgets per level.
fn default_budget(level: ThinkingLevel) -> u32 {
    match level {
        ThinkingLevel::Minimal => 1024,
        ThinkingLevel::Low => 2048,
        ThinkingLevel::Medium => 8192,
        ThinkingLevel::High | ThinkingLevel::Xhigh => 16384,
    }
}

/// Calculate adjusted maxTokens and thinking budget for budget-based thinking.
/// Returns (max_tokens, thinking_budget).
pub fn adjust_max_tokens_for_thinking(
    model: &Model,
    options: &SimpleStreamOptions,
) -> (u32, u32) {
    let reasoning = options.reasoning.unwrap_or(ThinkingLevel::Medium);
    let budgets = options.thinking_budgets.as_ref();

    let thinking_budget = match reasoning {
        ThinkingLevel::Minimal => budgets.and_then(|b| b.minimal).unwrap_or(default_budget(ThinkingLevel::Minimal)),
        ThinkingLevel::Low => budgets.and_then(|b| b.low).unwrap_or(default_budget(ThinkingLevel::Low)),
        ThinkingLevel::Medium => budgets.and_then(|b| b.medium).unwrap_or(default_budget(ThinkingLevel::Medium)),
        ThinkingLevel::High | ThinkingLevel::Xhigh => {
            budgets.and_then(|b| b.high).unwrap_or(default_budget(ThinkingLevel::High))
        }
    };

    let base_max = options
        .base
        .max_tokens
        .unwrap_or_else(|| model.max_tokens.min(32000));

    // Ensure at least 1024 tokens remain for output after thinking budget
    let max_tokens = (base_max + thinking_budget).max(thinking_budget + 1024);

    (max_tokens, thinking_budget)
}
