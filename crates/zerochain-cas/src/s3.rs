use std::any::Any;

use async_trait::async_trait;
use s3::creds::Credentials;
use s3::{Bucket, Region};
use tokio::io::AsyncRead;

use crate::backend::StorageBackend;
use crate::cid::Cid;
use crate::error::{CasError, Result};

/// S3-compatible content-addressed storage backend.
///
/// Stores objects in a configured S3 bucket using the CID hex as the key.
/// Supports MinIO, AWS S3, and any other S3-compatible store.
#[derive(Clone, Debug)]
pub struct S3Backend {
    bucket: Bucket,
}

impl S3Backend {
    /// Create a new S3 backend.
    ///
    /// `endpoint` can be used for MinIO or other S3-compatible services.
    /// If `endpoint` is `None`, standard AWS endpoints are used based on `region`.
    pub fn new(
        bucket_name: &str,
        region: &str,
        endpoint: Option<&str>,
        access_key: &str,
        secret_key: &str,
    ) -> Result<Self> {
        let credentials = Credentials::new(
            Some(access_key),
            Some(secret_key),
            None,
            None,
            None,
        )
        .map_err(|e| CasError::InvalidCid(format!("s3 credentials: {e}")))?;

        let region = if let Some(url) = endpoint {
            Region::Custom {
                region: region.to_string(),
                endpoint: url.to_string(),
            }
        } else {
            region
                .parse::<Region>()
                .map_err(|e| CasError::InvalidCid(format!("s3 region: {e}")))?
        };

        let bucket = *Bucket::new(bucket_name, region, credentials)
            .map_err(|e| CasError::InvalidCid(format!("s3 bucket: {e}")))?
            .with_path_style();

        Ok(Self { bucket })
    }

    /// Create from environment variables.
    ///
    /// Expected vars:
    /// - `ZEROCHAIN_CAS_S3_BUCKET`
    /// - `ZEROCHAIN_CAS_S3_REGION` (default: us-east-1)
    /// - `ZEROCHAIN_CAS_S3_ENDPOINT` (optional, for MinIO)
    /// - `ZEROCHAIN_CAS_S3_ACCESS_KEY`
    /// - `ZEROCHAIN_CAS_S3_SECRET_KEY`
    pub fn from_env() -> Result<Self> {
        let bucket_name = std::env::var("ZEROCHAIN_CAS_S3_BUCKET")
            .map_err(|_| CasError::InvalidCid("ZEROCHAIN_CAS_S3_BUCKET not set".into()))?;
        let region = std::env::var("ZEROCHAIN_CAS_S3_REGION").unwrap_or_else(|_| "us-east-1".into());
        let endpoint = std::env::var("ZEROCHAIN_CAS_S3_ENDPOINT").ok();
        let access_key = std::env::var("ZEROCHAIN_CAS_S3_ACCESS_KEY")
            .map_err(|_| CasError::InvalidCid("ZEROCHAIN_CAS_S3_ACCESS_KEY not set".into()))?;
        let secret_key = std::env::var("ZEROCHAIN_CAS_S3_SECRET_KEY")
            .map_err(|_| CasError::InvalidCid("ZEROCHAIN_CAS_S3_SECRET_KEY not set".into()))?;

        Self::new(
            &bucket_name,
            &region,
            endpoint.as_deref(),
            &access_key,
            &secret_key,
        )
    }

    fn key_for(cid: &Cid) -> String {
        cid.as_hex()
    }
}

#[async_trait]
impl StorageBackend for S3Backend {
    fn as_any(&self) -> &dyn Any {
        self
    }

    async fn put(&self, data: &[u8]) -> Result<Cid> {
        let cid = Cid::from_bytes(data);
        let key = Self::key_for(&cid);

        // Fast path: already stored
        if self.exists(&cid).await? {
            return Ok(cid);
        }

        let response = self
            .bucket
            .put_object(&key, data)
            .await
            .map_err(|e| CasError::Io(std::io::Error::other(
                format!("s3 put failed: {e}"),
            )))?;

        if response.status_code() != 200 {
            return Err(CasError::Io(std::io::Error::other(
                format!("s3 put failed with status: {}", response.status_code()),
            )));
        }

        tracing::debug!(cid = %cid, key = %key, "stored content in s3");
        Ok(cid)
    }

    async fn get(&self, cid: &Cid) -> Result<Vec<u8>> {
        let key = Self::key_for(cid);
        let response = self
            .bucket
            .get_object(&key)
            .await
            .map_err(|e| CasError::Io(std::io::Error::other(
                format!("s3 get failed: {e}"),
            )))?;

        if response.status_code() == 404 {
            return Err(CasError::NotFound(cid.to_string()));
        }

        if response.status_code() != 200 {
            return Err(CasError::Io(std::io::Error::other(
                format!("s3 get failed with status: {}", response.status_code()),
            )));
        }

        Ok(response.bytes().to_vec())
    }

    async fn get_reader(&self, cid: &Cid) -> Result<Box<dyn AsyncRead + Send + Unpin>> {
        // rust-s3 does not provide a streaming async reader directly.
        // We fetch the full object and wrap it in a Cursor for now.
        // For large objects, presigned URLs or ranged GETs would be better.
        let data = self.get(cid).await?;
        Ok(Box::new(std::io::Cursor::new(data)))
    }

    async fn exists(&self, cid: &Cid) -> Result<bool> {
        let key = Self::key_for(cid);
        match self.bucket.head_object(&key).await {
            Ok((_, code)) => Ok(code == 200),
            Err(e) => {
                tracing::debug!(cid = %cid, key = %key, error = %e, "S3 head_object failed");
                Ok(false)
            }
        }
    }
}
