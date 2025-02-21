use core::mem::MaybeUninit;
use std::alloc::{GlobalAlloc, Layout};

use heap_allocator::{FixedHeapAllocator, OwnedFixedHeapAllocator};

#[test]
fn test_fixed_heap_alloc_dealloc() {
    let mut memory = [MaybeUninit::uninit(); 1024];
    let allocator = FixedHeapAllocator::new(&mut memory);

    unsafe {
        // Allocate 128 bytes with 8-byte alignment.
        let layout = Layout::from_size_align(128, 8).unwrap();
        let ptr = allocator.alloc(layout);
        assert!(!ptr.is_null(), "Allocation should return a valid pointer");
        assert!(ptr as usize % 8 == 0, "Pointer should be 8-byte aligned");

        for i in 0..128 {
            *ptr.add(i) = i as u8;
        }

        allocator.dealloc(ptr, layout);
    }
}

#[test]
fn test_zero_size_allocation() {
    let mut memory = [MaybeUninit::uninit(); 1024];
    let allocator = FixedHeapAllocator::new(&mut memory);

    unsafe {
        let layout = Layout::from_size_align(0, 8).unwrap();
        let ptr = allocator.alloc(layout);
        assert_eq!(ptr, core::ptr::NonNull::dangling().as_ptr());
    }
}

#[test]
fn test_fixed_heap_realloc_increase() {
    let mut memory = [MaybeUninit::uninit(); 1024];
    let allocator = FixedHeapAllocator::new(&mut memory);

    unsafe {
        let layout = Layout::from_size_align(64, 8).unwrap();
        let ptr = allocator.alloc(layout);
        assert!(!ptr.is_null(), "Initial allocation should succeed");
        assert!(ptr as usize % 8 == 0, "Pointer should be 8-byte aligned");

        for i in 0..64 {
            *ptr.add(i) = (i + 1) as u8;
        }

        // Increase the allocation size to 128 bytes.
        let new_size = 128;
        let new_ptr = allocator.realloc(ptr, layout, new_size);
        assert!(!new_ptr.is_null(), "Reallocation should succeed");

        for i in 0..64 {
            assert_eq!(
                *new_ptr.add(i),
                (i + 1) as u8,
                "Data should be preserved after realloc"
            );
        }

        let new_layout = Layout::from_size_align(new_size, 8).unwrap();
        allocator.dealloc(new_ptr, new_layout);
    }
}

#[test]
fn test_fixed_heap_realloc_decrease() {
    let mut memory = [MaybeUninit::uninit(); 1024];
    let allocator = FixedHeapAllocator::new(&mut memory);

    unsafe {
        let layout = Layout::from_size_align(64, 1).unwrap();
        let ptr = allocator.alloc(layout);
        assert!(!ptr.is_null(), "Initial allocation should succeed");

        for i in 0..64 {
            *ptr.add(i) = (i + 1) as u8;
        }

        // Shrink the allocation size to 0 bytes.
        let new_ptr = allocator.realloc(ptr, layout, 0);
        assert!(!new_ptr.is_null(), "Reallocation should succeed");
        assert_eq!(new_ptr, core::ptr::NonNull::dangling().as_ptr());

        let new_layout = Layout::from_size_align(0, 1).unwrap();
        allocator.dealloc(new_ptr, new_layout);
    }
}

#[test]
fn test_fixed_heap_realloc_to_zero() {
    let mut memory = [MaybeUninit::uninit(); 1024];
    let allocator = FixedHeapAllocator::new(&mut memory);

    unsafe {
        let layout = Layout::from_size_align(64, 1).unwrap();
        let ptr = allocator.alloc(layout);
        assert!(!ptr.is_null(), "Initial allocation should succeed");

        let new_ptr = allocator.realloc(ptr, layout, 0);
        assert_eq!(new_ptr, core::ptr::NonNull::dangling().as_ptr());
    }
}

#[test]
fn test_owned_fixed_heap_alloc_dealloc() {
    let mut memory = Vec::with_capacity(1024);
    memory.resize(1024, MaybeUninit::uninit());
    let allocator = OwnedFixedHeapAllocator::new(memory);

    unsafe {
        let layout = Layout::from_size_align(128, 1).unwrap();
        let ptr = allocator.alloc(layout);
        assert!(!ptr.is_null(), "Allocation should return a valid pointer");
        allocator.dealloc(ptr, layout);
    }
}

#[cfg(feature = "mutex")]
#[test]
fn test_mutex_owned_fixed_heap_alloc_dealloc() {
    use heap_allocator::MutexOwnedFixedHeapAllocator;

    let mut memory = Vec::with_capacity(1024);
    memory.resize(1024, MaybeUninit::uninit());
    let allocator = MutexOwnedFixedHeapAllocator::new(memory);

    unsafe {
        let layout = Layout::from_size_align(128, 1).unwrap();
        let ptr = allocator.alloc(layout);
        assert!(!ptr.is_null(), "Allocation should return a valid pointer");
        allocator.dealloc(ptr, layout);
    }
}
