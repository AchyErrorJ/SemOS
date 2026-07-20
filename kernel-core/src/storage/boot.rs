//! Boot-Time Unlock Flow
//!
//! Handles system startup with encrypted storage:
//! 1. Check for existing storage
//! 2. Prompt for passphrase
//! 3. Derive master key
//! 4. Derive pool keys
//! 5. Decrypt and restore pools
//!
//! # Boot Modes
//!
//! - **Fresh Boot**: No storage exists, create new encrypted storage
//! - **Unlock Boot**: Storage exists, unlock with passphrase
//! - **Recovery Boot**: Storage corrupted, option to reset

use super::{StorageHeader, StorageResult, StorageError};
use crate::crypto::{
    Salt,
    master_key::{unlock, lock, is_unlocked},
    key_hierarchy::KeyHierarchy,
};

/// Boot state
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BootState {
    /// Initial state, not yet checked
    Unknown,
    /// Fresh system, no storage found
    Fresh,
    /// Storage found, awaiting unlock
    Locked,
    /// Successfully unlocked
    Unlocked,
    /// Unlock failed (wrong passphrase)
    Failed,
    /// Storage corrupted
    Corrupted,
}

/// Boot context
pub struct BootContext {
    /// Current boot state
    pub state: BootState,
    /// Storage salt (if found)
    pub salt: Option<Salt>,
    /// Number of unlock attempts
    pub attempts: u8,
    /// Maximum unlock attempts
    pub max_attempts: u8,
}

impl BootContext {
    /// Create new boot context
    pub fn new() -> Self {
        Self {
            state: BootState::Unknown,
            salt: None,
            attempts: 0,
            max_attempts: 3,
        }
    }

    /// Check storage and determine boot mode
    pub fn check_storage(&mut self, storage: &[u8]) -> BootState {
        if storage.len() < StorageHeader::SIZE {
            self.state = BootState::Fresh;
            crate::platform::log("  [boot] No storage found, fresh boot\n");
            return self.state;
        }

        // Try to read header
        let header = unsafe {
            core::ptr::read(storage.as_ptr() as *const StorageHeader)
        };

        if !header.verify() {
            // Check if it's all zeros (fresh) or corrupted
            let is_empty = storage[..64].iter().all(|&b| b == 0);
            if is_empty {
                self.state = BootState::Fresh;
                crate::platform::log("  [boot] Empty storage, fresh boot\n");
            } else {
                self.state = BootState::Corrupted;
                crate::platform::log("  [boot] Storage corrupted!\n");
            }
            return self.state;
        }

        // Valid storage found
        self.salt = Some(header.get_salt());
        self.state = BootState::Locked;
        crate::platform::log("  [boot] Storage found, unlock required\n");
        self.state
    }

    /// Attempt to unlock with passphrase
    pub fn try_unlock(&mut self, passphrase: &str) -> StorageResult<()> {
        if self.state != BootState::Locked && self.state != BootState::Failed {
            return Err(StorageError::InvalidHeader);
        }

        self.attempts += 1;

        let salt = self.salt.ok_or(StorageError::InvalidHeader)?;

        // Try to derive master key
        match unlock(passphrase, &salt) {
            Ok(()) => {
                // Derive pool keys
                match KeyHierarchy::init() {
                    Ok(()) => {
                        self.state = BootState::Unlocked;
                        crate::platform::log("  [boot] System unlocked successfully\n");
                        Ok(())
                    }
                    Err(_e) => {
                        lock();
                        self.state = BootState::Failed;
                        crate::platform::log("  [boot] Key derivation failed\n");
                        Err(StorageError::KeyNotInitialized)
                    }
                }
            }
            Err(_) => {
                self.state = BootState::Failed;
                crate::platform::log("  [boot] Unlock failed (attempt ");
                crate::platform::log_num(self.attempts as u64);
                crate::platform::log("/");
                crate::platform::log_num(self.max_attempts as u64);
                crate::platform::log(")\n");

                if self.attempts >= self.max_attempts {
                    crate::platform::log("  [boot] Maximum attempts reached!\n");
                }

                Err(StorageError::DecryptionFailed)
            }
        }
    }

    /// Initialize fresh storage with new passphrase
    pub fn init_fresh(&mut self, passphrase: &str, seed: u64) -> StorageResult<Salt> {
        if self.state != BootState::Fresh {
            return Err(StorageError::InvalidHeader);
        }

        // Generate new salt
        let salt = Salt::generate(seed);
        self.salt = Some(salt);

        // Derive master key
        unlock(passphrase, &salt).map_err(|_| StorageError::KeyNotInitialized)?;

        // Derive pool keys
        KeyHierarchy::init().map_err(|_| StorageError::KeyNotInitialized)?;

        self.state = BootState::Unlocked;
        crate::platform::log("  [boot] Fresh storage initialized\n");

        Ok(salt)
    }

