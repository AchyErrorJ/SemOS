// M27 Phase 2a A1 + R4 B2: memmap. On host targets we use
// memmap2 or a Vec<u8> fallback (miri/wasm32). On SemOS we fall
// through to the Vec<u8> fallback path — there is no FS-backed
// mapping primitive in semos-std yet, but reading the file into a
// Vec<u8> works since rmeta files in v1 are bounded by our heap.

#[cfg(not(target_os = "none"))]
mod imp_std {
    use std::fs::File;
    use std::io;
    use std::ops::{Deref, DerefMut};

    /// A trivial wrapper for [`memmap2::Mmap`] (or `Vec<u8>` on WASM/miri).
    #[cfg(not(any(miri, target_arch = "wasm32")))]
    pub struct Mmap(memmap2::Mmap);

    #[cfg(any(miri, target_arch = "wasm32"))]
    pub struct Mmap(Vec<u8>);

    #[cfg(not(any(miri, target_arch = "wasm32")))]
    impl Mmap {
        /// # Safety
        ///
        /// The given file must not be mutated (i.e., not written, not truncated, ...) until the mapping is closed.
        ///
        /// However in practice most callers do not ensure this, so uses of this function are likely unsound.
        #[inline]
        pub unsafe fn map(file: File) -> io::Result<Self> {
            unsafe { memmap2::MmapOptions::new().map_copy_read_only(&file).map(Mmap) }
        }
    }

    #[cfg(any(miri, target_arch = "wasm32"))]
    impl Mmap {
        #[inline]
        pub unsafe fn map(mut file: File) -> io::Result<Self> {
            use std::io::Read;
            let mut data = Vec::new();
            file.read_to_end(&mut data)?;
            Ok(Mmap(data))
        }
    }

    impl Deref for Mmap {
        type Target = [u8];
        #[inline]
        fn deref(&self) -> &[u8] {
            &self.0
        }
    }

    impl AsRef<[u8]> for Mmap {
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }

    #[cfg(not(any(miri, target_arch = "wasm32")))]
    pub struct MmapMut(memmap2::MmapMut);

    #[cfg(any(miri, target_arch = "wasm32"))]
    pub struct MmapMut(Vec<u8>);

    #[cfg(not(any(miri, target_arch = "wasm32")))]
    impl MmapMut {
        #[inline]
        pub fn map_anon(len: usize) -> io::Result<Self> {
            let mmap = memmap2::MmapMut::map_anon(len)?;
            Ok(MmapMut(mmap))
        }
        #[inline]
        pub fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
        #[inline]
        pub fn make_read_only(self) -> std::io::Result<Mmap> {
            let mmap = self.0.make_read_only()?;
            Ok(Mmap(mmap))
        }
    }

    #[cfg(any(miri, target_arch = "wasm32"))]
    impl MmapMut {
        #[inline]
        pub fn map_anon(len: usize) -> io::Result<Self> {
            let data = Vec::with_capacity(len);
            Ok(MmapMut(data))
        }
        #[inline]
        pub fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
        #[inline]
        pub fn make_read_only(self) -> std::io::Result<Mmap> {
            Ok(Mmap(self.0))
        }
    }

    impl Deref for MmapMut {
        type Target = [u8];
        #[inline]
        fn deref(&self) -> &[u8] {
            &self.0
        }
    }

    impl DerefMut for MmapMut {
        #[inline]
        fn deref_mut(&mut self) -> &mut [u8] {
            &mut self.0
        }
    }
}

#[cfg(target_os = "none")]
mod imp_none {
    //! M27 R4 B2: SemOS-target memmap shim. Backed by Vec<u8> like the
    //! miri/wasm32 host fallback. `File::read_to_end` requires
    //! `semos_std::io::Read`; on SemOS we use `semos_std::fs::read`
    //! directly via a wrapper accepting the same `File` newtype as
    //! upstream.

    use alloc::vec::Vec;
    use core::ops::{Deref, DerefMut};
    use semos_std::fs::File;
    use semos_std::io;

    pub struct Mmap(Vec<u8>);

    impl Mmap {
        /// # Safety
        /// Caller must ensure the file is not mutated while the mapping
        /// is live. On SemOS we read-into-buffer instead of true mmap,
        /// so this contract is trivially satisfied (we don't observe
        /// later writes), but the API matches upstream.
        #[inline]
        pub unsafe fn map(mut file: File) -> io::Result<Self> {
            use semos_std::io::Read;
            let mut data = Vec::new();
            file.read_to_end(&mut data)?;
            Ok(Mmap(data))
        }
    }

    impl Deref for Mmap {
        type Target = [u8];
        #[inline]
        fn deref(&self) -> &[u8] {
            &self.0
        }
    }

    impl AsRef<[u8]> for Mmap {
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }

    pub struct MmapMut(Vec<u8>);

    impl MmapMut {
        #[inline]
        pub fn map_anon(len: usize) -> io::Result<Self> {
            Ok(MmapMut(Vec::with_capacity(len)))
        }
        #[inline]
        pub fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
        #[inline]
        pub fn make_read_only(self) -> io::Result<Mmap> {
            Ok(Mmap(self.0))
        }
    }

    impl Deref for MmapMut {
        type Target = [u8];
        #[inline]
        fn deref(&self) -> &[u8] {
            &self.0
        }
    }

    impl DerefMut for MmapMut {
        #[inline]
        fn deref_mut(&mut self) -> &mut [u8] {
            &mut self.0
        }
    }
}

#[cfg(not(target_os = "none"))]
pub use imp_std::*;
#[cfg(target_os = "none")]
pub use imp_none::*;
