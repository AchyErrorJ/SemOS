//! Semantic Search Interface
//!
//! Provides high-level semantic search operations combining:
//! - Object registry (SUID-based lookup)
//! - Vector index (similarity search)
//! - Security tier filtering
//!
//! # Usage
//!
//! ```rust
//! // Store an object with its embedding
//! let suid = semantic_search.store(tier, content, embedding)?;
//!
//! // Find similar objects
//! let results = semantic_search.find_similar(query_embedding, max_tier, limit);
//! ```

use super::suid::SUID;
use super::object::{SemanticObject, SecurityTier, ObjectContent};
use super::registry;
use super::vector::{
    DefaultVector, SearchResult,
    DEFAULT_DIMS, MAX_VECTORS, global_vector_index, init_global_vector_index,
};

/// Maximum search results to return
pub const MAX_SEARCH_RESULTS: usize = 32;

/// Semantic search engine
/// Combines object registry with vector similarity search
pub struct SemanticSearch {
    initialized: bool,
}

impl SemanticSearch {
    /// Create a new semantic search instance
    pub const fn new() -> Self {
        Self { initialized: false }
    }

    /// Initialize the search engine
    pub fn init(&mut self) {
        init_global_vector_index();
        self.initialized = true;
    }

    /// Store an object with its vector embedding
    /// Returns the SUID on success
    pub fn store(
        &self,
        suid: SUID,
        tier: SecurityTier,
        owner: u8,
        content: &[u8],
        embedding: &[f32],
    ) -> Result<(), SearchError> {
        if !self.initialized {
            return Err(SearchError::NotInitialized);
        }

        // Create the semantic object
        let mut object = SemanticObject::new(suid, tier, owner);

        // Set content
        if !content.is_empty() {
            if let Some(obj_content) = ObjectContent::from_inline(content) {
                object.content = obj_content;
            } else {
                return Err(SearchError::ContentTooLarge);
            }
        }

        // Insert into registry
        {
            let mut registry = registry::global_registry();
            if !registry.insert(object) {
                return Err(SearchError::RegistryFull);
            }
        }

        // Add to vector index if embedding provided
        if !embedding.is_empty() {
            let vector = DefaultVector::from_slice(embedding);
            {
                let mut index = global_vector_index();
                if !index.insert(suid.high, suid.low, tier as u8, vector) {
                    // Remove from registry since vector insert failed
                    let mut registry = registry::global_registry();
                    registry.remove(&suid);
                    return Err(SearchError::IndexFull);
                }
            }
        }

        Ok(())
    }

    /// Update the vector embedding for an existing object
    pub fn update_embedding(&self, suid: &SUID, embedding: &[f32]) -> Result<(), SearchError> {
        if !self.initialized {
            return Err(SearchError::NotInitialized);
        }

        // Verify object exists
        {
            let registry = registry::global_registry();
            if registry.get(suid).is_none() {
                return Err(SearchError::NotFound);
            }
        }

        // Update vector index
        let vector = DefaultVector::from_slice(embedding);
        {
            let mut index = global_vector_index();
            if !index.update(suid.high, suid.low, vector.clone()) {
                // Object exists but not in vector index - insert it
                // Get tier from registry
                let registry = registry::global_registry();
                if let Some(obj) = registry.get(suid) {
                    if !index.insert(suid.high, suid.low, obj.tier as u8, vector) {
                        return Err(SearchError::IndexFull);
                    }
                }
            }
        }

        Ok(())
    }

    /// Find objects similar to the query embedding
    /// Returns up to `limit` results, filtered by max_tier
    pub fn find_similar(
        &self,
        query: &[f32],
        max_tier: u8,
        limit: usize,
        results: &mut [SearchResult],
    ) -> Result<usize, SearchError> {
        if !self.initialized {
            return Err(SearchError::NotInitialized);
        }

        let query_vec = DefaultVector::from_slice(query);

        {
            let index = global_vector_index();
            let found = index.search(&query_vec, max_tier, limit, results);
            Ok(found)
        }
    }

    /// Remove an object and its embedding
    pub fn remove(&self, suid: &SUID) -> Result<(), SearchError> {
        if !self.initialized {
            return Err(SearchError::NotInitialized);
        }

        // Remove from vector index
        {
            let mut index = global_vector_index();
            index.remove(suid.high, suid.low);
        }

        // Remove from registry
        {
            let mut registry = registry::global_registry();
            if registry.remove(suid).is_none() {
                return Err(SearchError::NotFound);
            }
        }

        Ok(())
    }

    /// Get statistics about the search index
    pub fn stats(&self) -> SearchStats {
        if !self.initialized {
            return SearchStats::default();
        }

        {
            let registry = registry::global_registry();
            let index = global_vector_index();

            SearchStats {
                object_count: registry.len(),
                vector_count: index.len(),
                max_objects: registry::MAX_OBJECTS,
                max_vectors: MAX_VECTORS,
                dimensions: DEFAULT_DIMS,
            }
        }
    }
}

/// Search error types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchError {
    /// Search engine not initialized
    NotInitialized,
    /// Object not found
    NotFound,
    /// Object registry is full
    RegistryFull,
    /// Vector index is full
    IndexFull,
    /// Content too large for inline storage
    ContentTooLarge,
    /// Permission denied (tier access)
    PermissionDenied,
    /// Invalid embedding dimensions
    InvalidDimensions,
}

impl SearchError {
    /// Convert to syscall error code
    pub fn to_error_code(self) -> i64 {
        match self {
            SearchError::NotInitialized => -20,
            SearchError::NotFound => -4,
            SearchError::RegistryFull => -9,
            SearchError::IndexFull => -21,
            SearchError::ContentTooLarge => -14,
            SearchError::PermissionDenied => -3,
            SearchError::InvalidDimensions => -22,
        }
    }
}

/// Search index statistics
#[derive(Default, Clone, Copy)]
pub struct SearchStats {
    /// Number of objects in registry
    pub object_count: usize,
    /// Number of vectors in index
    pub vector_count: usize,
    /// Maximum objects supported
    pub max_objects: usize,
    /// Maximum vectors supported
    pub max_vectors: usize,
    /// Vector dimensions
    pub dimensions: usize,
}

/// Global semantic search instance, behind the kernel mutex (2026-07-17
/// review, P1).
static GLOBAL_SEARCH: crate::sync::Mutex<SemanticSearch> =
    crate::sync::Mutex::new(SemanticSearch::new());

/// Lock the global semantic search instance.
pub fn global_search() -> crate::sync::MutexGuard<'static, SemanticSearch> {
    GLOBAL_SEARCH.lock()
}

/// Initialize the global semantic search
pub fn init_global_search() {
    GLOBAL_SEARCH.lock().init();
}
