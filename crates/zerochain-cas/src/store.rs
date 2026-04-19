use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::AsyncRead;

use crate::backend::{LocalBackend, StorageBackend};
use crate::cid::Cid;
use crate::error::{CasError, Result};

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
    pub async fn new(base_dir: PathBuf) -> Result<Self> {
        let backend = LocalBackend::new(base_dir).await?;
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
    pub async fn exists(&self, cid: &Cid) -> bool {
        self.backend.exists(cid).await
    }

    /// List all content identifiers currently stored.
    ///
    /// **Note:** This operation is only supported by the local filesystem
    /// backend. Calling it with an S3 backend will return an empty list.
    pub async fn list(&self) -> Result<Vec<Cid>> {
        let local = self
            .backend
            .as_any()
            .downcast_ref::<LocalBackend>();

        if let Some(local) = local {
            list_local(local).await
        } else {
            Ok(Vec::new())
        }
    }

    /// Remove content by its CID.
    pub async fn delete(&self, cid: &Cid) -> Result<()> {
        if let Some(local) = self.backend.as_any().downcast_ref::<LocalBackend>() {
            let path = local.path_for(cid);
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {
                    tracing::debug!(cid = %cid, "deleted content");
                    Ok(())
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    Err(CasError::NotFound(cid.to_string()))
                }
                Err(e) => Err(e.into()),
            }
        } else {
            tracing::warn!(cid = %cid, "delete not implemented for non-local backend");
            Ok(())
        }
    }

    /// Return the base directory of this store, if backed by local filesystem.
    pub fn base_dir(&self) -> Option<&Path> {
        self.backend
            .as_any()
            .downcast_ref::<LocalBackend>()
            .map(|l| l.base_dir())
    }
}

async fn list_local(local: &LocalBackend) -> Result<Vec<Cid>> {
    let base_dir = local.base_dir();
    let mut cids = Vec::new();
    let mut entries = match tokio::fs::read_dir(base_dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(cids),
        Err(e) => return Err(e.into()),
    };

    while let Some(entry) = entries.next_entry().await? {
        let metadata = entry.metadata().await?;
        if !metadata.is_dir() {
            continue;
        }
        let dir_name = entry.file_name();
        let dir_name = dir_name.to_string_lossy();

        if dir_name.len() != 2 {
            continue;
        }

        let mut sub_entries = tokio::fs::read_dir(entry.path()).await?;
        while let Some(sub_entry) = sub_entries.next_entry().await? {
            let meta = sub_entry.metadata().await?;
            if !meta.is_file() {
                continue;
            }
            let file_name = sub_entry.file_name();
            let file_name = file_name.to_string_lossy();
            if file_name.ends_with(".tmp") {
                continue;
            }
            if let Ok(cid) = file_name.parse::<Cid>() {
                cids.push(cid);
            }
        }
    }

    cids.sort_by_key(|a| a.as_hex());
    Ok(cids)
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
            CasError::NotFound(s) => assert_eq!(s, cid.to_string()),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn exists_true_and_false() {
        let (_dir, store) = make_store().await;
        let cid = store.put(b"exists test").await.unwrap();
        assert!(store.exists(&cid).await);
        let fake_cid = Cid::from_bytes(b"nope");
        assert!(!store.exists(&fake_cid).await);
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
        list.sort_by(|a, b| a.as_hex().cmp(&b.as_hex()));
        let mut expected = vec![cid1, cid2, cid3];
        expected.sort_by(|a, b| a.as_hex().cmp(&b.as_hex()));
        assert_eq!(list, expected);
    }

    #[tokio::test]
    async fn delete_removes_content() {
        let (_dir, store) = make_store().await;
        let cid = store.put(b"to be deleted").await.unwrap();
        assert!(store.exists(&cid).await);
        store.delete(&cid).await.unwrap();
        assert!(!store.exists(&cid).await);
    }

    #[tokio::test]
    async fn delete_not_found() {
        let (_dir, store) = make_store().await;
        let cid = Cid::from_bytes(b"ghost");
        let result = store.delete(&cid).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            CasError::NotFound(s) => assert_eq!(s, cid.to_string()),
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
        let expected_dir = store.base_dir().unwrap().join(&hex[..2]);
        let expected_file = expected_dir.join(&hex);
        assert!(expected_dir.is_dir());
        assert!(expected_file.is_file());
    }

    #[tokio::test]
    async fn base_dir_accessor() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        let store = CasStore::new(path.clone()).await.unwrap();
        assert_eq!(store.base_dir(), Some(path.as_path()));
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
