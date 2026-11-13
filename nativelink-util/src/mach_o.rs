// Copyright 2026 Trace Machina, Inc. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Recognizes Mach-O main executables by their header.

use std::io::{self, Read, Seek, SeekFrom};

/// `MH_MAGIC` and `MH_MAGIC_64` from `<mach-o/loader.h>`. A thin header is in
/// the byte order of its architecture, and these are read little-endian: the
/// only byte order a current Mac executes.
const MH_MAGIC: u32 = 0xfeed_face;
const MH_MAGIC_64: u32 = 0xfeed_facf;

/// `FAT_MAGIC` and `FAT_MAGIC_64` from `<mach-o/fat.h>`. A universal binary's
/// header is big-endian on every architecture.
const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_MAGIC_64: u32 = 0xcafe_babf;

/// The file type of a main executable, as opposed to a dylib, bundle or object.
const MH_EXECUTE: u32 = 0x2;

/// A Java class file also starts with `FAT_MAGIC`, followed by its version,
/// which reads as 45 architectures or more. A universal binary has a handful,
/// so, as LLVM's `identify_magic` does, a count from 43 up is not one.
const MAX_FAT_ARCHS: u32 = 42;

/// Whether `file` is a Mach-O main executable (`MH_EXECUTE`), on its own or as
/// the first slice of a universal binary. A file too short to hold the headers
/// is not one.
pub fn is_main_executable(file: &mut (impl Read + Seek)) -> io::Result<bool> {
    // A thin header up to its file type, or a universal header and its first
    // architecture up to the end of a 64-bit slice offset.
    let mut header = [0; 24];
    if !read_at(file, 0, &mut header)? {
        return Ok(false);
    }
    let magic = be_u32(&header, 0);
    if magic != FAT_MAGIC && magic != FAT_MAGIC_64 {
        return Ok(is_thin_main_executable(&header));
    }
    if !(1..=MAX_FAT_ARCHS).contains(&be_u32(&header, 4)) {
        return Ok(false);
    }
    let slice_offset = if magic == FAT_MAGIC {
        u64::from(be_u32(&header, 16))
    } else {
        (u64::from(be_u32(&header, 16)) << 32) | u64::from(be_u32(&header, 20))
    };
    let mut slice = [0; 16];
    Ok(read_at(file, slice_offset, &mut slice)? && is_thin_main_executable(&slice))
}

const fn is_thin_main_executable(header: &[u8]) -> bool {
    let magic = le_u32(header, 0);
    (magic == MH_MAGIC || magic == MH_MAGIC_64) && le_u32(header, 12) == MH_EXECUTE
}

/// Fills `buf` from `offset`, or reports that the file ends first.
fn read_at(file: &mut (impl Read + Seek), offset: u64, buf: &mut [u8]) -> io::Result<bool> {
    file.seek(SeekFrom::Start(offset))?;
    match file.read_exact(buf) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(err) => Err(err),
    }
}

const fn be_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

const fn le_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}
