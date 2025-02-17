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
//! # Fixed Heap Allocator
//!
//! This crate provides a fixed heap allocator based on the TLSF (Two-Level
//! Segregated Fit) algorithm. TLSF is a fast, constant-time memory allocation
//! algorithm designed for real-time applications. It organizes free memory into
//! segregated lists based on block sizes, allowing for quick allocation and
//! deallocation with minimal fragmentation.
//!
//! This allocator is designed for use in `no_std` environments and allocates
//! memory from a user-provided pre-allocated memory region. It supports both
//! single-threaded and multi-threaded usage via conditional compilation
//! features.
//!
//! ## Features
//!
//! - **Customizable Memory Block:** Supply your own memory region (as a slice
//!   of `MaybeUninit<u8>`) to back the allocator.
//! - **Thread-Safe Option:** When the `mutex` feature is enabled, a thread-safe
//!   variant (`MutexOwnedFixedHeapAllocator`)
//!   is available for multi-threaded environments.
//! - **Zero-Size Allocations:** Zero-size allocations are handled by returning
//!   a dangling pointer, following the Rust [`GlobalAlloc`] convention.
//!
//! ## Why This Crate?
//!
//! This crate was born out of the need to track memory usage for a specific
//! module and to evict cache items when available memory becomes scarce. Our
//! primary goal was to have fine-grained control over allocation sizes, which
//! is crucial for efficient cache management.
//!
//! Initially, we attempted to use allocators like **mimalloc** and **jemalloc**
//! as well as exploring **glibc**'s allocator. However, we encountered
//! significant challenges:
//!
//! - **mimalloc:** Although mimalloc is renowned for its high performance and
//!   thread-local caching, its internal handling of specialized heaps in
//!   multi-threaded environments made it unsuitable for our need to precisely
//!   track allocation sizes.
//!
//! - **jemalloc:** Despite its robustness, bundling jemalloc was problematic
//!   due to the crate providing non-deterministic builds, which is a
//!   requirement for our releases.
//!
//! Consequently, we turned to TLSF, implemented via the
//! [rlsf](https://docs.rs/rlsf) crate. Benchmarking revealed that our
//! TLSF-based allocator substantially outperformed our previous global-mimalloc
//! implementation in our specific use-case, even though raw memory throughput
//! was not our primary concern.
//!
//! This crate is a sister crate to [alloc_bytes](https://docs.rs/alloc_bytes),
//! which provides custom byte buffers using a custom allocator. By leveraging
//! an allocator/heap that is pre-allocated and fixed in size, we can ensure
//! that memory usage is predictable and that we can track memory usage with
//! precision.
//!
//! ## Usage Examples
//!
//! ### Multi-Threaded Example
//!
//! When using the `mutex` feature, you can use the thread-safe allocator in
//! multi-threaded applications:
//!
//! ```rust
//! use heap_allocator::MutexOwnedFixedHeapAllocator;
//! use core::alloc::{GlobalAlloc, Layout};
//! use core::mem::MaybeUninit;
//! use std::sync::LazyLock;
//! use std::thread;
//!
//! // Create a static, thread-safe allocator using std's LazyLock.
//! // This allocator is backed by a Vec that serves as the memory pool.
//! static ALLOCATOR: LazyLock<MutexOwnedFixedHeapAllocator<Vec<MaybeUninit<u8>>>> = LazyLock::new(|| {
//!     let mut memory = Vec::with_capacity(64 * 1024);
//!     // Fill the vector with uninitialized memory.
//!     memory.resize(64 * 1024, MaybeUninit::uninit());
//!     MutexOwnedFixedHeapAllocator::new(memory)
//! });
//!
//! fn main() {
//!     // Spawn multiple threads that allocate and deallocate memory
//!     // concurrently.
//!     let handles: Vec<_> = (0..4)
//!         .map(|_| {
//!             thread::spawn(|| {
//!                 unsafe {
//!                     let layout = Layout::from_size_align(128, 8).unwrap();
//!                     let ptr = ALLOCATOR.alloc(layout);
//!                     // Simulate work with allocated memory...
//!                     ALLOCATOR.dealloc(ptr, layout);
//!                 }
//!             })
//!         })
//!         .collect();
//!
//!     for handle in handles {
//!         handle.join().unwrap();
//!     }
//!
//!     // For high-performance multi-threaded environments, consider using
//!     // allocators such as mimalloc or jemalloc,
//!     // which provide advanced techniques like thread-local caching to reduce
//!     // contention.
//! }
//! ```
//!
//! This example uses `std::sync::LazyLock` (available in Rust 1.63+) to
//! initialize a static allocator without relying on external crates.
//!
//! ### Single-Threaded Example
//!
//! In a single-threaded environment, you can use the unsynchronized allocator
//! for improved performance:
//!
//! ```rust
//! use heap_allocator::FixedHeapAllocator;
//! use core::alloc::{GlobalAlloc, Layout};
//!
//! // Create a memory block for the heap.
//! let mut memory = [core::mem::MaybeUninit::uninit(); 64 * 1024];
//!
//! // Create an unsynchronized allocator.
//! let allocator = FixedHeapAllocator::new(&mut memory);
//!
//! // Allocation example.
//! unsafe {
//!     let layout = Layout::from_size_align(256, 8).unwrap();
//!     let ptr = allocator.alloc(layout);
//!     // Use allocated memory...
//!     allocator.dealloc(ptr, layout);
//! }
//! ```
//!
//! ## About TLSF
//!
//! The TLSF (Two-Level Segregated Fit) algorithm is designed to offer
//! constant-time allocation and deallocation in the worst-case scenario, making
//! it especially suitable for real-time and embedded systems. TLSF divides free
//! memory into several size classes using bit-level operations, which allows it
//! to quickly locate a free block that best fits the requested allocation size.
//!
//! This allocator leverages TLSF to manage a fixed, user-supplied memory
//! region, ensuring efficient and predictable memory management with low
//! fragmentation.
//!
//! ## Related Crates
//!
//! - **`alloc_bytes`:** A crate that provides custom byte buffers using a
//!   custom allocator. See [alloc_bytes](https://docs.rs/alloc_bytes) for more
//!   details.
//! - **rlsf:** The TLSF algorithm implementation is provided by the
//!   [rlsf](https://docs.rs/rlsf) crate, which offers efficient, constant-time
//!   memory allocation and deallocation routines suitable for real-time and
//!   embedded systems.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::ptr::NonNull;

