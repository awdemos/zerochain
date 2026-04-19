use std::sync::Arc;

use tokio::io::AsyncRead;

use crate::backend::StorageBackend;
use crate::cid::Cid;
use crate::error::Result;

/// Content-addressed store backed by a pluggable [`StorageBackend`].
///
/// By default uses a local filesystem backend with a two-level layout:
/// `{base_dir}/ab/abcdef...` where `ab` is the first two hex characters
/// of the Blake3 hash.  Writes are atomic (temp file + rename) to prevent
/// partial reads.
///
/// The backend can be swapped for S3/MinIO via [`CasStore::with_backend`].
#[derive(Clone)]
pub struct CasStore {
    backend: Arc<dyn StorageBackend>,
}

impl std::fmt::Debug for CasStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CasStore")
            .field("backend", &"<dyn StorageBackend>")
            .finish()
    }
}

impl CasStore {
    /// Create a new CAS store with the local filesystem backend.
    pub async fn new(base_dir: std::path::PathBuf) -> Result<Self> {
        let backend = crate::backend::LocalBackend::new(base_dir).await?;
        Ok(Self {
            backend: Arc::new(backend),
        })
    }

    /// Create a CAS store with an explicit backend.
    pub fn with_backend<B: StorageBackend + 'static>(backend: B) -> Self {
        Self {
            backend: Arc::new(backend),
        }
    }

    /// Store bytes and return their content identifier.
    pub async fn put(&self, data: &[u8]) -> Result<Cid> {
        self.backend.put(data).await
    }

    /// Retrieve content by its CID.
    pub async fn get(&self, cid: &Cid) -> Result<Vec<u8>> {
        self.backend.get(cid).await
    }

    /// Return a streaming reader for the given CID.
    pub async fn get_reader(&self, cid: &Cid) -> Result<Box<dyn AsyncRead + Send + Unpin>> {
        self.backend.get_reader(cid).await
    }

    /// Check whether content exists in the store.
    pub async fn exists(&self, cid: &Cid) -> Result<bool> {
        self.backend.exists(cid).await
    }

    /// List all content identifiers currently stored.
    pub async fn list(&self) -> Result<Vec<Cid>> {
        self.backend.list().await
    }

    /// Remove content by its CID.
    pub async fn delete(&self, cid: &Cid) -> Result<()> {
        self.backend.delete(cid).await
    }

    /// Return the backend location (filesystem path, bucket name, etc.).
    pub fn location(&self) -> String {
        self.backend.location()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn make_store() -> (TempDir, CasStore) {
        let dir = TempDir::new().unwrap();
        let store = CasStore::new(dir.path().to_path_buf()).await.unwrap();
        (dir, store)
    }

    #[tokio::test]
    async fn new_creates_directory() {
        let dir = TempDir::new().unwrap();
        let store_path = dir.path().join("cas_store");
        assert!(!store_path.exists());
        CasStore::new(store_path.clone()).await.unwrap();
        assert!(store_path.exists());
    }

    #[tokio::test]
    async fn put_and_get() {
        let (_dir, store) = make_store().await;
        let data = b"hello, world!";
        let cid = store.put(data).await.unwrap();
        let retrieved = store.get(&cid).await.unwrap();
        assert_eq!(retrieved, data);
    }

    #[tokio::test]
    async fn put_is_idempotent() {
        let (_dir, store) = make_store().await;
        let data = b"idempotent test";
        let cid1 = store.put(data).await.unwrap();
        let cid2 = store.put(data).await.unwrap();
        assert_eq!(cid1, cid2);
    }

    #[tokio::test]
    async fn put_deduplicates() {
        let (_dir, store) = make_store().await;
        let data = b"dedup content";
        let cid = store.put(data).await.unwrap();
        let cid2 = store.put(data).await.unwrap();
        assert_eq!(cid, cid2);
        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 1);
    }

    #[tokio::test]
    async fn get_not_found() {
        let (_dir, store) = make_store().await;
        let cid = Cid::from_bytes(b"nonexistent");
        let result = store.get(&cid).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            crate::error::CasError::NotFound(s) => assert_eq!(s, cid.to_string()),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn exists_true_and_false() {
        let (_dir, store) = make_store().await;
        let cid = store.put(b"exists test").await.unwrap();
        assert!(store.exists(&cid).await.unwrap());
        let fake_cid = Cid::from_bytes(b"nope");
        assert!(!store.exists(&fake_cid).await.unwrap());
    }

    #[tokio::test]
    async fn list_empty() {
        let (_dir, store) = make_store().await;
        let list = store.list().await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn list_returns_all() {
        let (_dir, store) = make_store().await;
        let cid1 = store.put(b"aaa").await.unwrap();
        let cid2 = store.put(b"bbb").await.unwrap();
        let cid3 = store.put(b"ccc").await.unwrap();
        let mut list = store.list().await.unwrap();
        list.sort_by_key(|a| a.as_hex());
        let mut expected = vec![cid1, cid2, cid3];
        expected.sort_by_key(|a| a.as_hex());
        assert_eq!(list, expected);
    }

    #[tokio::test]
    async fn delete_removes_content() {
        let (_dir, store) = make_store().await;
        let cid = store.put(b"to be deleted").await.unwrap();
        assert!(store.exists(&cid).await.unwrap());
        store.delete(&cid).await.unwrap();
        assert!(!store.exists(&cid).await.unwrap());
    }

    #[tokio::test]
    async fn delete_not_found() {
        let (_dir, store) = make_store().await;
        let cid = Cid::from_bytes(b"ghost");
        let result = store.delete(&cid).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            crate::error::CasError::NotFound(s) => assert_eq!(s, cid.to_string()),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_reader_streams_content() {
        let (_dir, store) = make_store().await;
        let data = b"streaming content test";
        let cid = store.put(data).await.unwrap();
        let mut reader = store.get_reader(&cid).await.unwrap();
        let mut read_data = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut reader, &mut read_data)
            .await
            .unwrap();
        assert_eq!(read_data, data);
    }

    #[tokio::test]
    async fn get_reader_not_found() {
        let (_dir, store) = make_store().await;
        let cid = Cid::from_bytes(b"no reader");
        let result = store.get_reader(&cid).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn file_layout_uses_prefix_directories() {
        let (_dir, store) = make_store().await;
        let cid = store.put(b"layout check").await.unwrap();
        let hex = cid.as_hex();
        let base = std::path::PathBuf::from(store.location());
        let expected_dir = base.join(&hex[..2]);
        let expected_file = expected_dir.join(&hex);
        assert!(expected_dir.is_dir());
        assert!(expected_file.is_file());
    }

    #[tokio::test]
    async fn location_returns_base_dir() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        let store = CasStore::new(path.clone()).await.unwrap();
        assert_eq!(store.location(), path.display().to_string());
    }

    #[tokio::test]
    async fn put_empty_data() {
        let (_dir, store) = make_store().await;
        let cid = store.put(b"").await.unwrap();
        let retrieved = store.get(&cid).await.unwrap();
        assert!(retrieved.is_empty());
    }

    #[tokio::test]
    async fn put_large_data() {
        let (_dir, store) = make_store().await;
        let data = vec![0xABu8; 1024 * 1024];
        let cid = store.put(&data).await.unwrap();
        let retrieved = store.get(&cid).await.unwrap();
        assert_eq!(retrieved.len(), data.len());
        assert_eq!(retrieved, data);
    }
}
