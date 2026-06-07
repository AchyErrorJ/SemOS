// M27 Phase 5c Stage G iter 6: no_std float ops shim.
//
// f32/f64 methods like sqrt/ceil/floor/trunc/round_ties_even/powi live in
// `std::f32/f64` (they call into libm or platform intrinsics). On
// x86_64-unknown-none these methods don't exist on the primitive types.
// This module defines a `FloatNoStd` trait that backs each method onto
// the `libm` crate so the macros in src/ir/immediates.rs and the
// constant-folding helpers in isa/s390x + isa/riscv64 keep working.

#![allow(dead_code)]

pub trait FloatNoStd: Copy {
    fn sqrt(self) -> Self;
    fn ceil(self) -> Self;
    fn floor(self) -> Self;
    fn trunc(self) -> Self;
    fn round_ties_even(self) -> Self;
    fn powi(self, n: i32) -> Self;
}

impl FloatNoStd for f32 {
    #[inline]
    fn sqrt(self) -> Self { libm::sqrtf(self) }
    #[inline]
    fn ceil(self) -> Self { libm::ceilf(self) }
    #[inline]
    fn floor(self) -> Self { libm::floorf(self) }
    #[inline]
    fn trunc(self) -> Self { libm::truncf(self) }
    #[inline]
    fn round_ties_even(self) -> Self { libm::roundevenf(self) }
    #[inline]
    fn powi(self, n: i32) -> Self { libm::powf(self, n as f32) }
}

impl FloatNoStd for f64 {
    #[inline]
    fn sqrt(self) -> Self { libm::sqrt(self) }
    #[inline]
    fn ceil(self) -> Self { libm::ceil(self) }
    #[inline]
    fn floor(self) -> Self { libm::floor(self) }
    #[inline]
    fn trunc(self) -> Self { libm::trunc(self) }
    #[inline]
    fn round_ties_even(self) -> Self { libm::roundeven(self) }
    #[inline]
    fn powi(self, n: i32) -> Self { libm::pow(self, n as f64) }
}
