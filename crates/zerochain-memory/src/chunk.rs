/// Split `text` into chunks of at most `chunk_size` characters, overlapping
/// by `overlap` characters between consecutive chunks.
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if chunk_size == 0 {
        return Vec::new();
    }
    let step = chunk_size.saturating_sub(overlap).max(1);
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let end = (start + chunk_size).min(text.len());
        chunks.push(text[start..end].to_string());
        if end == text.len() {
            break;
        }
        start = (start + step).min(text.len());
        if start >= text.len() {
            break;
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_returns_empty() {
        assert!(chunk_text("", 100, 20).is_empty());
    }

    #[test]
    fn short_text_is_one_chunk() {
        assert_eq!(chunk_text("hello", 100, 20), vec!["hello"]);
    }

    #[test]
    fn overlap_produces_expected_chunks() {
        let text = "abcdefghijklmnopqrstuvwxyz";
        let chunks = chunk_text(text, 10, 2);
        assert_eq!(chunks, vec!["abcdefghij", "ijklmnopqr", "qrstuvwxyz"]);
    }
}
