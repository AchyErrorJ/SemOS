use alloc::borrow::ToOwned;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use rustc_data_structures::owned_slice::OwnedSlice;
use rustc_hir::def_path_hash_map::{Config as HashMapConfig, DefPathHashMap};
use rustc_serialize::{Decodable, Decoder, Encodable, Encoder};
use rustc_span::def_id::{DefIndex, DefPathHash};

use crate::rmeta::EncodeContext;
use crate::rmeta::decoder::BlobDecodeContext;

#[cfg(not(target_os = "none"))]
pub(crate) enum DefPathHashMapRef<'tcx> {
    OwnedFromMetadata(odht::HashTable<HashMapConfig, OwnedSlice>),
    BorrowedFromTcx(&'tcx DefPathHashMap),
}

// M27 §1.3: incremental cache dropped on SemOS. odht (open-addressing
// hash table) is host-only; the SemOS rmeta path never reads/writes a
// def-path hash map (per Cargo.toml gate of odht). Stub the enum so
// downstream type references compile, and leave the impls trivial.
#[cfg(target_os = "none")]
pub(crate) enum DefPathHashMapRef<'tcx> {
    OwnedFromMetadata(OwnedSlice),
    BorrowedFromTcx(&'tcx DefPathHashMap),
}

impl DefPathHashMapRef<'_> {
    #[inline]
    pub(crate) fn def_path_hash_to_def_index(
        &self,
        def_path_hash: &DefPathHash,
    ) -> Option<DefIndex> {
        match *self {
            #[cfg(not(target_os = "none"))]
            DefPathHashMapRef::OwnedFromMetadata(ref map) => map.get(&def_path_hash.local_hash()),
            #[cfg(target_os = "none")]
            DefPathHashMapRef::OwnedFromMetadata(_) => None,
            DefPathHashMapRef::BorrowedFromTcx(_) => {
                panic!("DefPathHashMap::BorrowedFromTcx variant only exists for serialization")
            }
        }
    }
}

impl<'a, 'tcx> Encodable<EncodeContext<'a, 'tcx>> for DefPathHashMapRef<'tcx> {
    fn encode(&self, e: &mut EncodeContext<'a, 'tcx>) {
        match *self {
            DefPathHashMapRef::BorrowedFromTcx(def_path_hash_map) => {
                #[cfg(not(target_os = "none"))]
                {
                    let bytes = def_path_hash_map.raw_bytes();
                    e.emit_usize(bytes.len());
                    e.emit_raw_bytes(bytes);
                }
                // SemOS BTreeMap stub has no raw_bytes — emit empty.
                #[cfg(target_os = "none")]
                {
                    let _ = def_path_hash_map;
                    e.emit_usize(0);
                }
            }
            DefPathHashMapRef::OwnedFromMetadata(_) => {
                panic!("DefPathHashMap::OwnedFromMetadata variant only exists for deserialization")
            }
        }
    }
}

impl<'a> Decodable<BlobDecodeContext<'a>> for DefPathHashMapRef<'static> {
    fn decode(d: &mut BlobDecodeContext<'a>) -> DefPathHashMapRef<'static> {
        let len = d.read_usize();
        let pos = d.position();
        let o = d.blob().bytes().clone().slice(|blob| &blob[pos..pos + len]);

        // Although we already have the data we need via the `OwnedSlice`, we still need
        // to advance the `DecodeContext`'s position so it's in a valid state after
        // the method. We use `read_raw_bytes()` for that.
        let _ = d.read_raw_bytes(len);

        #[cfg(not(target_os = "none"))]
        {
            let inner = odht::HashTable::from_raw_bytes(o).unwrap_or_else(|e| {
                panic!("decode error: {e}");
            });
            DefPathHashMapRef::OwnedFromMetadata(inner)
        }
        // SemOS: keep the owned slice but never query it.
        #[cfg(target_os = "none")]
        DefPathHashMapRef::OwnedFromMetadata(o)
    }
}