use ouroboros::self_referencing;
#[cfg(feature = "mutex")]
use parking_lot::Mutex;
use rlsf::Tlsf;

#[cfg(feature = "mutex")]
/// A thread-safe fixed heap allocator that wraps an [`OwnedFixedHeapAllocator`]
/// in a [`parking_lot::Mutex`].
///
/// This type is enabled when the `mutex` feature is active and can be used as a
/// global allocator in multi-threaded environments. Note that using a mutex for
/// allocation may have performance implications. For high-performance
/// scenarios, consider using an allocator with thread-local caching
/// (e.g., `mimalloc` or `jemallocator`).
///
/// # Example
///
/// ```rust
/// use heap_allocator::MutexOwnedFixedHeapAllocator;
/// let mut memory = [core::mem::MaybeUninit::uninit(); 1024];
/// let allocator = MutexOwnedFixedHeapAllocator::new(&mut memory);
/// unsafe {
///     let layout = core::alloc::Layout::from_size_align(128, 8).unwrap();
///     let ptr = allocator.alloc(layout);
///     allocator.dealloc(ptr, layout);
/// }
/// ```
pub struct MutexOwnedFixedHeapAllocator<T: 'static> {
    inner: Mutex<OwnedFixedHeapAllocator<T>>,
}

#[cfg(feature = "mutex")]
impl<T> MutexOwnedFixedHeapAllocator<T>
where
    T: AsMut<[MaybeUninit<u8>]>,
{
    /// Creates a new instance of the mutex-protected fixed heap allocator.
    #[inline]
    pub fn new(data: T) -> Self {
        Self {
            inner: Mutex::new(OwnedFixedHeapAllocator::new(data)),
        }
    }
}

