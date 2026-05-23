//! Inter-Process Communication
//!
//! Provides pipes for unidirectional byte streams between tasks.

pub mod pipe;

pub use pipe::{
    close_read_end, close_write_end, create_pipe, dup_read_end, dup_write_end, pipe_read,
    pipe_write, PipeId,
};
