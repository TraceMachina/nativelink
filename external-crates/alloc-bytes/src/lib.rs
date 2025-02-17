// Copyright 2025 The NativeLink Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![no_std]
//! # `alloc_bytes`
//!
//! `alloc_bytes` is a crate that provides custom byte buffers allocated using a
//! user-provided allocator. It offers both mutable and immutable byte buffers,
//! enabling efficient memory allocation strategies in systems with custom
//! memory requirements.
//!
//! ## Features
//!
//! - **Custom Allocator Support:** Allocate bytes via any allocator
//!   implementing [`GlobalAlloc`].
//! - **Mutable & Immutable Buffers:** Build data with [`AllocBytesMut`] and
//!   freeze it into an immutable [`AllocBytes`].
//! - **Interoperability with the `bytes` Crate:** Convert to [`Bytes`]
//!   when the `bytes` feature is enabled.
//!
//! ## Usage Example
//!
//! This example demonstrates how to use the fixed heap allocator to repeatedly
//! allocate, write to, and freeze byte buffers from a fixed memory slab. It
//! also verifies that each allocated buffer's memory lies within the provided
//! slab.
//!
//! ```rust
//! use core::mem::MaybeUninit;
//! use alloc_bytes::{AllocBytes, AllocBytesMut};
//! use heap_allocator::FixedHeapAllocator;
//!
//! const SLAB_SIZE: usize = 1024; // 1 KiB.
//! let mut raw_slab = [MaybeUninit::uninit(); SLAB_SIZE];
//! let slab_range = raw_slab.as_ptr_range();
//! let allocator = FixedHeapAllocator::new(&mut raw_slab);
//!
//! // Loop to allocate, write, and free buffers from the slab.
//! for _ in 0..1024 {
//!    // Create a new mutable byte buffer from the allocator.
//!    let mut buffer = AllocBytesMut::new_unaligned(&allocator);
//!    // Write data into the buffer.
//!    buffer.extend_from_slice(b"Hello, world!").unwrap();
//!    // Freeze the mutable buffer into an immutable byte buffer.
//!    let data: AllocBytes<_, _> = buffer.freeze();
//!
//!    // Validate the buffer length and contents.
//!    assert_eq!(data.len(), 13);
//!    assert_eq!(data.as_ref(), b"Hello, world!");
//!
//!    // Assert that the allocated data pointer lies within the raw slab.
//!    assert!(
//!        slab_range.contains(&(data.as_ref().as_ptr() as *const _)),
//!        "data pointer not in raw_slab"
//!    );
//!
//!    // Optionally, if you have the `bytes` feature enabled, you
//!    // could convert dthe buffer:
//!    // let bytes_data = buffer.into_bytes();
//! }
//! ```
//!
//! ### Converting to [`Bytes`]
//!
//! When the `bytes` feature is enabled, you can convert an [`AllocBytesMut`]
//! into a [`Bytes`] object.
//!
//! Note: We use [`heap_allocator::OwnedFixedHeapAllocator`] instead of
//! [`heap_allocator::FixedHeapAllocator`] to mask the lifetime our allocator's
//! data (which is 'static in our case). This is often needed when complex
//! lifetimes are involved but data is owned anyway.
//!
//! ```rust
//! use std::sync::Arc;
//! use alloc_bytes::AllocBytesMut;
//! use heap_allocator::OwnedFixedHeapAllocator;
//! use bytes::Bytes;
//! use core::mem::MaybeUninit;
//!
//! const SLAB_SIZE: usize = 1024; // 1 KiB.
//! let mut raw_slab = Vec::with_capacity(SLAB_SIZE);
//! raw_slab.resize(SLAB_SIZE, MaybeUninit::uninit());
//! let allocator = Arc::new(OwnedFixedHeapAllocator::new(raw_slab));
//!
//! // Create a mutable byte buffer and write data.
//! let mut buffer = AllocBytesMut::new_unaligned(allocator.clone());
//! buffer.extend_from_slice(b"Hello Bytes!").unwrap();
//!
//! // Convert the mutable buffer into an immutable `Bytes` object.
//! let bytes_data: Bytes = buffer.into_bytes();
//! assert_eq!(bytes_data.as_ref(), b"Hello Bytes!");
//! ```
//!
//! ## Related Crates and Resources
//!
//! - **[bytes](https://docs.rs/bytes):** Utilities for working with byte
//!   buffers.
//! - **[heap_allocator](https://crates.io/crates/heap_allocator):** An example
//!   custom heap allocator.
//!
//! This crate is `no_std` compatible.
use core::alloc::{GlobalAlloc, Layout};
use core::ops::Deref;
use core::ptr::NonNull;

#[cfg(any(doc, feature = "bytes"))]
use bytes::Bytes;

/// Errors that can occur during allocation or reallocation.
#[derive(Debug)]
pub enum AllocBytesError {
    /// Returned when allocation or reallocation fails.
    FailedToRealloc,
    /// Returned when the requested capacity addition would overflow.
    ReserveTooLarge,
    /// Returned when the requested layout is invalid.
    Layout(core::alloc::LayoutError),
}

