//! User Account Registry
//!
//! Authoritative table of who exists in the system. Before this module the
//! kernel was treating a task's scheduler-slot index as if it were a user id,
//! which is wrong on three axes: slots get recycled, a single user can run
//! many concurrent tasks, and "ADMIN" / "GUEST" weren't real entities — just
//! magic numbers from `security::user_ids`.
//!
//! # Model
//!
//! - A [`UserAccount`] is a fixed-size record (no heap), keyed by [`UserId`].
//! - A [`UserRegistry`] holds up to [`MAX_USERS`] accounts in a flat array.
//! - System-reserved ids (`SYSTEM=0`, `ADMIN=1`, `GUEST=254`, `NOBODY=255`)
//!   are pre-populated at [`init`] and cannot be deleted.
//! - User-created accounts get ids in `[10, 250)`. Group ids work the same
//!   way but are not separately enforced today.
//!
//! # What this module is *not*
//!
//! Authentication. There's a `password_hash` field reserved so a future
//! login flow can fill it in (likely PBKDF2 against `crypto::pbkdf2`), but
//! credential verification is out of scope for Task #8. The point right now
//! is to give every running task an honest answer to "who am I acting for".

use crate::memory::SecurityTier;
use super::{UserId, SecurityError};

/// Maximum number of accounts the registry can hold (system + user slots).
pub const MAX_USERS: usize = 32;

/// Max length of a user name in bytes.
pub const MAX_USERNAME_LEN: usize = 31;

/// Reserved bottom range — system-managed user ids. Cannot be re-created
/// or repurposed by [`create_user`].
pub const FIRST_RESERVED_LOW: UserId = 0;
pub const LAST_RESERVED_LOW: UserId = 9;

/// First id `create_user` will hand out.
pub const FIRST_DYNAMIC_USER: UserId = 10;

/// One past the last id `create_user` will hand out. Above this is the
/// reserved top range (`GUEST=254`, `NOBODY=255`).
pub const FIRST_RESERVED_HIGH: UserId = 250;

/// Group id type — same width as `UserId` for symmetry.
pub type GroupId = u8;

pub mod groups {
    use super::GroupId;
    pub const SYSTEM: GroupId = 0;
    pub const ADMIN: GroupId = 1;
    pub const USERS: GroupId = 10;
    pub const GUEST: GroupId = 254;
    pub const NONE: GroupId = 255;
}

/// Per-account flag bits.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct UserFlags(pub u16);

impl UserFlags {
    /// Slot is in use.
    pub const ACTIVE: u16 = 1 << 0;
    /// Built-in account (cannot be deleted / cannot be re-created).
    pub const SYSTEM: u16 = 1 << 1;
    /// Account is locked (cannot setuid into it; reserved for future
    /// authentication failures).
    pub const LOCKED: u16 = 1 << 2;

    pub const fn empty() -> Self { Self(0) }
    pub const fn active() -> Self { Self(Self::ACTIVE) }

    pub fn is_active(&self) -> bool { self.0 & Self::ACTIVE != 0 }
    pub fn is_system(&self) -> bool { self.0 & Self::SYSTEM != 0 }
    pub fn is_locked(&self) -> bool { self.0 & Self::LOCKED != 0 }

    pub fn set(&mut self, mask: u16) { self.0 |= mask; }
    pub fn clear(&mut self, mask: u16) { self.0 &= !mask; }
}

/// A user account. Copy + fixed-size so it lives directly in the registry
/// array, no heap, no lifetimes.
#[derive(Clone, Copy)]
pub struct UserAccount {
    pub id: UserId,
    pub group: GroupId,
    /// Highest [`SecurityTier`] this user is allowed to access by default.
    /// A task started under this user inherits this as its `max_tier`.
    pub default_max_tier: SecurityTier,
    /// Reserved for a future authentication module. Stored as raw bytes so
    /// the layout doesn't change when we wire up real hashing.
    pub password_hash: [u8; 32],
    /// Name buffer + length (UTF-8 / ASCII).
    name: [u8; MAX_USERNAME_LEN],
    name_len: u8,
    pub flags: UserFlags,
}

