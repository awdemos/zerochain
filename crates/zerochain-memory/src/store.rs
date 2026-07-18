//! Filesystem-native vector memory store.

/// Persistent memory store backed by the filesystem.
pub struct MemoryStore;

impl MemoryStore {
    /// Create a new memory store at the given path.
    pub fn new(_path: std::path::PathBuf) -> Self {
        Self
    }
}
