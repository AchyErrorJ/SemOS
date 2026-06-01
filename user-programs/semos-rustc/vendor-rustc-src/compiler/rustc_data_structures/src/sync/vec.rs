use alloc::vec::Vec;
use core::marker::PhantomData;

use rustc_index::Idx;

// Stage F1: elsa::sync::LockFreeFrozenVec / parking_lot::RwLock are
// host-only deps. On SemOS we substitute futex-backed semos_std
// primitives (Mutex<Vec<T>> for both append-only types — single-
// threaded per §1.4 so the perf hit is irrelevant).
#[cfg(not(target_os = "none"))]
#[derive(Default)]
pub struct AppendOnlyIndexVec<I: Idx, T: Copy> {
    vec: elsa::sync::LockFreeFrozenVec<T>,
    _marker: PhantomData<fn(&I)>,
}

#[cfg(target_os = "none")]
pub struct AppendOnlyIndexVec<I: Idx, T: Copy> {
    vec: semos_std::sync::Mutex<Vec<T>>,
    _marker: PhantomData<fn(&I)>,
}

#[cfg(target_os = "none")]
impl<I: Idx, T: Copy> Default for AppendOnlyIndexVec<I, T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_os = "none"))]
impl<I: Idx, T: Copy> AppendOnlyIndexVec<I, T> {
    pub fn new() -> Self {
        Self { vec: elsa::sync::LockFreeFrozenVec::new(), _marker: PhantomData }
    }

    pub fn push(&self, val: T) -> I {
        let i = self.vec.push(val);
        I::new(i)
    }

    pub fn get(&self, i: I) -> Option<T> {
        let i = i.index();
        self.vec.get(i)
    }
}

#[cfg(target_os = "none")]
impl<I: Idx, T: Copy> AppendOnlyIndexVec<I, T> {
    pub fn new() -> Self {
        Self { vec: semos_std::sync::Mutex::new(Vec::new()), _marker: PhantomData }
    }

    pub fn push(&self, val: T) -> I {
        let mut v = self.vec.lock();
        let i = v.len();
        v.push(val);
        I::new(i)
    }

    pub fn get(&self, i: I) -> Option<T> {
        let v = self.vec.lock();
        v.get(i.index()).copied()
    }
}

#[cfg(not(target_os = "none"))]
#[derive(Default)]
pub struct AppendOnlyVec<T: Copy> {
    vec: parking_lot::RwLock<Vec<T>>,
}

#[cfg(target_os = "none")]
pub struct AppendOnlyVec<T: Copy> {
    vec: semos_std::sync::RwLock<Vec<T>>,
}

#[cfg(target_os = "none")]
impl<T: Copy> Default for AppendOnlyVec<T> {
    fn default() -> Self { Self::new() }
}

impl<T: Copy> AppendOnlyVec<T> {
    #[cfg(not(target_os = "none"))]
    pub fn new() -> Self {
        Self { vec: Default::default() }
    }

    #[cfg(target_os = "none")]
    pub fn new() -> Self {
        Self { vec: semos_std::sync::RwLock::new(Vec::new()) }
    }

    pub fn push(&self, val: T) -> usize {
        let mut v = self.vec.write();
        let n = v.len();
        v.push(val);
        n
    }

    pub fn get(&self, i: usize) -> Option<T> {
        self.vec.read().get(i).copied()
    }

    pub fn iter_enumerated(&self) -> impl Iterator<Item = (usize, T)> {
        (0..)
            .map(|i| (i, self.get(i)))
            .take_while(|(_, o)| o.is_some())
            .filter_map(|(i, o)| Some((i, o?)))
    }

    pub fn iter(&self) -> impl Iterator<Item = T> {
        (0..).map(|i| self.get(i)).take_while(|o| o.is_some()).flatten()
    }
}

impl<T: Copy + PartialEq> AppendOnlyVec<T> {
    pub fn contains(&self, val: T) -> bool {
        self.iter_enumerated().any(|(_, v)| v == val)
    }
}

impl<A: Copy> FromIterator<A> for AppendOnlyVec<A> {
    fn from_iter<T: IntoIterator<Item = A>>(iter: T) -> Self {
        let this = Self::new();
        for val in iter {
            this.push(val);
        }
        this
    }
}
