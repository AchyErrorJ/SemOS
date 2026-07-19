//! Content Summarization
//!
//! Generates summaries of content for Tier 1 (Internal) objects.
//! The LLM sees only summaries, not the full content.
//!
//! # Summarization Strategies
//!
//! Since we're in a bare-metal kernel without ML capabilities,
//! we use extractive summarization techniques:
//!
//! 1. **First-N sentences**: Extract first N sentences as summary
//! 2. **Key phrase extraction**: Find important terms/phrases
//! 3. **Length-based truncation**: Truncate with indicator
//!
//! In the future, this could call out to an external summarization
//! service or use a small local model.

use super::MAX_ENTRY_SIZE;

/// Maximum summary length
pub const MAX_SUMMARY_LEN: usize = 512;

/// Summarization strategy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SummaryStrategy {
    /// Extract first N sentences
    FirstSentences = 0,
    /// Truncate with ellipsis
    Truncate = 1,
    /// Extract key phrases (simplified)
    KeyPhrases = 2,
}

/// A generated summary
pub struct Summary {
    /// Summary content
    content: [u8; MAX_SUMMARY_LEN],
    /// Actual length
    len: usize,
    /// Strategy used
    strategy: SummaryStrategy,
    /// Original content length
    original_len: usize,
    /// Compression ratio (0.0 - 1.0)
    ratio: f32,
}

impl Summary {
    /// Create an empty summary
    pub const fn empty() -> Self {
        Self {
            content: [0u8; MAX_SUMMARY_LEN],
            len: 0,
            strategy: SummaryStrategy::Truncate,
            original_len: 0,
            ratio: 0.0,
        }
    }

    /// Get summary content as bytes
    pub fn as_bytes(&self) -> &[u8] {
        &self.content[..self.len]
    }

    /// Get summary length
    pub fn len(&self) -> usize {
        self.len
    }

    /// Check if summary is empty
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Get compression ratio
    pub fn compression_ratio(&self) -> f32 {
        self.ratio
    }

    /// Get original content length
    pub fn original_length(&self) -> usize {
        self.original_len
    }
}

/// Content summarizer
pub struct Summarizer {
    initialized: bool,
    strategy: SummaryStrategy,
    max_sentences: usize,
    target_ratio: f32,
}

impl Summarizer {
    /// Create a new summarizer
    pub const fn new() -> Self {
        Self {
            initialized: false,
            strategy: SummaryStrategy::FirstSentences,
            max_sentences: 3,
            target_ratio: 0.25,
        }
    }

    /// Initialize the summarizer
    pub fn init(&mut self) {
        self.initialized = true;
    }

    /// Set summarization strategy
    pub fn set_strategy(&mut self, strategy: SummaryStrategy) {
        self.strategy = strategy;
    }

    /// Set maximum sentences for FirstSentences strategy
    pub fn set_max_sentences(&mut self, n: usize) {
        self.max_sentences = n;
    }

    /// Set target compression ratio
    pub fn set_target_ratio(&mut self, ratio: f32) {
        self.target_ratio = ratio.max(0.1).min(0.9);
    }

    /// Summarize content
    pub fn summarize(&self, content: &[u8]) -> Summary {
        if !self.initialized || content.is_empty() {
            return Summary::empty();
        }

        match self.strategy {
            SummaryStrategy::FirstSentences => self.summarize_first_sentences(content),
            SummaryStrategy::Truncate => self.summarize_truncate(content),
            SummaryStrategy::KeyPhrases => self.summarize_key_phrases(content),
        }
    }

    /// Extract first N sentences
    fn summarize_first_sentences(&self, content: &[u8]) -> Summary {
        let mut summary = Summary::empty();
        summary.original_len = content.len();
        summary.strategy = SummaryStrategy::FirstSentences;

        let mut sentences = 0;
        let mut out_pos = 0;

        // Add prefix
        let prefix = b"[SUMMARY] ";
        let prefix_len = prefix.len().min(MAX_SUMMARY_LEN);
        summary.content[..prefix_len].copy_from_slice(&prefix[..prefix_len]);
        out_pos = prefix_len;

        let mut i = 0;
        while i < content.len() && sentences < self.max_sentences && out_pos < MAX_SUMMARY_LEN - 3 {
            let c = content[i];
            summary.content[out_pos] = c;
            out_pos += 1;

            // Check for sentence end (.!?)
            if c == b'.' || c == b'!' || c == b'?' {
                // Look ahead for space or newline
                if i + 1 < content.len() {
                    let next = content[i + 1];
                    if next == b' ' || next == b'\n' || next == b'\r' {
                        sentences += 1;
                    }
                } else {
                    sentences += 1;
                }
            }

            i += 1;
        }

        // Add ellipsis if truncated
        if i < content.len() && out_pos + 3 <= MAX_SUMMARY_LEN {
            summary.content[out_pos] = b'.';
            summary.content[out_pos + 1] = b'.';
            summary.content[out_pos + 2] = b'.';
            out_pos += 3;
        }

        summary.len = out_pos;
        summary.ratio = if content.len() > 0 {
            summary.len as f32 / content.len() as f32
        } else {
            1.0
        };

        summary
    }