    /// Lock the system (clear keys)
    pub fn lock_system(&mut self) {
        KeyHierarchy::clear();
        lock();
        self.state = BootState::Locked;
        crate::platform::log("  [boot] System locked\n");
    }

    /// Check if system is unlocked
    pub fn is_unlocked(&self) -> bool {
        self.state == BootState::Unlocked && is_unlocked()
    }

    /// Get remaining unlock attempts
    pub fn remaining_attempts(&self) -> u8 {
        self.max_attempts.saturating_sub(self.attempts)
    }
}

/// Boot sequence states for state machine
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BootSequence {
    /// Initial hardware setup
    HardwareInit,
    /// Memory subsystem init
    MemoryInit,
    /// Check for existing storage
    StorageCheck,
    /// Prompt for passphrase
    PassphrasePrompt,
    /// Unlock storage
    Unlock,
    /// Load encrypted pools
    LoadPools,
    /// Start scheduler
    StartScheduler,
    /// Boot complete
    Complete,
    /// Error state
    Error,
}

/// Run the complete boot sequence
pub fn run_boot_sequence(storage: &[u8]) -> StorageResult<BootContext> {
    crate::platform::log("\n");
    crate::platform::log("╔══════════════════════════════════════════════════════════════════╗\n");
    crate::platform::log("║              SEMANTIC OS - SECURE BOOT SEQUENCE                  ║\n");
    crate::platform::log("╚══════════════════════════════════════════════════════════════════╝\n\n");

    let mut ctx = BootContext::new();

    // Step 1: Check storage
    crate::platform::log("[1/4] Checking storage...\n");
    let state = ctx.check_storage(storage);

    match state {
        BootState::Fresh => {
            crate::platform::log("      Status: Fresh installation\n");
            crate::platform::log("      Action: Will create new encrypted storage\n");
        }
        BootState::Locked => {
            crate::platform::log("      Status: Encrypted storage found\n");
            crate::platform::log("      Action: Passphrase required\n");
        }
        BootState::Corrupted => {
            crate::platform::log("      Status: CORRUPTED\n");
            crate::platform::log("      Action: Recovery required\n");
            return Err(StorageError::ChecksumFailed);
        }
        _ => {}
    }

    // Step 2: Key derivation status
    crate::platform::log("\n[2/4] Cryptographic subsystem...\n");
    crate::platform::log("      Algorithm: PBKDF2-SHA256 (100,000 iterations)\n");
    crate::platform::log("      Cipher: ChaCha20-Poly1305\n");
    crate::platform::log("      Key size: 256-bit\n");

    // Step 3: Pool status
    crate::platform::log("\n[3/4] Memory pools...\n");
    crate::platform::log("      Public:    Ready (unencrypted)\n");
    crate::platform::log("      Internal:  Ready (encrypted)\n");
    crate::platform::log("      Sensitive: Ready (encrypted)\n");
    crate::platform::log("      Secret:    Ready (encrypted)\n");

    // Step 4: Boot status
    crate::platform::log("\n[4/4] Boot status...\n");
    match state {
        BootState::Fresh => {
            crate::platform::log("      Mode: FIRST BOOT\n");
            crate::platform::log("      Set passphrase to complete setup\n");
        }
        BootState::Locked => {
            crate::platform::log("      Mode: LOCKED\n");
            crate::platform::log("      Enter passphrase to unlock\n");
        }
        _ => {}
    }

    crate::platform::log("\n");
    crate::platform::log("──────────────────────────────────────────────────────────────────\n");

    Ok(ctx)
}

/// Demo boot sequence (for testing without actual storage)
pub fn demo_boot(passphrase: &str) -> StorageResult<()> {
    crate::platform::log("\n[*] Demo Boot Sequence\n");
    crate::platform::log("========================\n\n");

    // Simulate fresh boot
    let mut ctx = BootContext::new();
    ctx.state = BootState::Fresh;

    // Initialize with demo passphrase
    let seed = 0x12345678_87654321u64; // Demo seed
    let salt = ctx.init_fresh(passphrase, seed)?;

    crate::platform::log("  Salt: ");
    for b in salt.as_bytes() {
        crate::platform::log_hex_byte(*b);
    }
    crate::platform::log("\n");

    // Verify key hierarchy is initialized
    if KeyHierarchy::is_initialized() {
        crate::platform::log("  Pool keys: DERIVED\n");
    } else {
        crate::platform::log("  Pool keys: NOT INITIALIZED\n");
        return Err(StorageError::KeyNotInitialized);
    }

    // Print key hierarchy status
    crate::crypto::key_hierarchy::print_status();

    crate::platform::log("\n[*] Demo boot complete\n");
    Ok(())
}

