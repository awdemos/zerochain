use std::any::Any;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::fs;
use tokio::io::AsyncRead;

use crate::cid::Cid;
use crate::error::{CasError, Result};

/// Abstraction over content-addressed storage backends.
#[async_trait]
pub trait StorageBackend: Send + Sync {
    fn as_any(&self) -> &dyn Any;

    /// Store bytes and return their content identifier.
    async fn put(&self, data: &[u8]) -> Result<Cid>;

    /// Retrieve content by its CID.
    async fn get(&self, cid: &Cid) -> Result<Vec<u8>>;

    /// Return a streaming reader for the given CID.
    async fn get_reader(&self, cid: &Cid) -> Result<Box<dyn AsyncRead + Send + Unpin>>;

    /// Check whether content exists in the store.
    async fn exists(&self, cid: &Cid) -> Result<bool>;

    /// List all stored content identifiers.
    async fn list(&self) -> Result<Vec<Cid>>;

    /// Remove content by its CID.
    async fn delete(&self, cid: &Cid) -> Result<()>;

    /// Return a human-readable location for this backend (e.g. directory path or bucket name).
    fn location(&self) -> String;
}

/// Filesystem-backed content-addressed storage.
///
/// Files are stored using a two-level layout: `{base_dir}/ab/abcdef...`
/// where `ab` is the first two hex characters of the Blake3 hash.
/// Writes are atomic (temp file + rename) to prevent partial reads.
#[derive(Clone, Debug)]
pub struct LocalBackend {
    base_dir: PathBuf,
}

impl LocalBackend {
    /// Create a new local backend rooted at `base_dir`.
    pub async fn new(base_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&base_dir)
            .await
            .map_err(|e| CasError::StoreDirectory {
                path: base_dir.clone(),
                source: e,
            })?;
        Ok(Self { base_dir })
    }

    /// Full filesystem path for a given CID.
    #[must_use] pub fn path_for(&self, cid: &Cid) -> PathBuf {
        self.base_dir.join(cid.relative_path())
    }

    /// Return the base directory.
    #[must_use] pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }
}

#[async_trait]
impl StorageBackend for LocalBackend {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn put(&self, data: &[u8]) -> Result<Cid> {
        let cid = Cid::from_bytes(data);
        let path = self.path_for(&cid);

        // Fast path: already stored
        if path.exists() {
            return Ok(cid);
        }

        // Atomic write: temp file in same directory, then rename
        let parent = path
            .parent()
            .ok_or_else(|| CasError::InvalidCid("CID path has no parent directory".into()))?;
        fs::create_dir_all(parent).await
            .map_err(|e| CasError::io(parent, e))?;

        let temp_path = path.with_extension("tmp");
        fs::write(&temp_path, data).await
            .map_err(|e| CasError::io(&temp_path, e))?;
        fs::rename(&temp_path, &path).await
            .map_err(|e| CasError::io(&path, e))?;

        tracing::debug!(cid = %cid, "stored content");
        Ok(cid)
    }

    async fn get(&self, cid: &Cid) -> Result<Vec<u8>> {
        let path = self.path_for(cid);
        match fs::read(&path).await {
            Ok(data) => Ok(data),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(CasError::NotFound(cid.to_string()))
            }
            Err(e) => Err(CasError::io(&path, e)),
        }
    }

    async fn get_reader(&self, cid: &Cid) -> Result<Box<dyn AsyncRead + Send + Unpin>> {
        let path = self.path_for(cid);
        if !path.exists() {
            return Err(CasError::NotFound(cid.to_string()));
        }
        let file = fs::File::open(&path).await
            .map_err(|e| CasError::io(&path, e))?;
        Ok(Box::new(tokio::io::BufReader::new(file)))
    }

    async fn exists(&self, cid: &Cid) -> Result<bool> {
        Ok(self.path_for(cid).exists())
    }

    async fn list(&self) -> Result<Vec<Cid>> {
        let mut cids = Vec::new();
        let mut entries = match fs::read_dir(&self.base_dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(cids),
            Err(e) => return Err(CasError::io(&self.base_dir, e)),
        };

        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(e)) => e,
                Ok(None) => break,
                Err(e) => return Err(CasError::io(&self.base_dir, e)),
            };
            let metadata = entry.metadata().await
                .map_err(|e| CasError::io(entry.path(), e))?;
            if !metadata.is_dir() {
                continue;
            }
            let dir_name = entry.file_name();
            let dir_name = dir_name.to_string_lossy();

            if dir_name.len() != 2 {
                continue;
            }

            let subdir = entry.path();
            let mut sub_entries = fs::read_dir(&subdir).await
                .map_err(|e| CasError::io(&subdir, e))?;
            loop {
                let sub_entry = match sub_entries.next_entry().await {
                    Ok(Some(e)) => e,
                    Ok(None) => break,
                    Err(e) => return Err(CasError::io(&subdir, e)),
                };
                let meta = sub_entry.metadata().await
                    .map_err(|e| CasError::io(sub_entry.path(), e))?;
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

        cids.sort_by_key(super::cid::Cid::as_hex);
        Ok(cids)
    }

    async fn delete(&self, cid: &Cid) -> Result<()> {
        let path = self.path_for(cid);
        match fs::remove_file(&path).await {
            Ok(()) => {
                tracing::debug!(cid = %cid, "deleted content");
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(CasError::NotFound(cid.to_string()))
            }
            Err(e) => Err(CasError::io(&path, e)),
        }
    }

    fn location(&self) -> String {
        self.base_dir.display().to_string()
    }
}