/// An immutable byte buffer allocated via a custom allocator.
///
/// This type holds a raw pointer to memory allocated using the provided
/// allocator. It is not [`Sync`] because the inner pointer is mutated in
/// controlled ways. If you need to share the buffer across threads concurrently
/// (i.e. require [`Sync`]), consider converting it into a [`Bytes`]
/// object via [`AllocBytesMut::into_bytes()`] or wrapping it in an `Arc`.
///
/// # Type Parameters
/// - `T`: A type that can be converted to a reference to the allocator.
/// - `A`: The allocator type that implements [`GlobalAlloc`].
/// - `ALIGN`: The alignment to use for allocations (default is 1).
pub struct AllocBytes<T, A, const ALIGN: usize = 1>
where
    T: Deref<Target = A>,
    A: GlobalAlloc + ?Sized,
{
    heap: T,
    ptr: NonNull<u8>,
    size: usize,
    cap: usize,
    _phantom: core::marker::PhantomData<A>,
}

impl<T, A, const ALIGN: usize> AllocBytes<T, A, ALIGN>
where
    T: Deref<Target = A>,
    A: GlobalAlloc + ?Sized,
{
    /// Returns the number of bytes in the buffer.
    #[inline]
    pub const fn len(&self) -> usize {
        self.size
    }

    /// Returns if the buffer is empty.
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the capacity of the buffer in bytes.
    #[inline]
    pub const fn cap(&self) -> usize {
        self.cap
    }

    /// Returns a reference to the allocator used for the buffer.
    #[inline]
    pub fn heap(&self) -> &A {
        &self.heap
    }
}

/// Safety: Although [`AllocBytes`] contains a raw pointer, it is only mutated
/// during deallocation (in [`Drop`]) and via exclusive mutable access in
/// [`AllocBytesMut`]. Thus, it is safe to implement [`Send`] if the underlying
/// allocator is thread-safe.
unsafe impl<T, A, const ALIGN: usize> Send for AllocBytes<T, A, ALIGN>
where
    T: Deref<Target = A>,
    A: GlobalAlloc + ?Sized,
{
}

impl<T: Deref<Target = A>, A: GlobalAlloc, const ALIGN: usize> AsRef<[u8]>
    for AllocBytes<T, A, ALIGN>
{
    /// Returns a byte slice of the allocated memory.
    ///
    /// # Safety
    ///
    /// This is safe because the invariants of [`AllocBytes`] ensure that the
    /// pointer is valid for [`AllocBytes::len()`] bytes.
    fn as_ref(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.size) }
    }
}

impl<T: Deref<Target = A>, A: GlobalAlloc + ?Sized, const ALIGN: usize> Drop
    for AllocBytes<T, A, ALIGN>
{
    /// Deallocates the allocated memory.
    ///
    /// # Safety
    ///
    /// The memory was allocated with a layout of size [`AllocBytes::cap()`] and
    /// alignment `ALIGN`. If `ptr` is still [`NonNull::dangling()`], no
    /// deallocation is performed.
    fn drop(&mut self) {
        if self.ptr == NonNull::dangling() {
            // No allocation was ever performed.
            return;
        }
        unsafe {
            self.heap.dealloc(
                self.ptr.as_ptr(),
                // SAFETY: The layout here matches the one used during
                // [re]allocation.
                Layout::from_size_align_unchecked(self.cap, ALIGN),
            );
        }
    }
}

/// A mutable byte buffer for building up data with a custom allocator.
///
/// This type allows extending the buffer and then "freezing" it into an
/// immutable [`AllocBytes`] (using [`AllocBytesMut::freeze()`] and/or [`Bytes`]
/// (using [`AllocBytesMut::into_bytes()`]).
pub struct AllocBytesMut<'a, T, A, const ALIGN: usize>
where
    T: Deref<Target = A> + 'a,
    A: GlobalAlloc + ?Sized + 'a,
{
    inner: AllocBytes<T, A, ALIGN>,
    _phantom: core::marker::PhantomData<&'a ()>,
}

impl<T: Deref<Target = A>, A: GlobalAlloc> AllocBytesMut<'_, T, A, 1> {
    /// Creates a new mutable byte buffer with unaligned allocations
    /// (ie: alignment = 1).
    pub fn new_unaligned(heap: T) -> Self {
        Self {
            inner: AllocBytes {
                heap,
                // Use dangling pointer as a sentinel for "no allocation".
                ptr: NonNull::dangling(),
                size: 0,
                cap: 0,
                _phantom: core::marker::PhantomData,
            },
            _phantom: core::marker::PhantomData,
        }
    }
}

