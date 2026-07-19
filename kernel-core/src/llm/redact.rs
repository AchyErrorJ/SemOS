//! Content Redaction
//!
//! Redacts sensitive information from content before LLM access.
//! Used for Tier 2 (Sensitive) objects.
//!
//! # Redaction Patterns
//!
//! - Email addresses: user@domain.com -> [EMAIL]
//! - Phone numbers: +1-555-1234 -> [PHONE]
//! - SSN patterns: 123-45-6789 -> [SSN]
//! - Credit cards: 4111-1111-1111-1111 -> [CARD]
//! - IP addresses: 192.168.1.1 -> [IP]
//! - API keys: sk-xxx... -> [KEY]

use super::MAX_ENTRY_SIZE;

/// Redaction level determines how aggressive the redaction is
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RedactionLevel {
    /// Light - only obvious PII (emails, SSN, cards)
    Light = 0,
    /// Standard - PII plus identifiers (IPs, keys)
    Standard = 1,
    /// Aggressive - all patterns plus names and numbers
    Aggressive = 2,
}

/// Placeholder tokens for redacted content
pub mod tokens {
    pub const EMAIL: &[u8] = b"[EMAIL]";
    pub const PHONE: &[u8] = b"[PHONE]";
    pub const SSN: &[u8] = b"[SSN]";
    pub const CARD: &[u8] = b"[CARD]";
    pub const IP: &[u8] = b"[IP]";
    pub const KEY: &[u8] = b"[KEY]";
    pub const NAME: &[u8] = b"[NAME]";
    pub const NUMBER: &[u8] = b"[NUM]";
    pub const REDACTED: &[u8] = b"[REDACTED]";
}

/// Content redactor
pub struct Redactor {
    initialized: bool,
    level: RedactionLevel,
}

impl Redactor {
    /// Create a new redactor
    pub const fn new() -> Self {
        Self {
            initialized: false,
            level: RedactionLevel::Standard,
        }
    }

    /// Initialize the redactor
    pub fn init(&mut self) {
        self.initialized = true;
    }

    /// Set redaction level
    pub fn set_level(&mut self, level: RedactionLevel) {
        self.level = level;
    }

    /// Redact content, writing result to output buffer
    /// Returns the length of redacted content
    pub fn redact(&self, input: &[u8], output: &mut [u8]) -> usize {
        if !self.initialized || input.is_empty() {
            return 0;
        }

        let mut out_pos = 0;
        let mut in_pos = 0;

        while in_pos < input.len() && out_pos < output.len() {
            // Try to match patterns at current position
            if let Some((token, skip)) = self.match_pattern(&input[in_pos..]) {
                // Copy redaction token to output
                let copy_len = token.len().min(output.len() - out_pos);
                output[out_pos..out_pos + copy_len].copy_from_slice(&token[..copy_len]);
                out_pos += copy_len;
                in_pos += skip;
            } else {
                // No match, copy character as-is
                output[out_pos] = input[in_pos];
                out_pos += 1;
                in_pos += 1;
            }
        }

        out_pos
    }

    /// Try to match a redaction pattern at the start of input
    /// Returns (replacement_token, characters_to_skip) if matched
    fn match_pattern(&self, input: &[u8]) -> Option<(&'static [u8], usize)> {
        // Check for email pattern (contains @ followed by .)
        if let Some(len) = self.match_email(input) {
            return Some((tokens::EMAIL, len));
        }

        // Check for SSN pattern (XXX-XX-XXXX)
        if let Some(len) = self.match_ssn(input) {
            return Some((tokens::SSN, len));
        }

        // Check for credit card pattern (16 digits, possibly with dashes/spaces)
        if let Some(len) = self.match_credit_card(input) {
            return Some((tokens::CARD, len));
        }

        // Check for phone number pattern
        if let Some(len) = self.match_phone(input) {
            return Some((tokens::PHONE, len));
        }

        // Standard and Aggressive levels
        if self.level as u8 >= RedactionLevel::Standard as u8 {
            // Check for IP address
            if let Some(len) = self.match_ip(input) {
                return Some((tokens::IP, len));
            }

            // Check for API key patterns
            if let Some(len) = self.match_api_key(input) {
                return Some((tokens::KEY, len));
            }
        }

        // Aggressive level only
        if self.level == RedactionLevel::Aggressive {
            // Check for standalone numbers (potential IDs)
            if let Some(len) = self.match_long_number(input) {
                return Some((tokens::NUMBER, len));
            }
        }

        None
    }