    /// Simple truncation with ellipsis
    fn summarize_truncate(&self, content: &[u8]) -> Summary {
        let mut summary = Summary::empty();
        summary.original_len = content.len();
        summary.strategy = SummaryStrategy::Truncate;

        // Calculate target length
        let target_len = ((content.len() as f32 * self.target_ratio) as usize)
            .min(MAX_SUMMARY_LEN - 20);

        // Add prefix
        let prefix = b"[SUMMARY] ";
        let prefix_len = prefix.len();
        summary.content[..prefix_len].copy_from_slice(prefix);
        let mut out_pos = prefix_len;

        if content.len() <= target_len {
            // Content fits entirely
            let copy_len = content.len().min(MAX_SUMMARY_LEN - out_pos);
            summary.content[out_pos..out_pos + copy_len].copy_from_slice(&content[..copy_len]);
            summary.len = out_pos + copy_len;
        } else {
            // Truncate at word boundary
            let mut end_pos = target_len;

            // Find last space before target
            while end_pos > 0 && content[end_pos - 1] != b' ' {
                end_pos -= 1;
            }

            if end_pos == 0 {
                end_pos = target_len;
            }

            let copy_len = end_pos.min(MAX_SUMMARY_LEN - out_pos - 3);
            summary.content[out_pos..out_pos + copy_len].copy_from_slice(&content[..copy_len]);
            out_pos += copy_len;

            // Add ellipsis
            summary.content[out_pos] = b'.';
            summary.content[out_pos + 1] = b'.';
            summary.content[out_pos + 2] = b'.';
            summary.len = out_pos + 3;
        }

        summary.ratio = if content.len() > 0 {
            summary.len as f32 / content.len() as f32
        } else {
            1.0
        };

        summary
    }

    /// Extract key phrases (simplified implementation)
    fn summarize_key_phrases(&self, content: &[u8]) -> Summary {
        let mut summary = Summary::empty();
        summary.original_len = content.len();
        summary.strategy = SummaryStrategy::KeyPhrases;

        // Add prefix
        let prefix = b"[KEY PHRASES] ";
        let prefix_len = prefix.len();
        summary.content[..prefix_len].copy_from_slice(prefix);
        let mut out_pos = prefix_len;

        // Simple word extraction: find capitalized words and long words
        let mut in_word = false;
        let mut word_start = 0;
        let mut phrases_found = 0;
        const MAX_PHRASES: usize = 10;

        for i in 0..content.len() {
            let c = content[i];
            let is_word_char = c.is_ascii_alphanumeric() || c == b'-' || c == b'\'';

            if is_word_char && !in_word {
                // Start of word
                word_start = i;
                in_word = true;
            } else if !is_word_char && in_word {
                // End of word
                let word_len = i - word_start;
                let word = &content[word_start..i];

                // Include if: capitalized (not at start), long, or contains numbers
                let include = (word_len >= 6)
                    || (word_start > 0 && word[0].is_ascii_uppercase())
                    || word.iter().any(|c| c.is_ascii_digit());

                if include && phrases_found < MAX_PHRASES {
                    // Add separator if not first
                    if phrases_found > 0 && out_pos + 2 < MAX_SUMMARY_LEN {
                        summary.content[out_pos] = b',';
                        summary.content[out_pos + 1] = b' ';
                        out_pos += 2;
                    }

                    // Copy word
                    let copy_len = word_len.min(MAX_SUMMARY_LEN - out_pos);
                    if copy_len > 0 {
                        summary.content[out_pos..out_pos + copy_len]
                            .copy_from_slice(&word[..copy_len]);
                        out_pos += copy_len;
                        phrases_found += 1;
                    }
                }

                in_word = false;
            }
        }

        summary.len = out_pos;
        summary.ratio = if content.len() > 0 {
            summary.len as f32 / content.len() as f32
        } else {
            1.0
        };

        summary
    }
}

/// Initialize the summarization subsystem
pub fn init() {
    super::context_builder::global_summarizer().init();
}
