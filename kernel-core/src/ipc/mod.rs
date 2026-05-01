//! Inter-Process Communication
//!
//! Provides pipes for unidirectional byte streams between tasks.

pub mod pipe;

pub use pipe::{create_pipe, pipe_read, pipe_write, close_read_end, close_write_end, PipeId};
