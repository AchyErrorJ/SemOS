//! Vector Embeddings and Similarity Search
//!
//! This module implements vector operations for semantic similarity search.
//! Vectors are fixed-dimension arrays of f32 values representing semantic embeddings.
//!
//! # Supported Operations
//! - Cosine similarity
//! - Euclidean distance
//! - Dot product
//!
//! # Dimensions
//! We support multiple embedding sizes:
//! - DIMS_SMALL (64): For testing and low-memory scenarios
//! - DIMS_MEDIUM (128): Balanced performance
//! - DIMS_LARGE (384): MiniLM-compatible embeddings

use core::cmp::Ordering;

/// Small embedding dimension (for testing)
pub const DIMS_SMALL: usize = 64;

/// Medium embedding dimension
pub const DIMS_MEDIUM: usize = 128;

/// Large embedding dimension (MiniLM-compatible)
pub const DIMS_LARGE: usize = 384;

/// Default dimension used by the kernel
pub const DEFAULT_DIMS: usize = DIMS_SMALL;

/// Maximum vectors in the search index
pub const MAX_VECTORS: usize = 512;

/// A fixed-size vector embedding
#[derive(Clone)]
pub struct Vector<const N: usize> {
    /// The embedding values
    pub data: [f32; N],
    /// Whether this vector is normalized
    pub normalized: bool,
}

impl<const N: usize> Vector<N> {
    /// Create a zero vector
    pub const fn zero() -> Self {
        Self {
            data: [0.0; N],
            normalized: false,
        }
    }

    /// Create from an array
    pub fn from_array(data: [f32; N]) -> Self {
        Self {
            data,
            normalized: false,
        }
    }

    /// Create from a slice (truncates or pads with zeros)
    pub fn from_slice(slice: &[f32]) -> Self {
        let mut data = [0.0f32; N];
        let copy_len = core::cmp::min(slice.len(), N);
        data[..copy_len].copy_from_slice(&slice[..copy_len]);
        Self {
            data,
            normalized: false,
        }
    }

    /// Get the dimension
    pub const fn dims(&self) -> usize {
        N
    }

    /// Compute the L2 (Euclidean) norm
    pub fn norm(&self) -> f32 {
        self.dot(self).sqrt()
    }

    /// Compute dot product with another vector
    pub fn dot(&self, other: &Self) -> f32 {
        let mut sum = 0.0f32;
        for i in 0..N {
            sum += self.data[i] * other.data[i];
        }
        sum
    }

    /// Normalize the vector in-place (make unit length)
    pub fn normalize(&mut self) {
        let norm = self.norm();
        if norm > 1e-10 {
            for i in 0..N {
                self.data[i] /= norm;
            }
            self.normalized = true;
        }
    }

    /// Return a normalized copy
    pub fn normalized(&self) -> Self {
        let mut v = self.clone();
        v.normalize();
        v
    }

    /// Compute cosine similarity with another vector
    /// Returns value in range [-1, 1] where 1 = identical direction
    pub fn cosine_similarity(&self, other: &Self) -> f32 {
        let dot = self.dot(other);
        let norm_a = self.norm();
        let norm_b = other.norm();

        if norm_a < 1e-10 || norm_b < 1e-10 {
            return 0.0;
        }

        dot / (norm_a * norm_b)
    }

    /// Compute cosine similarity when both vectors are pre-normalized
    /// (Faster - just dot product)
    pub fn cosine_similarity_normalized(&self, other: &Self) -> f32 {
        debug_assert!(self.normalized && other.normalized);
        self.dot(other)
    }

    /// Compute Euclidean distance
    pub fn euclidean_distance(&self, other: &Self) -> f32 {
        let mut sum = 0.0f32;
        for i in 0..N {
            let diff = self.data[i] - other.data[i];
            sum += diff * diff;
        }
        sum.sqrt()
    }

    /// Compute squared Euclidean distance (faster, no sqrt)
    pub fn euclidean_distance_squared(&self, other: &Self) -> f32 {
        let mut sum = 0.0f32;
        for i in 0..N {
            let diff = self.data[i] - other.data[i];
            sum += diff * diff;
        }
        sum
    }

    /// Add another vector
    pub fn add(&self, other: &Self) -> Self {
        let mut result = Self::zero();
        for i in 0..N {
            result.data[i] = self.data[i] + other.data[i];
        }
        result
    }

    /// Subtract another vector
    pub fn sub(&self, other: &Self) -> Self {
        let mut result = Self::zero();
        for i in 0..N {
            result.data[i] = self.data[i] - other.data[i];
        }
        result
    }

    /// Scale by a scalar
    pub fn scale(&self, s: f32) -> Self {
        let mut result = Self::zero();
        for i in 0..N {
            result.data[i] = self.data[i] * s;
        }
        result
    }
}

impl<const N: usize> Default for Vector<N> {
    fn default() -> Self {
        Self::zero()
    }
}

/// Type alias for the default vector size
pub type DefaultVector = Vector<DEFAULT_DIMS>;

/// A search result with SUID and similarity score
#[derive(Clone, Copy)]
pub struct SearchResult {
    /// Object SUID high bits
    pub suid_high: u64,
    /// Object SUID low bits
    pub suid_low: u64,
    /// Similarity score (higher = more similar)
    pub score: f32,
}

impl SearchResult {
    pub fn new(suid_high: u64, suid_low: u64, score: f32) -> Self {
        Self { suid_high, suid_low, score }
    }
}

