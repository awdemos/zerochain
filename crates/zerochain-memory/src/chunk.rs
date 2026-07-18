/// Split `text` into chunks of at most `chunk_size` characters, overlapping
/// by `overlap` characters between consecutive chunks.
///
/// `overlap` is clamped to `chunk_size.saturating_sub(1)` so that the step
/// between chunks is always at least one character.
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    if chunk_size == 0 {
        return Vec::new();
    }
    let overlap = overlap.min(chunk_size.saturating_sub(1));
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }
    let step = chunk_size.saturating_sub(overlap).max(1);
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + chunk_size).min(chars.len());
        chunks.push(chars[start..end].iter().collect());
        if end == chars.len() {
            break;
        }
        start = (start + step).min(chars.len());
        if start >= chars.len() {
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

    #[test]
    fn multi_byte_text_never_splits_characters() {
        let text = "こんにちは世界";
        let chunks = chunk_text(text, 3, 1);
        assert_eq!(chunks, vec!["こんに", "にちは", "は世界"]);
        for chunk in &chunks {
            assert_eq!(chunk.chars().count(), 3);
        }
    }

    #[test]
    fn overlap_greater_than_or_equal_to_chunk_size_is_clamped() {
        assert_eq!(chunk_text("hello", 3, 5), vec!["hel", "ell", "llo"]);
    }
}
