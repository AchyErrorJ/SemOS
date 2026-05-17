//! Filesystem abstractions.

pub mod ramfs;
pub mod paths;

pub use ramfs::{Ramfs, RamfsFile, FileType, FdTable};
pub use paths::{Namespace, FsError, ROOT_SUID};
