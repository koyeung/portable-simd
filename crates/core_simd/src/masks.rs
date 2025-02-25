//! Types and traits associated with masking elements of vectors.
//! Types representing
#![allow(non_camel_case_types)]

#[cfg_attr(
    not(all(target_arch = "x86_64", target_feature = "avx512f")),
    path = "masks/full_masks.rs"
)]
#[cfg_attr(
    all(target_arch = "x86_64", target_feature = "avx512f"),
    path = "masks/bitmask.rs"
)]
mod mask_impl;

mod to_bitmask;
pub use to_bitmask::{ToBitMask, ToBitMaskArray};

use crate::simd::{
    cmp::SimdPartialEq, intrinsics, LaneCount, Simd, SimdElement, SupportedLaneCount,
};
use core::cmp::Ordering;
use core::{fmt, mem};

mod sealed {
    use super::*;

    /// Not only does this seal the `MaskElement` trait, but these functions prevent other traits
    /// from bleeding into the parent bounds.
    ///
    /// For example, `eq` could be provided by requiring `MaskElement: PartialEq`, but that would
    /// prevent us from ever removing that bound, or from implementing `MaskElement` on
    /// non-`PartialEq` types in the future.
    pub trait Sealed {
        fn valid<const N: usize>(values: Simd<Self, N>) -> bool
        where
            LaneCount<N>: SupportedLaneCount,
            Self: SimdElement;

        fn eq(self, other: Self) -> bool;

        const TRUE: Self;

        const FALSE: Self;
    }
}
use sealed::Sealed;

/// Marker trait for types that may be used as SIMD mask elements.
///
/// # Safety
/// Type must be a signed integer.
pub unsafe trait MaskElement: SimdElement + Sealed {}

macro_rules! impl_element {
    { $ty:ty } => {
        impl Sealed for $ty {
            #[inline]
            fn valid<const N: usize>(value: Simd<Self, N>) -> bool
            where
                LaneCount<N>: SupportedLaneCount,
            {
                (value.simd_eq(Simd::splat(0 as _)) | value.simd_eq(Simd::splat(-1 as _))).all()
            }

            #[inline]
            fn eq(self, other: Self) -> bool { self == other }

            const TRUE: Self = -1;
            const FALSE: Self = 0;
        }

        // Safety: this is a valid mask element type
        unsafe impl MaskElement for $ty {}
    }
}

impl_element! { i8 }
impl_element! { i16 }
impl_element! { i32 }
impl_element! { i64 }
impl_element! { isize }

/// A SIMD vector mask for `N` elements of width specified by `Element`.
///
/// Masks represent boolean inclusion/exclusion on a per-element basis.
///
/// The layout of this type is unspecified, and may change between platforms
/// and/or Rust versions, and code should not assume that it is equivalent to
/// `[T; N]`.
#[repr(transparent)]
pub struct Mask<T, const N: usize>(mask_impl::Mask<T, N>)
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount;

impl<T, const N: usize> Copy for Mask<T, N>
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
}

