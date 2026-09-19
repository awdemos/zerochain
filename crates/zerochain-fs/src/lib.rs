//! Copy-on-write filesystem abstraction (Btrfs, APFS, directory fallback).

pub mod anchored;
pub mod atomic;
pub mod cow;
pub mod error;

pub use anchored::{read_anchored, write_anchored};
pub use atomic::{
    acquire_lock, clean_output, clear_executing, is_complete, is_error, is_executing, is_locked,
    mark_complete, mark_error, mark_executing, write_atomic, LockGuard,
};
pub use cow::{detect_backend, BtrfsCow, CowPlatform, DirectoryCow, NoopCow, SubvolumeMode};
pub use error::{FsError, Result};
