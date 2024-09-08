// Copyright 2023 Nathan Fiedler, Trace Machina, Inc.  Some rights reserved.
// Originally licensed under MIT license.
//
// This implementation is heavily based on:
// https://github.com/PlakarLabs/go-cdc-chunkers/blob/main/chunkers/ultracdc/ultracdc.go

use bytes::{Bytes, BytesMut};
use tokio_util::codec::Decoder;

struct State {
    hash: u32,
    position: usize,
}

impl State {
    fn reset(&mut self) {
        self.hash = 0;
        self.position = 0;
    }
}

/// This algorithm will take an input stream and build frames based on UltraCDC algorithm.
/// see: <https://www.usenix.org/system/files/conference/atc16/atc16-paper-xia.pdf>
///
/// In layman's terms this means we can take an input of unknown size and composition
/// and chunk it into smaller chunks with chunk boundaries that will be very similar
/// even when a small part of the file is changed. This use cases where a large file
/// (say 1G) has a few minor changes weather they byte inserts, deletes or updates
/// and when running through this algorithm it will slice the file up so that under
/// normal conditions all the chunk boundaries will be identical except the ones near
/// the mutations.
///
/// This is not very useful on its own, but is extremely useful because we can pair
/// this together with a hash algorithm (like sha256) to then hash each chunk and
/// then check to see if we already have the sha256 somewhere before attempting to
/// upload the file. If the file does exist, we can skip the chunk and continue then
/// in the index file we can reference the same chunk that already exists.
///
/// Or put simply, it helps upload only the parts of the files that change, instead
/// of the entire file.
pub struct UltraCDC {
    min_size: usize,
    avg_size: usize,
    max_size: usize,

    norm_size: usize,
    mask_easy: u32,
    mask_hard: u32,

    state: State,
}

impl UltraCDC {
    pub fn new(min_size: usize, avg_size: usize, max_size: usize) -> Self {
        assert!(min_size < avg_size, "Expected {min_size} < {avg_size}");
        assert!(avg_size < max_size, "Expected {avg_size} < {max_size}");
        let norm_size = {
            let mut offset = min_size + ((min_size + 1) / 2);
            if offset > avg_size {
                offset = avg_size;
            }
            avg_size - offset
        };
        // Calculate the number of bits closest approximating our average.
        let bits = (avg_size as f64).log2().round() as u32;
        Self {
            min_size,
            avg_size,
            max_size,

            norm_size,
            // Turn our bits into a bitmask we can use later on for more
            // efficient bitwise operations.
            mask_hard: (2u64.pow(bits + 1) - 1) as u32,
            mask_easy: (2u64.pow(bits - 1) - 1) as u32,

            state: State {
                hash: 0,
                position: 0,
            },
        }
    }

    pub fn rolling_hash(&self, data: &[u8], window_size: usize) -> u64 {
        // Implement a simple rolling hash function
        let mut hash_val = 0u64;
        for (i, &byte) in data.iter().enumerate().take(window_size) {
            hash_val = hash_val.saturating_add((byte as u64) << (i & 0x1F));
        }
        hash_val
    }
}

impl Decoder for UltraCDC {
    type Item = Bytes;
    type Error = std::io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if buf.len() <= self.min_size {
            return Ok(None);
        }
        // Zero means not found.
        let mut split_point = 0;

        let window_size = 64;

        // Note: We use this kind of loop because it improved performance of this loop by 20%.
        let mut i = 0;

        while i < buf.len() {
            // Create a dynamic slice from buf
            let chunk = &buf[..=i];

            // Determine the appropriate mask based on the current index.
            let mask = if i < self.norm_size {
                self.mask_hard as u64
            } else {
                self.mask_easy as u64
            };

            if chunk.len() >= self.min_size {
                // Calculate the rolling hash value
                let chunk_len = chunk.len();
                let bytes_mut = &chunk[chunk_len.saturating_sub(window_size)..];
                let rolling_hash_value = self.rolling_hash(bytes_mut, window_size);

                if (rolling_hash_value & mask) == mask || chunk_len > self.max_size {
                    split_point = i;
                    break;
                }
            }
            i += 1;
        }

        if split_point >= self.min_size {
            self.state.reset();
            debug_assert!(
                split_point <= self.max_size,
                "Expected {} < {}",
                split_point,
                self.max_size
            );
            return Ok(Some(buf.split_to(split_point).freeze()));
        }
        self.state.position = split_point;
        if self.state.position == 0 {
            self.state.position = buf.len();
        }

        Ok(None)
    }

    fn decode_eof(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match self.decode(buf)? {
            Some(frame) => Ok(Some(frame)),
            // If we are EOF and have no more bytes in stream return the entire buffer.
            None => {
                self.state.reset();
                if buf.is_empty() {
                    // If our buffer is empty we don't have any more data.
                    return Ok(None);
                }
                Ok(Some(buf.split().freeze()))
            }
        }
    }
}

impl Clone for UltraCDC {
    /// Clone configuration but with new state. This is useful where you can create
    /// a base UltraCDC object then clone it when you want to process a new stream.
    fn clone(&self) -> Self {
        Self {
            min_size: self.min_size,
            avg_size: self.avg_size,
            max_size: self.max_size,

            norm_size: self.norm_size,
            mask_easy: self.mask_easy,
            mask_hard: self.mask_hard,

            state: State {
                hash: 0,
                position: 0,
            },
        }
    }
}
