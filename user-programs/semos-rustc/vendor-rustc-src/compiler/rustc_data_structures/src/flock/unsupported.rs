// M27 Phase 2a A1 + R4 B2: flock unsupported backend.
// On SemOS (target_os = "none") this is the active backend — file
// locking is not implemented in semos-std, so we return an
// Unsupported error from `new`. §1.3 drops incremental compilation
// anyway, so this path should not be reached in v1.

#[cfg(not(target_os = "none"))]
mod imp_std {
    use std::io;
    use std::path::Path;

    #[derive(Debug)]
    pub struct Lock(());

    impl Lock {
        pub fn new(_p: &Path, _wait: bool, _create: bool, _exclusive: bool) -> io::Result<Lock> {
            let msg = "file locks not supported on this platform";
            Err(io::Error::new(io::ErrorKind::Other, msg))
        }

        pub fn error_unsupported(_err: &io::Error) -> bool {
            true
        }
    }
}

#[cfg(target_os = "none")]
mod imp_none {
    use semos_std::io;
    use semos_std::path::Path;

    #[derive(Debug)]
    pub struct Lock(());

    impl Lock {
        pub fn new(_p: &Path, _wait: bool, _create: bool, _exclusive: bool) -> io::Result<Lock> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "flock not supported on SemOS"))
        }

        pub fn error_unsupported(_err: &io::Error) -> bool {
            true
        }
    }
}

#[cfg(not(target_os = "none"))]
pub use imp_std::*;
#[cfg(target_os = "none")]
pub use imp_none::*;
