//! Context Builder
//!
//! Builds LLM context from semantic objects across security tiers.
//! Applies appropriate transformations based on tier:
//! - Tier 0: Include verbatim
//! - Tier 1: Include summary only
//! - Tier 2: Include redacted version
//! - Tier 3: Exclude entirely

use crate::semantic::{SUID, SemanticObject};
use crate::memory::SecurityTier;
use crate::semantic::vector::SearchResult;
use super::{MAX_CONTEXT_ENTRIES, MAX_ENTRY_SIZE, MAX_CONTEXT_SIZE, LlmError};
use super::redact::Redactor;
use super::summarize::Summarizer;

/// A single entry in the LLM context
#[derive(Clone, Copy)]
pub struct ContextEntry {
    /// Source object SUID
    pub suid_high: u64,
    pub suid_low: u64,
    /// Original security tier
    pub original_tier: u8,
    /// How this entry was processed
    pub processing: ContentProcessing,
    /// Relevance score (from vector search)
    pub relevance: f32,
    /// Content buffer
    content: [u8; MAX_ENTRY_SIZE],
    /// Actual content length
    content_len: usize,
}

/// How content was processed for LLM
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ContentProcessing {
    /// Included verbatim (Tier 0)
    Verbatim = 0,
    /// Summarized (Tier 1)
    Summarized = 1,
    /// Redacted (Tier 2)
    Redacted = 2,
    /// Excluded (Tier 3) - placeholder entry
    Excluded = 3,
}

impl ContextEntry {
    /// Create an empty context entry
    pub const fn empty() -> Self {
        Self {
            suid_high: 0,
            suid_low: 0,
            original_tier: 0,
            processing: ContentProcessing::Excluded,
            relevance: 0.0,
            content: [0u8; MAX_ENTRY_SIZE],
            content_len: 0,
        }
    }

    /// Create a new context entry
    pub fn new(
        suid: &SUID,
        tier: SecurityTier,
        processing: ContentProcessing,
        relevance: f32,
        content: &[u8],
    ) -> Self {
        let mut entry = Self::empty();
        entry.suid_high = suid.high;
        entry.suid_low = suid.low;
        entry.original_tier = tier as u8;
        entry.processing = processing;
        entry.relevance = relevance;

        let copy_len = content.len().min(MAX_ENTRY_SIZE);
        entry.content[..copy_len].copy_from_slice(&content[..copy_len]);
        entry.content_len = copy_len;

        entry
    }

    /// Get the content as a byte slice
    pub fn content(&self) -> &[u8] {
        &self.content[..self.content_len]
    }

    /// Check if this entry has content
    pub fn has_content(&self) -> bool {
        self.content_len > 0
    }
}

/// Complete LLM context built from multiple objects
pub struct LlmContext {
    /// Context entries
    entries: [ContextEntry; MAX_CONTEXT_ENTRIES],
    /// Number of entries
    count: usize,
    /// Total content size
    total_size: usize,
    /// Requester's maximum tier
    requester_tier: u8,
}

impl LlmContext {
    /// Create an empty context
    pub const fn new() -> Self {
        Self {
            entries: [ContextEntry::empty(); MAX_CONTEXT_ENTRIES],
            count: 0,
            total_size: 0,
            requester_tier: 0,
        }
    }

    /// Add an entry to the context
    pub fn add_entry(&mut self, entry: ContextEntry) -> bool {
        if self.count >= MAX_CONTEXT_ENTRIES {
            return false;
        }

        let new_size = self.total_size + entry.content_len;
        if new_size > MAX_CONTEXT_SIZE {
            return false;
        }

        self.entries[self.count] = entry;
        self.count += 1;
        self.total_size = new_size;
        true
    }

    /// Get number of entries
    pub fn len(&self) -> usize {
        self.count
    }

    /// Check if context is empty
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Get total content size
    pub fn total_size(&self) -> usize {
        self.total_size
    }

    /// Get an entry by index
    pub fn get(&self, index: usize) -> Option<&ContextEntry> {
        if index < self.count {
            Some(&self.entries[index])
        } else {
            None
        }
    }

    /// Iterate over entries
    pub fn iter(&self) -> impl Iterator<Item = &ContextEntry> {
        self.entries[..self.count].iter()
    }

    /// Get statistics about the context
    pub fn stats(&self) -> ContextStats {
        let mut stats = ContextStats::default();
        stats.total_entries = self.count;
        stats.total_size = self.total_size;

        for entry in self.iter() {
            match entry.processing {
                ContentProcessing::Verbatim => stats.verbatim_count += 1,
                ContentProcessing::Summarized => stats.summarized_count += 1,
                ContentProcessing::Redacted => stats.redacted_count += 1,
                ContentProcessing::Excluded => stats.excluded_count += 1,
            }
        }

        stats
    }
}

/// Statistics about context contents
#[derive(Default, Clone, Copy)]
pub struct ContextStats {
    pub total_entries: usize,
    pub total_size: usize,
    pub verbatim_count: usize,
    pub summarized_count: usize,
    pub redacted_count: usize,
    pub excluded_count: usize,
}

/// Context builder for LLM requests
pub struct ContextBuilder {
    initialized: bool,
}

impl ContextBuilder {
    /// Create a new context builder
    pub const fn new() -> Self {
        Self { initialized: false }
    }

