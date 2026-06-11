//! Support for serializing the dep-graph and reloading it.

// tidy-alphabetical-start
#![cfg_attr(target_os = "none", no_std)]
#![deny(missing_docs)]
#![cfg_attr(not(target_os = "none"), feature(file_buffered))]
// tidy-alphabetical-end

#[macro_use]
extern crate alloc;

#[cfg(not(target_os = "none"))]
extern crate std as semos_std;
#[cfg(not(target_os = "none"))]
extern crate std;

#[cfg(not(target_os = "none"))]
mod assert_dep_graph;
#[cfg(not(target_os = "none"))]
mod errors;
#[cfg(not(target_os = "none"))]
mod persist;

#[cfg(not(target_os = "none"))]
pub use persist::{
    LoadResult, copy_cgu_workproduct_to_incr_comp_cache_dir, finalize_session_directory,
    in_incr_comp_dir, in_incr_comp_dir_sess, load_query_result_cache, save_work_product_index,
    setup_dep_graph,
};
use rustc_middle::util::Providers;

#[allow(missing_docs)]
#[cfg(not(target_os = "none"))]
pub fn provide(providers: &mut Providers) {
    providers.hooks.save_dep_graph =
        |tcx| tcx.sess.time("serialize_dep_graph", || persist::save_dep_graph(tcx));
}

#[allow(missing_docs)]
#[cfg(target_os = "none")]
pub fn provide(providers: &mut Providers) {
    providers.hooks.save_dep_graph = |_tcx| {};
}

#[cfg(not(target_os = "none"))]
rustc_fluent_macro::fluent_messages! { "../messages.ftl" }

// SemOS stub surface — incremental is dropped per M27 §1.3.
#[cfg(target_os = "none")]
mod semos_stub {
    use alloc::vec::Vec;
    use rustc_data_structures::fx::FxIndexMap;
    use rustc_hashes::Hash64;
    use rustc_hir::definitions::DefPathHash;
    use rustc_middle::dep_graph::{DepGraph, WorkProduct, WorkProductId};
    use rustc_middle::query::on_disk_cache::OnDiskCache;
    use rustc_session::Session;
    use rustc_span::{Symbol, def_id::StableCrateId};
    use semos_std::path::{Path, PathBuf};

    /// Result of loading something from disk.
    pub enum LoadResult<T> {
        /// Loaded successfully.
        Ok {
            /// Loaded value.
            data: T,
        },
        /// The file is missing.
        DataOutOfDate,
        /// Loading failed with the given error.
        LoadDepGraph(PathBuf, semos_std::io::Error),
        /// Couldn't load the file because its needed crate is missing.
        Error {
            /// Message describing the failure.
            message: alloc::string::String,
        },
    }

    /// Returns the path to a file in the incremental compilation cache directory.
    pub fn in_incr_comp_dir_sess(_sess: &Session, _file_name: &str) -> PathBuf { PathBuf::new() }
    /// Returns the path to a file in the given incremental compilation session directory.
    pub fn in_incr_comp_dir(_dir: &Path, _file_name: &str) -> PathBuf { PathBuf::new() }
    /// Finalizes the incremental compilation session directory.
    pub fn finalize_session_directory(_sess: &Session, _svh: Option<rustc_data_structures::svh::Svh>) {}
    /// Loads the query result cache from disk.
    pub fn load_query_result_cache(_sess: &Session) -> Option<OnDiskCache> { None }
    /// Sets up the dependency graph.
    pub fn setup_dep_graph(_sess: &Session, _crate_name: Symbol, _stable_crate_id: StableCrateId) -> DepGraph {
        DepGraph::new_disabled()
    }
    /// Saves the work product index.
    pub fn save_work_product_index(_sess: &Session, _dep_graph: &DepGraph, _new_work_products: FxIndexMap<WorkProductId, WorkProduct>) {}
    /// Copies a CGU work product to the incremental compilation directory.
    pub fn copy_cgu_workproduct_to_incr_comp_cache_dir(_sess: &Session, _cgu_name: &str, _files: &[(&'static str, &Path)], _known_links: &[PathBuf]) -> Option<(WorkProductId, WorkProduct)> { None }

    // Unused-import silencer.
    #[allow(dead_code)]
    fn _unused() {
        let _: Vec<u8> = Vec::new();
        let _ = Hash64::new(0);
        let _: Option<DefPathHash> = None;
    }
}

#[cfg(target_os = "none")]
pub use semos_stub::{
    LoadResult, copy_cgu_workproduct_to_incr_comp_cache_dir, finalize_session_directory,
    in_incr_comp_dir, in_incr_comp_dir_sess, load_query_result_cache, save_work_product_index,
    setup_dep_graph,
};