impl<T, A, const ALIGN: usize> AllocBytesMut<'_, T, A, ALIGN>
where
    T: Deref<Target = A>,
    A: GlobalAlloc,
{
    /// Creates a new mutable byte buffer.
    ///
    /// This is the same as [`AllocBytesMut::new_unaligned()`] but used
    /// when `ALIGN` needs to be > 1.
    ///
    /// See: [`AllocBytesMut::new_unaligned()`]
    pub fn new(heap: T) -> Self {
        Self {
            inner: AllocBytes {
                heap,
                ptr: NonNull::dangling(),
                size: 0,
                cap: 0,
                _phantom: core::marker::PhantomData,
            },
            _phantom: core::marker::PhantomData,
        }
    }

    /// Returns the current capacity of the buffer in bytes.
    #[inline]
    pub const fn capacity(&self) -> usize {
        self.inner.cap
    }

    /// Returns the current length (number of bytes used) of the buffer.
    #[inline]
    pub const fn len(&self) -> usize {
        self.inner.size
    }

    /// Returns `true` if the buffer is empty.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Ensures that the buffer has at least `additional` extra bytes of
    /// capacity.
    ///
    /// If the current capacity is insufficient, the buffer is reallocated.
    ///
    /// # Errors
    ///
    /// Returns an error if the new capacity calculation overflows, the layout
    /// is invalid, or the allocation/reallocation fails.
    ///
    /// Data, length, and capacity are left unchanged if an error occurs.
    #[inline]
    pub fn reserve(&mut self, additional: usize) -> Result<(), AllocBytesError> {
        let len = self.len();
        debug_assert!(
            self.inner.cap >= len,
            "Capacity is less than length in AllocBytesMut::reserve"
        );
        // SAFETY: `AllocBytes::cap` is always >= `AllocBytes::size`.
        let remaining = unsafe { self.inner.cap.unchecked_sub(len) };
        if additional <= remaining {
            return Ok(());
        }
        let new_capacity = len
            .checked_add(additional)
            .ok_or(AllocBytesError::ReserveTooLarge)?;
        let new_ptr = unsafe {
            if self.inner.ptr == NonNull::dangling() {
                let layout = if ALIGN == 1 {
                    // SAFETY: For alignment 1, layout creation cannot fail.
                    Layout::from_size_align_unchecked(new_capacity, ALIGN)
                } else {
                    Layout::from_size_align(new_capacity, ALIGN).map_err(AllocBytesError::Layout)?
                };
                self.inner.heap.alloc(layout)
            } else {
                let layout = if ALIGN == 1 {
                    // SAFETY: For alignment 1, layout creation cannot fail.
                    Layout::from_size_align_unchecked(self.inner.cap, ALIGN)
                } else {
                    Layout::from_size_align(self.inner.cap, ALIGN)
                        .map_err(AllocBytesError::Layout)?
                };
                self.inner
                    .heap
                    .realloc(self.inner.ptr.as_ptr(), layout, new_capacity)
            }
        };
        let new_ptr = NonNull::new(new_ptr).ok_or(AllocBytesError::FailedToRealloc)?;
        self.inner.cap = new_capacity;
        self.inner.ptr = new_ptr;
        Ok(())
    }

    /// Extends the buffer by copying bytes from the provided slice.
    ///
    /// # Errors
    ///
    /// Returns an error if reserving additional capacity fails.
    #[inline]
    pub fn extend_from_slice(&mut self, extend: &[u8]) -> Result<(), AllocBytesError> {
        let cnt = extend.len();
        self.reserve(cnt)?;

        unsafe {
            // SAFETY:
            // - We assume that `AllocBytes::size` <= `AllocBytes::cap`.
            // - The source and destination cannot overlap if we own a mutable
            //   reference to the data, since rust's borrow checker disallows
            //   mutable references and non-mutable references to the same data.
            debug_assert!(
                self.inner.size <= self.inner.cap,
                "Size is greater than capacity in AllocBytesMut::extend_from_slice"
            );
            core::ptr::copy_nonoverlapping(
                extend.as_ptr(),
                self.inner.ptr.as_ptr().add(self.inner.size),
                cnt,
            );
            // SAFETY:
            // - We previously reserved `cnt` additional bytes, so it should
            //   have failed if the new size would overflow.
            debug_assert!(
                self.inner.size <= usize::MAX - cnt,
                "Overflow in AllocBytesMut::extend_from_slice"
            );
            self.inner.size = self.inner.size.unchecked_add(cnt);
        }
        Ok(())
    }

    /// Freezes the mutable buffer, converting it into an immutable
    /// [`AllocBytes`].
    #[inline]
    pub fn freeze(self) -> AllocBytes<T, A, ALIGN> {
        self.inner
    }

    /// Converts the mutable buffer into a [`Bytes`] object.
    ///
    /// Requires the `bytes` feature to be  enabled.
    #[cfg(any(doc, feature = "bytes"))]
    #[inline]
    pub fn into_bytes(self) -> Bytes
    where
        T: 'static,
        A: 'static,
    {
        Bytes::from_owner(self.freeze())
    }
}