#[cfg(feature = "mutex")]
unsafe impl<T> GlobalAlloc for MutexOwnedFixedHeapAllocator<T> {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.inner.lock().alloc(layout)
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.inner.lock().dealloc(ptr, layout);
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        self.inner.lock().realloc(ptr, layout, new_size)
    }
}

/// An owned fixed heap allocator with erased lifetimes.
///
/// This type is identical to `FixedHeapAllocator<'static, T>` but erases the
/// lifetime of the backing data. This can be useful in complex lifetime
/// scenarios where you want to simplify the type signatures.
///
/// # Example
///
/// ```rust
/// use core::alloc::{GlobalAlloc, Layout};
/// use heap_allocator::OwnedFixedHeapAllocator;
///
/// let mut memory = Vec::with_capacity(64 * 1024);
/// memory.resize(64 * 1024, core::mem::MaybeUninit::uninit());
/// let allocator = OwnedFixedHeapAllocator::new(memory);
/// unsafe {
///     let layout = Layout::from_size_align(256, 8).unwrap();
///     let ptr = allocator.alloc(layout);
///     allocator.dealloc(ptr, layout);
/// }
/// ```
pub struct OwnedFixedHeapAllocator<T: 'static> {
    inner: FixedHeapAllocator<'static, T>,
}

impl<T> OwnedFixedHeapAllocator<T>
where
    T: AsMut<[MaybeUninit<u8>]> + 'static,
{
    /// Creates a new instance of the fixed heap allocator.
    #[inline]
    pub fn new(data: T) -> Self {
        Self {
            inner: FixedHeapAllocator::new(data),
        }
    }
}

unsafe impl<T> GlobalAlloc for OwnedFixedHeapAllocator<T> {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.inner.alloc(layout)
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.inner.dealloc(ptr, layout);
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        self.inner.realloc(ptr, layout, new_size)
    }
}

/// Internal self-referencing structure for `FixedHeapAllocator`.
///
/// This structure ties the lifetime of the TLSF heap to the backing data
/// provided by the user. The `heap` field is constructed using a mutable
/// reference from `data`, allowing the TLSF heap to safely reference the
/// user-provided memory block.
///
/// # Safety
///
/// The lifetime annotations and the use of `UnsafeCell` ensure that the heap
/// does not outlive the backing data. It is critical that the provided memory
/// meets the alignment and size requirements expected by the TLSF algorithm.
#[self_referencing]
struct FixedHeapAllocatorInner<'data, T: 'data> {
    data: T,
    // The TLSF heap is created from the provided memory block.
    #[borrows(mut data)]
    #[not_covariant]
    heap: UnsafeCell<Tlsf<'this, usize, u8, 32, 8>>,
    phantom: core::marker::PhantomData<&'data ()>,
}

/// A fixed heap allocator implemented on top of a user-provided memory region.
///
/// This allocator uses a TLSF (Two-Level Segregated Fit) algorithm to manage
/// memory allocations and implements the [`GlobalAlloc`] trait, making it
/// usable as a global allocator.
///
/// Note: If you encounter lifetime issues, consider using
/// [`OwnedFixedHeapAllocator`], which erases lifetimes.
pub struct FixedHeapAllocator<'data, T: 'data> {
    inner: FixedHeapAllocatorInner<'data, T>,
}

impl<'data, T> FixedHeapAllocator<'data, T>
where
    T: AsMut<[MaybeUninit<u8>]> + 'data,
{
    /// Creates a new `FixedHeapAllocator` instance.
    ///
    /// The provided memory (`data`) is used to initialize a TLSF heap, and the
    /// entire memory block is inserted as a free block.
    ///
    /// # Arguments
    ///
    /// * `data` - A user-provided memory region as a slice of
    ///   `MaybeUninit<u8>`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use heap_allocator::FixedHeapAllocator;
    /// let mut memory = [core::mem::MaybeUninit::uninit(); 1024];
    /// let allocator = FixedHeapAllocator::new(&mut memory);
    /// ```
    pub fn new(data: T) -> Self {
        Self {
            inner: FixedHeapAllocatorInnerBuilder {
                data,
                heap_builder: |data| {
                    let mut heap = Tlsf::new();
                    heap.insert_free_block(data.as_mut());
                    UnsafeCell::new(heap)
                },
                phantom: core::marker::PhantomData,
            }
            .build(),
        }
    }
}