impl<T, const N: usize> Clone for Mask<T, N>
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<T, const N: usize> Mask<T, N>
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    /// Construct a mask by setting all elements to the given value.
    #[inline]
    pub fn splat(value: bool) -> Self {
        Self(mask_impl::Mask::splat(value))
    }

    /// Converts an array of bools to a SIMD mask.
    #[inline]
    pub fn from_array(array: [bool; N]) -> Self {
        // SAFETY: Rust's bool has a layout of 1 byte (u8) with a value of
        //     true:    0b_0000_0001
        //     false:   0b_0000_0000
        // Thus, an array of bools is also a valid array of bytes: [u8; N]
        // This would be hypothetically valid as an "in-place" transmute,
        // but these are "dependently-sized" types, so copy elision it is!
        unsafe {
            let bytes: [u8; N] = mem::transmute_copy(&array);
            let bools: Simd<i8, N> = intrinsics::simd_ne(Simd::from_array(bytes), Simd::splat(0u8));
            Mask::from_int_unchecked(intrinsics::simd_cast(bools))
        }
    }

    /// Converts a SIMD mask to an array of bools.
    #[inline]
    pub fn to_array(self) -> [bool; N] {
        // This follows mostly the same logic as from_array.
        // SAFETY: Rust's bool has a layout of 1 byte (u8) with a value of
        //     true:    0b_0000_0001
        //     false:   0b_0000_0000
        // Thus, an array of bools is also a valid array of bytes: [u8; N]
        // Since our masks are equal to integers where all bits are set,
        // we can simply convert them to i8s, and then bitand them by the
        // bitpattern for Rust's "true" bool.
        // This would be hypothetically valid as an "in-place" transmute,
        // but these are "dependently-sized" types, so copy elision it is!
        unsafe {
            let mut bytes: Simd<i8, N> = intrinsics::simd_cast(self.to_int());
            bytes &= Simd::splat(1i8);
            mem::transmute_copy(&bytes)
        }
    }

    /// Converts a vector of integers to a mask, where 0 represents `false` and -1
    /// represents `true`.
    ///
    /// # Safety
    /// All elements must be either 0 or -1.
    #[inline]
    #[must_use = "method returns a new mask and does not mutate the original value"]
    pub unsafe fn from_int_unchecked(value: Simd<T, N>) -> Self {
        // Safety: the caller must confirm this invariant
        unsafe { Self(mask_impl::Mask::from_int_unchecked(value)) }
    }

    /// Converts a vector of integers to a mask, where 0 represents `false` and -1
    /// represents `true`.
    ///
    /// # Panics
    /// Panics if any element is not 0 or -1.
    #[inline]
    #[must_use = "method returns a new mask and does not mutate the original value"]
    #[track_caller]
    pub fn from_int(value: Simd<T, N>) -> Self {
        assert!(T::valid(value), "all values must be either 0 or -1",);
        // Safety: the validity has been checked
        unsafe { Self::from_int_unchecked(value) }
    }

    /// Converts the mask to a vector of integers, where 0 represents `false` and -1
    /// represents `true`.
    #[inline]
    #[must_use = "method returns a new vector and does not mutate the original value"]
    pub fn to_int(self) -> Simd<T, N> {
        self.0.to_int()
    }

    /// Converts the mask to a mask of any other element size.
    #[inline]
    #[must_use = "method returns a new mask and does not mutate the original value"]
    pub fn cast<U: MaskElement>(self) -> Mask<U, N> {
        Mask(self.0.convert())
    }

    /// Tests the value of the specified element.
    ///
    /// # Safety
    /// `index` must be less than `self.len()`.
    #[inline]
    #[must_use = "method returns a new bool and does not mutate the original value"]
    pub unsafe fn test_unchecked(&self, index: usize) -> bool {
        // Safety: the caller must confirm this invariant
        unsafe { self.0.test_unchecked(index) }
    }

    /// Tests the value of the specified element.
    ///
    /// # Panics
    /// Panics if `index` is greater than or equal to the number of elements in the vector.
    #[inline]
    #[must_use = "method returns a new bool and does not mutate the original value"]
    #[track_caller]
    pub fn test(&self, index: usize) -> bool {
        assert!(index < N, "element index out of range");
        // Safety: the element index has been checked
        unsafe { self.test_unchecked(index) }
    }

    /// Sets the value of the specified element.
    ///
    /// # Safety
    /// `index` must be less than `self.len()`.
    #[inline]
    pub unsafe fn set_unchecked(&mut self, index: usize, value: bool) {
        // Safety: the caller must confirm this invariant
        unsafe {
            self.0.set_unchecked(index, value);
        }
    }

    /// Sets the value of the specified element.
    ///
    /// # Panics
    /// Panics if `index` is greater than or equal to the number of elements in the vector.
    #[inline]
    #[track_caller]
    pub fn set(&mut self, index: usize, value: bool) {
        assert!(index < N, "element index out of range");
        // Safety: the element index has been checked
        unsafe {
            self.set_unchecked(index, value);
        }
    }

    /// Returns true if any element is set, or false otherwise.
    #[inline]
    #[must_use = "method returns a new bool and does not mutate the original value"]
    pub fn any(self) -> bool {
        self.0.any()
    }

    /// Returns true if all elements are set, or false otherwise.
    #[inline]
    #[must_use = "method returns a new bool and does not mutate the original value"]
    pub fn all(self) -> bool {
        self.0.all()
    }
}

// vector/array conversion
impl<T, const N: usize> From<[bool; N]> for Mask<T, N>
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    #[inline]
    fn from(array: [bool; N]) -> Self {
        Self::from_array(array)
    }
}

impl<T, const N: usize> From<Mask<T, N>> for [bool; N]
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    #[inline]
    fn from(vector: Mask<T, N>) -> Self {
        vector.to_array()
    }
}

impl<T, const N: usize> Default for Mask<T, N>
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    #[inline]
    #[must_use = "method returns a defaulted mask with all elements set to false (0)"]
    fn default() -> Self {
        Self::splat(false)
    }
}

impl<T, const N: usize> PartialEq for Mask<T, N>
where
    T: MaskElement + PartialEq,
    LaneCount<N>: SupportedLaneCount,
{
    #[inline]
    #[must_use = "method returns a new bool and does not mutate the original value"]
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T, const N: usize> PartialOrd for Mask<T, N>
where
    T: MaskElement + PartialOrd,
    LaneCount<N>: SupportedLaneCount,
{
    #[inline]
    #[must_use = "method returns a new Ordering and does not mutate the original value"]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.0.partial_cmp(&other.0)
    }
}

impl<T, const N: usize> fmt::Debug for Mask<T, N>
where
    T: MaskElement + fmt::Debug,
    LaneCount<N>: SupportedLaneCount,
{
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries((0..N).map(|i| self.test(i)))
            .finish()
    }
}

