//! Filesystem abstractions.

pub mod ramfs;

pub use ramfs::{Ramfs, RamfsFile, FileType, FdTable};
