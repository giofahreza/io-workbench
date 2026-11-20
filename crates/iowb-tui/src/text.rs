use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Wraps `text` to `width` display columns, honouring newlines and preferring to
/// break at whitespace. Always returns at least one (possibly empty) line.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.replace('\t', "    ");
        let raw = raw.trim_end_matches('\r');
        let mut current = String::new();
        let mut used = 0;
        for ch in raw.chars() {
            let w = ch.width().unwrap_or(0);
            if used + w > width && used > 0 {
                match current.rfind(' ') {
                    Some(space) if space > 0 && ch != ' ' => {
                        let rest = current[space + 1..].to_string();
                        current.truncate(space);
                        lines.push(std::mem::take(&mut current));
                        used = rest.width();
                        current = rest;
                    }
                    _ => {
                        lines.push(std::mem::take(&mut current));
                        used = 0;
                    }
                }
                if ch == ' ' && used == 0 {
                    continue;
                }
            }
            current.push(ch);
            used += w;
        }
        lines.push(current);
    }
    lines
}

/// Shortens `text` to at most `width` columns, ending with an ellipsis when cut.
pub fn truncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_at_whitespace_and_keeps_newlines() {
        assert_eq!(
            wrap("hello brave new world", 11),
            ["hello brave", "new world"]
        );
        assert_eq!(wrap("a\n\nb", 10), ["a", "", "b"]);
        assert_eq!(wrap("", 10), [""]);
    }

    #[test]
    fn hard_breaks_words_longer_than_the_width() {
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn counts_wide_characters_as_two_columns() {
        assert_eq!(wrap("日本語日本語", 6), ["日本語", "日本語"]);
    }

    #[test]
    fn truncates_with_an_ellipsis() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
    }
}
