use std::mem::MaybeUninit;

use alloc_bytes::{AllocBytesError, AllocBytesMut};
use heap_allocator::FixedHeapAllocator;

#[test]
fn test_extend_and_freeze() {
    let mut alloc_buf = [MaybeUninit::uninit(); 1024];
    let alloc = FixedHeapAllocator::new(&mut alloc_buf);
    let mut buf = AllocBytesMut::new_unaligned(&alloc);
    // Initially the buffer should be empty.
    assert_eq!(buf.len(), 0);
    assert!(buf.is_empty());

    // Extend the buffer with some data.
    buf.extend_from_slice(b"Hello, world!").unwrap();
    assert_eq!(buf.len(), 13);

    // Freeze the mutable buffer into an immutable one.
    let data = buf.freeze();
    assert_eq!(data.len(), 13);
    assert_eq!(data.as_ref(), b"Hello, world!");
}

#[test]
fn test_reserve_increases_capacity() {
    let mut alloc_buf = [MaybeUninit::uninit(); 1024];
    let alloc = FixedHeapAllocator::new(&mut alloc_buf);
    let mut buf = AllocBytesMut::new_unaligned(&alloc);

    // Write some data into the buffer.
    buf.extend_from_slice(b"12345").unwrap();
    let old_capacity = buf.capacity();

    // Reserve additional capacity.
    buf.reserve(10).unwrap();
    let new_capacity = buf.capacity();

    // The new capacity should be at least current length + reserved extra.
    assert!(new_capacity >= buf.len() + 10);
    // In many cases the capacity will increase.
    assert!(new_capacity >= old_capacity);
}

#[test]
fn test_reserve_overflow() {
    let mut alloc_buf = [MaybeUninit::uninit(); 1024];
    let alloc = FixedHeapAllocator::new(&mut alloc_buf);
    let mut buf = AllocBytesMut::new_unaligned(&alloc);

    // Extend the buffer so that the length is non-zero.
    buf.extend_from_slice(b"12345").unwrap();

    // Attempting to reserve an extra huge amount should trigger an overflow error.
    let result = buf.reserve(usize::MAX);
    match result {
        Err(AllocBytesError::ReserveTooLarge) => {} // expected error
        _ => panic!("Expected ReserveTooLarge error"),
    }
}

#[cfg(feature = "bytes")]
#[test]
fn test_into_bytes_conversion() {
    use std::sync::Arc;

    use heap_allocator::MutexOwnedFixedHeapAllocator;

    let mut alloc_buf = Vec::with_capacity(1024);
    alloc_buf.resize(1024, MaybeUninit::uninit());
    let alloc = Arc::new(MutexOwnedFixedHeapAllocator::new(alloc_buf));
    let mut buf = AllocBytesMut::new_unaligned(alloc);
    buf.extend_from_slice(b"Hello Bytes!").unwrap();

    // Convert the mutable buffer into a `Bytes` object.
    let bytes_data = buf.into_bytes();
    assert_eq!(bytes_data.as_ref(), b"Hello Bytes!");
}
