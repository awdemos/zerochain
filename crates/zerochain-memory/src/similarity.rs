/// Compute cosine similarity between two equal-length f32 vectors.
/// Returns `None` if either vector is empty or has different length.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;
    for i in 0..a.len() {
        let ai = a[i] as f64;
        let bi = b[i] as f64;
        dot += ai * bi;
        norm_a += ai * ai;
        norm_b += bi * bi;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        return Some(0.0);
    }
    Some((dot / denom) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_vectors_have_similarity_one() {
        let v = vec![1.0f32, 2.0, 3.0];
        assert!((cosine_similarity(&v, &v).unwrap() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn orthogonal_vectors_have_similarity_zero() {
        let a = vec![1.0f32, 0.0];
        let b = vec![0.0f32, 1.0];
        assert!((cosine_similarity(&a, &b).unwrap()).abs() < 1e-6);
    }

    #[test]
    fn mismatched_lengths_return_none() {
        assert!(cosine_similarity(&[1.0], &[1.0, 2.0]).is_none());
    }
}