impl<T, const N: usize> core::ops::BitAnd for Mask<T, N>
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    type Output = Self;
    #[inline]
    #[must_use = "method returns a new mask and does not mutate the original value"]
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl<T, const N: usize> core::ops::BitAnd<bool> for Mask<T, N>
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    type Output = Self;
    #[inline]
    #[must_use = "method returns a new mask and does not mutate the original value"]
    fn bitand(self, rhs: bool) -> Self {
        self & Self::splat(rhs)
    }
}

impl<T, const N: usize> core::ops::BitAnd<Mask<T, N>> for bool
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    type Output = Mask<T, N>;
    #[inline]
    #[must_use = "method returns a new mask and does not mutate the original value"]
    fn bitand(self, rhs: Mask<T, N>) -> Mask<T, N> {
        Mask::splat(self) & rhs
    }
}

impl<T, const N: usize> core::ops::BitOr for Mask<T, N>
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    type Output = Self;
    #[inline]
    #[must_use = "method returns a new mask and does not mutate the original value"]
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl<T, const N: usize> core::ops::BitOr<bool> for Mask<T, N>
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    type Output = Self;
    #[inline]
    #[must_use = "method returns a new mask and does not mutate the original value"]
    fn bitor(self, rhs: bool) -> Self {
        self | Self::splat(rhs)
    }
}

impl<T, const N: usize> core::ops::BitOr<Mask<T, N>> for bool
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    type Output = Mask<T, N>;
    #[inline]
    #[must_use = "method returns a new mask and does not mutate the original value"]
    fn bitor(self, rhs: Mask<T, N>) -> Mask<T, N> {
        Mask::splat(self) | rhs
    }
}

impl<T, const N: usize> core::ops::BitXor for Mask<T, N>
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    type Output = Self;
    #[inline]
    #[must_use = "method returns a new mask and does not mutate the original value"]
    fn bitxor(self, rhs: Self) -> Self::Output {
        Self(self.0 ^ rhs.0)
    }
}

impl<T, const N: usize> core::ops::BitXor<bool> for Mask<T, N>
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    type Output = Self;
    #[inline]
    #[must_use = "method returns a new mask and does not mutate the original value"]
    fn bitxor(self, rhs: bool) -> Self::Output {
        self ^ Self::splat(rhs)
    }
}

impl<T, const N: usize> core::ops::BitXor<Mask<T, N>> for bool
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    type Output = Mask<T, N>;
    #[inline]
    #[must_use = "method returns a new mask and does not mutate the original value"]
    fn bitxor(self, rhs: Mask<T, N>) -> Self::Output {
        Mask::splat(self) ^ rhs
    }
}

impl<T, const N: usize> core::ops::Not for Mask<T, N>
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    type Output = Mask<T, N>;
    #[inline]
    #[must_use = "method returns a new mask and does not mutate the original value"]
    fn not(self) -> Self::Output {
        Self(!self.0)
    }
}

impl<T, const N: usize> core::ops::BitAndAssign for Mask<T, N>
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    #[inline]
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 = self.0 & rhs.0;
    }
}

impl<T, const N: usize> core::ops::BitAndAssign<bool> for Mask<T, N>
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    #[inline]
    fn bitand_assign(&mut self, rhs: bool) {
        *self &= Self::splat(rhs);
    }
}

impl<T, const N: usize> core::ops::BitOrAssign for Mask<T, N>
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 = self.0 | rhs.0;
    }
}

impl<T, const N: usize> core::ops::BitOrAssign<bool> for Mask<T, N>
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    #[inline]
    fn bitor_assign(&mut self, rhs: bool) {
        *self |= Self::splat(rhs);
    }
}

impl<T, const N: usize> core::ops::BitXorAssign for Mask<T, N>
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    #[inline]
    fn bitxor_assign(&mut self, rhs: Self) {
        self.0 = self.0 ^ rhs.0;
    }
}

impl<T, const N: usize> core::ops::BitXorAssign<bool> for Mask<T, N>
where
    T: MaskElement,
    LaneCount<N>: SupportedLaneCount,
{
    #[inline]
    fn bitxor_assign(&mut self, rhs: bool) {
        *self ^= Self::splat(rhs);
    }
}

macro_rules! impl_from {
    { $from:ty  => $($to:ty),* } => {
        $(
        impl<const N: usize> From<Mask<$from, N>> for Mask<$to, N>
        where
            LaneCount<N>: SupportedLaneCount,
        {
            #[inline]
            fn from(value: Mask<$from, N>) -> Self {
                value.cast()
            }
        }
        )*
    }
}
impl_from! { i8 => i16, i32, i64, isize }
impl_from! { i16 => i32, i64, isize, i8 }
impl_from! { i32 => i64, isize, i8, i16 }
impl_from! { i64 => isize, i8, i16, i32 }
impl_from! { isize => i8, i16, i32, i64 }