    /// Match email pattern: word@word.word
    fn match_email(&self, input: &[u8]) -> Option<usize> {
        if input.len() < 5 {
            return None;
        }

        let mut at_pos = None;
        let mut dot_after_at = false;
        let mut i = 0;

        // Find @ symbol
        while i < input.len() {
            if input[i] == b'@' {
                at_pos = Some(i);
                break;
            }
            if !is_email_char(input[i]) {
                return None;
            }
            i += 1;
        }

        let at_pos = at_pos?;
        if at_pos == 0 {
            return None;
        }

        // Check domain part after @
        i = at_pos + 1;
        while i < input.len() && is_email_char(input[i]) {
            if input[i] == b'.' && i > at_pos + 1 {
                dot_after_at = true;
            }
            i += 1;
        }

        if dot_after_at && i > at_pos + 3 {
            Some(i)
        } else {
            None
        }
    }

    /// Match SSN pattern: XXX-XX-XXXX
    fn match_ssn(&self, input: &[u8]) -> Option<usize> {
        if input.len() < 11 {
            return None;
        }

        // Check pattern: 3 digits, dash, 2 digits, dash, 4 digits
        if input[0..3].iter().all(|&c| c.is_ascii_digit())
            && input[3] == b'-'
            && input[4..6].iter().all(|&c| c.is_ascii_digit())
            && input[6] == b'-'
            && input[7..11].iter().all(|&c| c.is_ascii_digit())
        {
            Some(11)
        } else {
            None
        }
    }

    /// Match credit card pattern: 16 digits (with optional dashes/spaces)
    fn match_credit_card(&self, input: &[u8]) -> Option<usize> {
        if input.len() < 13 {
            return None;
        }

        let mut digit_count = 0;
        let mut i = 0;

        while i < input.len() && digit_count < 16 {
            if input[i].is_ascii_digit() {
                digit_count += 1;
            } else if input[i] == b'-' || input[i] == b' ' {
                // Allow separators
            } else {
                break;
            }
            i += 1;
        }

        if digit_count >= 13 {
            Some(i)
        } else {
            None
        }
    }

    /// Match phone number pattern
    fn match_phone(&self, input: &[u8]) -> Option<usize> {
        if input.len() < 7 {
            return None;
        }

        let mut digit_count = 0;
        let mut i = 0;

        // Allow leading + for international
        if input[0] == b'+' {
            i = 1;
        }

        while i < input.len() && i < 20 {
            if input[i].is_ascii_digit() {
                digit_count += 1;
            } else if input[i] == b'-' || input[i] == b' ' || input[i] == b'(' || input[i] == b')' {
                // Allow common phone separators
            } else {
                break;
            }
            i += 1;
        }

        // Phone numbers typically have 7-15 digits
        if digit_count >= 7 && digit_count <= 15 {
            Some(i)
        } else {
            None
        }
    }

    /// Match IP address pattern: X.X.X.X
    fn match_ip(&self, input: &[u8]) -> Option<usize> {
        if input.len() < 7 {
            return None;
        }

        let mut octets = 0;
        let mut current_digits = 0;
        let mut i = 0;

        while i < input.len() && octets < 4 {
            if input[i].is_ascii_digit() {
                current_digits += 1;
                if current_digits > 3 {
                    return None;
                }
            } else if input[i] == b'.' {
                if current_digits == 0 {
                    return None;
                }
                octets += 1;
                current_digits = 0;
            } else {
                break;
            }
            i += 1;
        }

        // Count the last octet
        if current_digits > 0 {
            octets += 1;
        }

        if octets == 4 {
            Some(i)
        } else {
            None
        }
    }

    /// Match API key patterns (sk-, pk-, api_, etc.)
    fn match_api_key(&self, input: &[u8]) -> Option<usize> {
        // Common API key prefixes
        const PREFIXES: &[&[u8]] = &[
            b"sk-", b"pk-", b"api_", b"key_", b"token_",
            b"secret_", b"AKIA", b"Bearer ",
        ];

        for prefix in PREFIXES {
            if input.len() >= prefix.len() && &input[..prefix.len()] == *prefix {
                // Find end of key (usually alphanumeric)
                let mut i = prefix.len();
                while i < input.len() && (input[i].is_ascii_alphanumeric() || input[i] == b'-' || input[i] == b'_') {
                    i += 1;
                }
                if i > prefix.len() + 8 {
                    return Some(i);
                }
            }
        }
        None
    }

    /// Match long number sequences (potential IDs)
    fn match_long_number(&self, input: &[u8]) -> Option<usize> {
        if !input[0].is_ascii_digit() {
            return None;
        }

        let mut i = 0;
        while i < input.len() && input[i].is_ascii_digit() {
            i += 1;
        }

        // Only redact sequences of 6+ digits
        if i >= 6 {
            Some(i)
        } else {
            None
        }
    }
}

/// Check if character is valid in email
fn is_email_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'.' || c == b'-' || c == b'_' || c == b'+'
}

/// Initialize the redaction subsystem
pub fn init() {
    super::context_builder::global_redactor().init();
}
