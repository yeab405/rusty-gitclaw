pub const MAX_OUTPUT: usize = 100_000;
pub const MAX_LINES: usize = 2000;
pub const MAX_BYTES: usize = 100_000;
pub const DEFAULT_TIMEOUT: u64 = 120;
pub const DEFAULT_MEMORY_PATH: &str = "memory/MEMORY.md";

pub fn truncate_output(text: &str) -> String {
    if text.len() > MAX_OUTPUT {
        format!(
            "[output truncated, showing last ~100KB]\n{}",
            &text[text.len() - MAX_OUTPUT..]
        )
    } else {
        text.to_string()
    }
}

pub struct PaginateResult {
    pub text: String,
    pub has_more: bool,
    pub shown_range: (usize, usize),
    pub total_lines: usize,
}

pub fn paginate_lines(text: &str, offset: Option<usize>, limit: Option<usize>) -> Result<PaginateResult, String> {
    let all_lines: Vec<&str> = text.split('\n').collect();
    let total_lines = all_lines.len();

    let start_line = offset.map(|o| o.saturating_sub(1)).unwrap_or(0);
    if start_line >= total_lines {
        return Err(format!("Offset {} is beyond end of file ({total_lines} lines)", start_line + 1));
    }

    let max_lines = limit.unwrap_or(MAX_LINES);
    let end_line = (start_line + max_lines).min(total_lines);
    let mut selected: String = all_lines[start_line..end_line].join("\n");

    let mut truncated_by_bytes = false;
    if selected.len() > MAX_BYTES {
        selected.truncate(MAX_BYTES);
        truncated_by_bytes = true;
    }

    let has_more = end_line < total_lines || truncated_by_bytes;

    Ok(PaginateResult {
        text: selected,
        has_more,
        shown_range: (start_line + 1, end_line),
        total_lines,
    })
}