impl<T> FixedHeapAllocator<'_, T> {
    #[inline]
    fn inner_alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() == 0 {
            return NonNull::dangling().as_ptr();
        }
        match self.inner.with_heap(|heap| {
            // Safety: `heap` will always have a value.
            // We will never hold a mutable reference to `heap` while it is
            // borrowed
            unsafe { &mut *heap.get() }.allocate(layout)
        }) {
            Some(ptr) => ptr.as_ptr(),
            None => core::ptr::null_mut(),
        }
    }

    #[inline]
    fn inner_dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ptr == NonNull::dangling().as_ptr() {
            return; // No deallocation necessary for zero-size allocations.
        }
        debug_assert!(!ptr.is_null(), "dealloc called with null pointer");
        // Safety: We would rather have the program at a lower level
        // with a segfault, rather than panic here if the user is calling
        // `dealloc` with a null pointer.
        let raw_ptr = unsafe { NonNull::new_unchecked(ptr) };
        self.inner
            // Safety: `heap` will always have a value.
            // We will never hold a mutable reference to `heap` while it is
            // borrowed.
            .with_heap(|heap| unsafe { (*heap.get()).deallocate(raw_ptr, layout.align()) });
    }

    #[inline]
    fn inner_realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ptr == NonNull::dangling().as_ptr() {
            // Treat a dangling pointer as a zero-size allocation; allocate new
            // memory.
            let layout = unsafe {
                // Safety: All we really want to do is modify `layout` to have the
                // new size and call `alloc` with it, but there's now api for it.
                Layout::from_size_align_unchecked(new_size, layout.align())
            };
            return self.inner_alloc(layout);
        }
        if new_size == 0 {
            // Zero new size: deallocate existing memory and return a dangling
            // pointer.
            self.inner_dealloc(ptr, layout);
            return NonNull::dangling().as_ptr();
        }
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        debug_assert!(!ptr.is_null(), "realloc called with null pointer");
        // Safety: We would rather have the program at a lower level
        // with a segfault, rather than panic here if the user is calling
        // `dealloc` with a null pointer.
        let raw_ptr = unsafe { NonNull::new_unchecked(ptr) };
        match self
            .inner
            // Safety: `heap` will always have a value.
            // We will never hold a mutable reference to `heap` while it is
            // borrowed.
            .with_heap(|heap| unsafe { (*heap.get()).reallocate(raw_ptr, new_layout) })
        {
            Some(new_ptr) => new_ptr.as_ptr(),
            None => core::ptr::null_mut(),
        }
    }
}

unsafe impl<T> GlobalAlloc for FixedHeapAllocator<'_, T> {
    /// Allocates memory according to the specified `layout`.
    ///
    /// For zero-size allocations, a dangling pointer is returned in accordance
    /// with the [`GlobalAlloc`] contract.
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.inner_alloc(layout)
    }

    /// Deallocates the memory referenced by `ptr` using the given `layout`.
    ///
    /// If `ptr` is a dangling pointer (i.e. representing a zero-size
    /// allocation), no action is taken. A debug assertion ensures that a null
    /// pointer is never deallocated.
    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.inner_dealloc(ptr, layout);
    }

    /// Reallocates memory referenced by `ptr` to a new size (`new_size`).
    ///
    /// If `ptr` is a dangling pointer, a new allocation is performed.
    /// If `new_size` is zero, the memory is deallocated and a dangling pointer
    /// is returned. A debug assertion ensures that `ptr` is not null.
    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        self.inner_realloc(ptr, layout, new_size)
    }
}
