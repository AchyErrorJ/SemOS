// M27 Phase 2a A1 + R4 B2: temp_dir. Upstream wraps `tempfile::TempDir`
// so the temp directory can survive past Drop when --keep-temps is set.
// SemOS has no tempfile equivalent in v1; the stub creates a struct
// that holds a PathBuf and yields it via AsRef<Path>. Real fix needs
// the tempfile vendor + no_std patch (externals queue).
//
// Host build path is unchanged.

#[cfg(not(target_os = "none"))]
mod imp_std {
    use std::mem::ManuallyDrop;
    use std::path::Path;

    pub use tempfile::TempDir;

    /// This is used to avoid TempDir being dropped on error paths unintentionally.
    #[derive(Debug)]
    pub struct MaybeTempDir {
        dir: ManuallyDrop<TempDir>,
        // Whether the TempDir should be deleted on drop.
        keep: bool,
    }

    impl Drop for MaybeTempDir {
        fn drop(&mut self) {
            // SAFETY: We are in the destructor, and no further access will
            // occur.
            let dir = unsafe { ManuallyDrop::take(&mut self.dir) };
            if self.keep {
                let _ = dir.keep();
            }
        }
    }

    impl AsRef<Path> for MaybeTempDir {
        fn as_ref(&self) -> &Path {
            self.dir.path()
        }
    }

    impl MaybeTempDir {
        pub fn new(dir: TempDir, keep_on_drop: bool) -> MaybeTempDir {
            MaybeTempDir { dir: ManuallyDrop::new(dir), keep: keep_on_drop }
        }
    }
}

#[cfg(target_os = "none")]
mod imp_none {
    use semos_std::path::{Path, PathBuf};

    /// M27 R4 B2: stub MaybeTempDir holding a PathBuf.
    /// `new` takes the SemOS-target stub `TempDir` (defined here, since
    /// upstream's `TempDir` comes from the unported `tempfile` crate)
    /// plus a keep-on-drop flag.
    #[derive(Debug)]
    pub struct MaybeTempDir {
        path: PathBuf,
        #[allow(dead_code)]
        keep: bool,
    }

    impl MaybeTempDir {
        pub fn new(dir: TempDir, keep_on_drop: bool) -> MaybeTempDir {
            MaybeTempDir { path: dir.into_path(), keep: keep_on_drop }
        }
    }

    impl AsRef<Path> for MaybeTempDir {
        fn as_ref(&self) -> &Path {
            self.path.as_path()
        }
    }

    /// Stub TempDir for SemOS. Doesn't create anything; just a path
    /// container. Callers that try to write to it will get an io error
    /// from semos-std's fs surface.
    #[derive(Debug)]
    pub struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        pub fn new() -> Self {
            TempDir { path: PathBuf::from("/tmp") }
        }

        pub fn new_in(path: impl Into<PathBuf>) -> Self {
            TempDir { path: path.into() }
        }

        pub fn into_path(self) -> PathBuf {
            self.path
        }

        pub fn path(&self) -> &Path {
            self.path.as_path()
        }
    }
}

#[cfg(not(target_os = "none"))]
pub use imp_std::*;
#[cfg(target_os = "none")]
pub use imp_none::*;
