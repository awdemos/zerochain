#![allow(clippy::missing_errors_doc)]
//! Copy-on-write filesystem abstraction (Btrfs, APFS, directory fallback).

pub mod atomic;
pub mod cow;
pub mod error;

pub use atomic::{
    acquire_lock, clean_output, clear_executing, is_complete, is_error, is_executing, is_locked,
    mark_complete, mark_error, mark_executing, write_atomic, LockGuard,
};
pub use cow::{detect_backend, BtrfsCow, CowPlatform, DirectoryCow, NoopCow};
pub use error::{FsError, Result};
