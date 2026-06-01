//! `scoped-tls` — M27 Phase 5b Stage E iter 11 stub fork.
//!
//! Forwards to semos_std::thread::ScopedKey + scoped_thread_local!.

#![no_std]

pub use semos_std::thread::ScopedKey;

#[macro_export]
macro_rules! scoped_thread_local {
    ($(#[$attrs:meta])* static $name:ident: $ty:ty) => {
        $(#[$attrs])*
        static $name: $crate::ScopedKey<$ty> = $crate::ScopedKey::new();
    };
    ($(#[$attrs:meta])* pub static $name:ident: $ty:ty) => {
        $(#[$attrs])*
        pub static $name: $crate::ScopedKey<$ty> = $crate::ScopedKey::new();
    };
}