impl UserAccount {
    pub const fn empty() -> Self {
        Self {
            id: super::user_ids::NOBODY,
            group: groups::NONE,
            default_max_tier: SecurityTier::Public,
            password_hash: [0u8; 32],
            name: [0u8; MAX_USERNAME_LEN],
            name_len: 0,
            flags: UserFlags::empty(),
        }
    }

    fn new(id: UserId, name: &str, group: GroupId, tier: SecurityTier, system: bool) -> Self {
        let mut acc = Self::empty();
        acc.id = id;
        acc.group = group;
        acc.default_max_tier = tier;
        let bytes = name.as_bytes();
        let n = bytes.len().min(MAX_USERNAME_LEN);
        acc.name[..n].copy_from_slice(&bytes[..n]);
        acc.name_len = n as u8;
        acc.flags = UserFlags::active();
        if system { acc.flags.set(UserFlags::SYSTEM); }
        acc
    }

    pub fn name(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }
}

/// Flat registry. Slot 0 always holds SYSTEM.
pub struct UserRegistry {
    accounts: [UserAccount; MAX_USERS],
    initialized: bool,
}

impl UserRegistry {
    pub const fn new() -> Self {
        Self {
            accounts: [UserAccount::empty(); MAX_USERS],
            initialized: false,
        }
    }

    /// Populate the built-in accounts. Idempotent.
    pub fn init(&mut self) {
        if self.initialized { return; }

        // Slot layout: reserved-low ids first, then dynamic ids appended.
        self.accounts[0] = UserAccount::new(
            super::user_ids::SYSTEM, "system",
            groups::SYSTEM, SecurityTier::Secret, true);
        self.accounts[1] = UserAccount::new(
            super::user_ids::ADMIN, "admin",
            groups::ADMIN, SecurityTier::Secret, true);
        self.accounts[2] = UserAccount::new(
            super::user_ids::GUEST, "guest",
            groups::GUEST, SecurityTier::Public, true);
        self.accounts[3] = UserAccount::new(
            super::user_ids::NOBODY, "nobody",
            groups::NONE, SecurityTier::Public, true);

        self.initialized = true;
    }

    /// Look up an account by id. Returns the slot, not a clone, so the
    /// caller can mutate it through `lookup_mut` when needed.
    pub fn lookup(&self, id: UserId) -> Option<&UserAccount> {
        self.accounts.iter().find(|a| a.flags.is_active() && a.id == id)
    }

    pub fn lookup_mut(&mut self, id: UserId) -> Option<&mut UserAccount> {
        self.accounts.iter_mut().find(|a| a.flags.is_active() && a.id == id)
    }

    /// Look up a user by name. Linear scan over the bounded array.
    pub fn lookup_by_name(&self, name: &[u8]) -> Option<&UserAccount> {
        self.accounts.iter().find(|a| a.flags.is_active() && a.name() == name)
    }

    /// Create a new dynamic user. Returns the assigned id.
    ///
    /// Restrictions:
    /// - Requester must be in the system or admin group (`creator_is_admin`).
    /// - Name must be non-empty and ≤ [`MAX_USERNAME_LEN`].
    /// - `default_max_tier` cannot exceed the creator's own tier (the
    ///   caller enforces this — the registry doesn't know who's asking).
    pub fn create_user(
        &mut self,
        name: &str,
        group: GroupId,
        default_max_tier: SecurityTier,
    ) -> Result<UserId, SecurityError> {
        let bytes = name.as_bytes();
        if bytes.is_empty() || bytes.len() > MAX_USERNAME_LEN {
            return Err(SecurityError::InvalidPolicy);
        }
        // Reject duplicate names.
        if self.lookup_by_name(bytes).is_some() {
            return Err(SecurityError::InvalidPolicy);
        }

        // Find a free dynamic id. Walk the ranges and pick the first
        // unused one — there are only a couple hundred to consider.
        let mut candidate: UserId = FIRST_DYNAMIC_USER;
        while candidate < FIRST_RESERVED_HIGH {
            if self.lookup(candidate).is_none() {
                break;
            }
            candidate = match candidate.checked_add(1) {
                Some(v) => v,
                None => return Err(SecurityError::InvalidPolicy),
            };
        }
        if candidate >= FIRST_RESERVED_HIGH {
            return Err(SecurityError::InvalidPolicy); // table full
        }

        // Find a free registry slot.
        let slot = self.accounts.iter().position(|a| !a.flags.is_active())
            .ok_or(SecurityError::InvalidPolicy)?;

        self.accounts[slot] = UserAccount::new(
            candidate, name, group, default_max_tier, false);
        Ok(candidate)
    }