    /// Initialize the context builder
    pub fn init(&mut self) {
        self.initialized = true;
    }

    /// Build context from search results
    ///
    /// Takes vector search results and builds an LLM-ready context
    /// with appropriate tier-based transformations.
    pub fn build_from_search(
        &self,
        results: &[SearchResult],
        result_count: usize,
        requester_tier: u8,
    ) -> Result<LlmContext, LlmError> {
        if !self.initialized {
            return Err(LlmError::NotInitialized);
        }

        let mut context = LlmContext::new();
        context.requester_tier = requester_tier;

        // Get registry for object lookup
        let registry = crate::semantic::registry::global_registry();

        for i in 0..result_count {
            let result = &results[i];
            let suid = SUID::new(result.suid_high, result.suid_low);

            // Look up the object
            if let Some(object) = registry.get(&suid) {
                // Check tier access
                let obj_tier = object.tier as u8;
                if obj_tier > requester_tier {
                    // Can't access this object at all
                    continue;
                }

                // Process content based on tier
                let entry = self.process_object(object, result.score)?;

                if !context.add_entry(entry) {
                    // Context full or too large
                    break;
                }
            }
        }

        if context.is_empty() {
            return Err(LlmError::NoResults);
        }

        Ok(context)
    }

    /// Build context from a list of SUIDs
    pub fn build_from_suids(
        &self,
        suids: &[(u64, u64)],
        requester_tier: u8,
    ) -> Result<LlmContext, LlmError> {
        if !self.initialized {
            return Err(LlmError::NotInitialized);
        }

        let mut context = LlmContext::new();
        context.requester_tier = requester_tier;

        let registry = crate::semantic::registry::global_registry();

        for (high, low) in suids {
            let suid = SUID::new(*high, *low);

            if let Some(object) = registry.get(&suid) {
                let obj_tier = object.tier as u8;
                if obj_tier > requester_tier {
                    continue;
                }

                // Use 1.0 as default relevance for direct lookups
                let entry = self.process_object(object, 1.0)?;

                if !context.add_entry(entry) {
                    break;
                }
            }
        }

        if context.is_empty() {
            return Err(LlmError::NoResults);
        }

        Ok(context)
    }

    /// Process a single object according to its tier
    fn process_object(
        &self,
        object: &SemanticObject,
        relevance: f32,
    ) -> Result<ContextEntry, LlmError> {
        let content = object.content.as_bytes().unwrap_or(&[]);
        let tier = object.tier;

        let mut entry = ContextEntry::empty();
        entry.suid_high = object.suid.high;
        entry.suid_low = object.suid.low;
        entry.original_tier = tier as u8;
        entry.relevance = relevance;

        match tier {
            SecurityTier::Public => {
                // Tier 0: Include verbatim
                entry.processing = ContentProcessing::Verbatim;
                let copy_len = content.len().min(MAX_ENTRY_SIZE);
                entry.content[..copy_len].copy_from_slice(&content[..copy_len]);
                entry.content_len = copy_len;
            }
            SecurityTier::Internal => {
                // Tier 1: Summarize
                let summary = global_summarizer().summarize(content);
                entry.processing = ContentProcessing::Summarized;
                let summary_bytes = summary.as_bytes();
                let copy_len = summary_bytes.len().min(MAX_ENTRY_SIZE);
                entry.content[..copy_len].copy_from_slice(&summary_bytes[..copy_len]);
                entry.content_len = copy_len;
            }
            SecurityTier::Sensitive => {
                // Tier 2: Redact
                entry.processing = ContentProcessing::Redacted;
                let redacted_len = global_redactor()
                    .redact(content, &mut entry.content);
                entry.content_len = redacted_len;
            }
            SecurityTier::Secret => {
                // Tier 3: Exclude (should not reach here due to tier check)
                return Err(LlmError::AccessDenied);
            }
        };

        Ok(entry)
    }
}

// Global instances, behind the yield-on-contention kernel mutex (the old
// `static mut` + `&'static mut` pattern raced when an interrupts-enabled
// syscall handler was preempted mid-borrow — 2026-07-17 review, P1).
static GLOBAL_CONTEXT_BUILDER: crate::sync::Mutex<ContextBuilder> =
    crate::sync::Mutex::new(ContextBuilder::new());
static GLOBAL_REDACTOR: crate::sync::Mutex<super::redact::Redactor> =
    crate::sync::Mutex::new(super::redact::Redactor::new());
static GLOBAL_SUMMARIZER: crate::sync::Mutex<super::summarize::Summarizer> =
    crate::sync::Mutex::new(super::summarize::Summarizer::new());

/// Lock the global context builder. Never hold the guard across a blocking
/// call or a nested `dispatch()`.
pub fn global_context_builder() -> crate::sync::MutexGuard<'static, ContextBuilder> {
    GLOBAL_CONTEXT_BUILDER.lock()
}

/// Lock the global redactor.
pub fn global_redactor() -> crate::sync::MutexGuard<'static, super::redact::Redactor> {
    GLOBAL_REDACTOR.lock()
}

/// Lock the global summarizer.
pub fn global_summarizer() -> crate::sync::MutexGuard<'static, super::summarize::Summarizer> {
    GLOBAL_SUMMARIZER.lock()
}

/// Initialize the context builder subsystem
pub fn init() {
    GLOBAL_CONTEXT_BUILDER.lock().init();
}
