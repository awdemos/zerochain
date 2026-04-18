mod backend;
mod cid;
mod error;
mod store;

#[cfg(feature = "s3")]
mod s3;

pub use backend::{LocalBackend, StorageBackend};
pub use cid::Cid;
pub use error::{CasError, Result};
pub use store::CasStore;

#[cfg(feature = "s3")]
pub use s3::S3Backend;