    /// Remove a dynamic user. System accounts cannot be removed.
    pub fn delete_user(&mut self, id: UserId) -> Result<(), SecurityError> {
        let slot = self.accounts.iter_mut()
            .find(|a| a.flags.is_active() && a.id == id)
            .ok_or(SecurityError::PolicyNotFound)?;
        if slot.flags.is_system() {
            return Err(SecurityError::InsufficientPrivilege);
        }
        *slot = UserAccount::empty();
        Ok(())
    }

    /// Number of active accounts in the registry. Cheap O(n) scan; only
    /// called from stats / demos.
    pub fn count(&self) -> usize {
        self.accounts.iter().filter(|a| a.flags.is_active()).count()
    }

    /// Iterate over active accounts.
    pub fn iter(&self) -> impl Iterator<Item = &UserAccount> {
        self.accounts.iter().filter(|a| a.flags.is_active())
    }
}

/// Global instance, behind the yield-on-contention kernel mutex (the old
/// "single-threaded kernel" contract was false for interrupts-enabled
/// syscall handlers — 2026-07-17 review, P1).
static GLOBAL_REGISTRY: crate::sync::Mutex<UserRegistry> =
    crate::sync::Mutex::new(UserRegistry::new());

/// Lock the global user registry.
pub fn global_user_registry() -> crate::sync::MutexGuard<'static, UserRegistry> {
    GLOBAL_REGISTRY.lock()
}

/// Initialise the registry. Idempotent.
pub fn init() {
    GLOBAL_REGISTRY.lock().init();
}

/// Convenience: is `id` allowed to perform privileged operations such as
/// creating users or writing system policies?
pub fn is_privileged(id: UserId) -> bool {
    id == super::user_ids::SYSTEM || id == super::user_ids::ADMIN
}

/// Convenience: can `requester` switch the effective user to `target`?
/// Rule: only system can become anyone; admin can drop into any user except
/// system; ordinary users cannot setuid.
pub fn can_setuid_to(requester: UserId, target: UserId, registry: &UserRegistry) -> bool {
    if registry.lookup(target).map(|a| a.flags.is_locked()).unwrap_or(true) {
        // Target either doesn't exist or is locked — refuse.
        return false;
    }
    match requester {
        id if id == super::user_ids::SYSTEM => true,
        id if id == super::user_ids::ADMIN => target != super::user_ids::SYSTEM,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_populates_builtins() {
        let mut r = UserRegistry::new();
        r.init();
        assert!(r.lookup(super::super::user_ids::SYSTEM).is_some());
        assert!(r.lookup(super::super::user_ids::ADMIN).is_some());
        assert!(r.lookup(super::super::user_ids::GUEST).is_some());
        assert!(r.lookup(super::super::user_ids::NOBODY).is_some());
    }

    #[test]
    fn create_and_delete_dynamic_user() {
        let mut r = UserRegistry::new();
        r.init();
        let id = r.create_user("alice", groups::USERS, SecurityTier::Internal).unwrap();
        assert!(id >= FIRST_DYNAMIC_USER);
        assert_eq!(r.lookup(id).unwrap().name(), b"alice");
        // Duplicate name rejected.
        assert!(r.create_user("alice", groups::USERS, SecurityTier::Internal).is_err());
        // Delete succeeds; system accounts cannot be deleted.
        assert!(r.delete_user(id).is_ok());
        assert!(r.delete_user(super::super::user_ids::ADMIN).is_err());
    }

    #[test]
    fn setuid_policy() {
        let mut r = UserRegistry::new();
        r.init();
        let alice = r.create_user("alice", groups::USERS, SecurityTier::Internal).unwrap();
        // Admin can drop to alice but not to system.
        assert!(can_setuid_to(super::super::user_ids::ADMIN, alice, &r));
        assert!(!can_setuid_to(super::super::user_ids::ADMIN, super::super::user_ids::SYSTEM, &r));
        // Alice cannot setuid at all.
        assert!(!can_setuid_to(alice, super::super::user_ids::ADMIN, &r));
    }
}