/// Vector index entry
struct VectorEntry<const N: usize> {
    /// Object SUID
    suid_high: u64,
    suid_low: u64,
    /// Security tier (for access control during search)
    tier: u8,
    /// The vector embedding (normalized for fast cosine similarity)
    vector: Vector<N>,
    /// Is this entry active?
    active: bool,
}

impl<const N: usize> VectorEntry<N> {
    const fn empty() -> Self {
        Self {
            suid_high: 0,
            suid_low: 0,
            tier: 0,
            vector: Vector::zero(),
            active: false,
        }
    }
}

/// Vector search index
/// Provides fast similarity search over stored vectors
pub struct VectorIndex<const N: usize> {
    /// Stored vectors
    entries: [VectorEntry<N>; MAX_VECTORS],
    /// Number of active entries
    count: usize,
}

impl<const N: usize> VectorIndex<N> {
    /// Create an empty index
    pub const fn new() -> Self {
        const EMPTY: VectorEntry<64> = VectorEntry::empty();
        // Can't use generic const in array init, so we use a workaround
        Self {
            entries: unsafe { core::mem::zeroed() },
            count: 0,
        }
    }

    /// Initialize the index
    pub fn init(&mut self) {
        for entry in self.entries.iter_mut() {
            entry.active = false;
        }
        self.count = 0;
    }

    /// Add a vector to the index
    pub fn insert(&mut self, suid_high: u64, suid_low: u64, tier: u8, vector: Vector<N>) -> bool {
        // Find empty slot or existing entry
        for entry in self.entries.iter_mut() {
            if !entry.active {
                entry.suid_high = suid_high;
                entry.suid_low = suid_low;
                entry.tier = tier;
                entry.vector = vector.normalized();
                entry.active = true;
                self.count += 1;
                return true;
            }
        }
        false // Index full
    }

    /// Remove a vector from the index
    pub fn remove(&mut self, suid_high: u64, suid_low: u64) -> bool {
        for entry in self.entries.iter_mut() {
            if entry.active && entry.suid_high == suid_high && entry.suid_low == suid_low {
                entry.active = false;
                self.count -= 1;
                return true;
            }
        }
        false
    }

    /// Update a vector in the index
    pub fn update(&mut self, suid_high: u64, suid_low: u64, vector: Vector<N>) -> bool {
        for entry in self.entries.iter_mut() {
            if entry.active && entry.suid_high == suid_high && entry.suid_low == suid_low {
                entry.vector = vector.normalized();
                return true;
            }
        }
        false
    }

    /// Search for similar vectors
    /// Returns up to `limit` results, filtered by max_tier
    pub fn search(
        &self,
        query: &Vector<N>,
        max_tier: u8,
        limit: usize,
        results: &mut [SearchResult],
    ) -> usize {
        let query_norm = query.normalized();
        let mut found = 0;

        // Collect all matching results with scores
        let mut candidates: [(f32, usize); MAX_VECTORS] = [(0.0, 0); MAX_VECTORS];
        let mut num_candidates = 0;

        for (idx, entry) in self.entries.iter().enumerate() {
            if entry.active && entry.tier <= max_tier {
                let score = query_norm.cosine_similarity_normalized(&entry.vector);
                candidates[num_candidates] = (score, idx);
                num_candidates += 1;
            }
        }

        // Sort by score (descending) using simple insertion sort
        for i in 1..num_candidates {
            let key = candidates[i];
            let mut j = i;
            while j > 0 && candidates[j - 1].0 < key.0 {
                candidates[j] = candidates[j - 1];
                j -= 1;
            }
            candidates[j] = key;
        }

        // Copy top results
        let result_count = core::cmp::min(limit, core::cmp::min(num_candidates, results.len()));
        for i in 0..result_count {
            let (score, idx) = candidates[i];
            let entry = &self.entries[idx];
            results[i] = SearchResult::new(entry.suid_high, entry.suid_low, score);
            found += 1;
        }

        found
    }

    /// Get the number of vectors in the index
    pub fn len(&self) -> usize {
        self.count
    }

    /// Check if index is empty
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// Global vector index instance, behind the kernel mutex (2026-07-17
/// review, P1).
static GLOBAL_VECTOR_INDEX: crate::sync::Mutex<VectorIndex<DEFAULT_DIMS>> =
    crate::sync::Mutex::new(VectorIndex {
        entries: unsafe { core::mem::zeroed() },
        count: 0,
    });

/// Lock the global vector index.
pub fn global_vector_index() -> crate::sync::MutexGuard<'static, VectorIndex<DEFAULT_DIMS>> {
    GLOBAL_VECTOR_INDEX.lock()
}

/// Initialize the global vector index
pub fn init_global_vector_index() {
    GLOBAL_VECTOR_INDEX.lock().init();
}

// ============================================================================
// Utility functions for f32 operations in no_std
// ============================================================================

/// Approximate square root using Newton-Raphson
trait FloatExt {
    fn sqrt(self) -> Self;
    fn abs(self) -> Self;
}

impl FloatExt for f32 {
    fn sqrt(self) -> f32 {
        if self <= 0.0 {
            return 0.0;
        }

        // Fast inverse square root (Quake III style) + Newton-Raphson
        let mut guess = self * 0.5;
        let mut last = 0.0f32;

        // Newton-Raphson iterations
        for _ in 0..10 {
            guess = 0.5 * (guess + self / guess);
            if (guess - last).abs() < 1e-7 {
                break;
            }
            last = guess;
        }

        guess
    }

    fn abs(self) -> f32 {
        if self < 0.0 { -self } else { self }
    }
}
