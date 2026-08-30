fn human_byte_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn utf8_prefix_boundary(value: &str, max_bytes: usize) -> usize {
    if value.len() <= max_bytes {
        return value.len();
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    boundary
}

fn utf8_suffix_boundary(value: &str, max_bytes: usize) -> usize {
    if value.len() <= max_bytes {
        return 0;
    }
    let mut boundary = value.len().saturating_sub(max_bytes);
    while boundary < value.len() && !value.is_char_boundary(boundary) {
        boundary += 1;
    }
    boundary
}

fn bound_agent_text(value: &str, max_bytes: usize, label: &str) -> String {
    let sanitized = sanitize_agent_text(value);
    if sanitized.len() <= max_bytes {
        return sanitized;
    }
    if max_bytes == 0 {
        return String::new();
    }

    let marker = format!(
        "\n\n[truncated {label}: original {} bytes; showing beginning and end]\n\n",
        sanitized.len()
    );
    if marker.len() >= max_bytes {
        let end = utf8_prefix_boundary(&marker, max_bytes);
        return marker[..end].to_string();
    }
    let available = max_bytes - marker.len();
    let head_budget = available.saturating_mul(3) / 4;
    let tail_budget = available - head_budget;
    let head_end = utf8_prefix_boundary(&sanitized, head_budget);
    let tail_start = utf8_suffix_boundary(&sanitized, tail_budget);
    format!(
        "{}{}{}",
        &sanitized[..head_end],
        marker,
        &sanitized[tail_start..]
    )
}
