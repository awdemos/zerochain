use std::path::{Path, PathBuf};

use tokio::fs;
use tokio::io::AsyncRead;
use tracing;

use crate::cid::Cid;
use crate::error::{CasError, Result};

/// Content-addressed store backed by a local filesystem directory.
///
/// Files are stored using a two-level layout: `{base_dir}/ab/abcdef...`
/// where `ab` is the first two hex characters of the Blake3 hash.
/// Writes are atomic (temp file + rename) to prevent partial reads.
#[derive(Clone, Debug)]
pub struct CasStore {
    base_dir: PathBuf,
}

impl CasStore {
    /// Create a new CAS store rooted at `base_dir`.
    ///
    /// Creates the directory (and the standard two-letter prefix subdirectories
    /// on demand) if it does not already exist.
    pub async fn new(base_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&base_dir)
            .await
            .map_err(|e| CasError::StoreDirectory {
                path: base_dir.clone(),
                source: e,
            })?;
        Ok(Self { base_dir })
    }

    /// Store bytes and return their content identifier.
    ///
    /// If the content already exists this is a no-op (deduplication).
    pub async fn put(&self, data: &[u8]) -> Result<Cid> {
        let cid = Cid::from_bytes(data);
        let path = self.path_for(&cid);

        // Fast path: already stored
        if path.exists() {
            return Ok(cid);
        }

        // Atomic write: temp file in same directory, then rename
        let parent = path
            .parent()
            .expect("CID path always has a parent directory");
        fs::create_dir_all(parent).await?;

        let temp_path = path.with_extension("tmp");
        fs::write(&temp_path, data).await?;
        fs::rename(&temp_path, &path).await?;

        tracing::debug!(cid = %cid, "stored content");
        Ok(cid)
    }

    /// Retrieve content by its CID.
    pub async fn get(&self, cid: &Cid) -> Result<Vec<u8>> {
        let path = self.path_for(cid);
        match fs::read(&path).await {
            Ok(data) => Ok(data),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(CasError::NotFound(cid.to_string()))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Return a streaming reader for the given CID.
    pub async fn get_reader(&self, cid: &Cid) -> Result<impl AsyncRead> {
        let path = self.path_for(cid);
        if !path.exists() {
            return Err(CasError::NotFound(cid.to_string()));
        }
        let file = fs::File::open(&path).await?;
        Ok(tokio::io::BufReader::new(file))
    }

    /// Check whether content exists in the store.
    pub async fn exists(&self, cid: &Cid) -> bool {
        self.path_for(cid).exists()
    }

    /// List all content identifiers currently stored.
    pub async fn list(&self) -> Result<Vec<Cid>> {
        let mut cids = Vec::new();
        let mut entries = match fs::read_dir(&self.base_dir).await {
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

            // Only look at two-character hex prefix directories
            if dir_name.len() != 2 {
                continue;
            }

            let mut sub_entries = fs::read_dir(entry.path()).await?;
            while let Some(sub_entry) = sub_entries.next_entry().await? {
                let meta = sub_entry.metadata().await?;
                if !meta.is_file() {
                    continue;
                }
                let file_name = sub_entry.file_name();
                let file_name = file_name.to_string_lossy();
                // Skip temp files
                if file_name.ends_with(".tmp") {
                    continue;
                }
                // The full CID is the file_name (64 hex chars)
                if let Ok(cid) = file_name.parse::<Cid>() {
                    cids.push(cid);
                }
            }
        }

        cids.sort_by(|a, b| a.as_hex().cmp(&b.as_hex()));
        Ok(cids)
    }

    /// Remove content by its CID.
    pub async fn delete(&self, cid: &Cid) -> Result<()> {
        let path = self.path_for(cid);
        match fs::remove_file(&path).await {
            Ok(()) => {
                tracing::debug!(cid = %cid, "deleted content");
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(CasError::NotFound(cid.to_string()))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Return the base directory of this store.
    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Full filesystem path for a given CID.
    fn path_for(&self, cid: &Cid) -> PathBuf {
        self.base_dir.join(cid.relative_path())
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
        let expected_dir = store.base_dir.join(&hex[..2]);
        let expected_file = expected_dir.join(&hex);
        assert!(expected_dir.is_dir());
        assert!(expected_file.is_file());
    }

    #[tokio::test]
    async fn base_dir_accessor() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        let store = CasStore::new(path.clone()).await.unwrap();
        assert_eq!(store.base_dir(), path);
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
