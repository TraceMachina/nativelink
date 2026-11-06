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

use std::io::Cursor;

use nativelink_util::mach_o::is_main_executable;

const MH_MAGIC: u32 = 0xfeed_face;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_MAGIC_64: u32 = 0xcafe_babf;

const MH_OBJECT: u32 = 0x1;
const MH_EXECUTE: u32 = 0x2;
const MH_DYLIB: u32 = 0x6;
const MH_DYLINKER: u32 = 0x7;
const MH_BUNDLE: u32 = 0x8;

const CPU_TYPE_ARM64: u32 = 0x0100_000c;
const SLICE_OFFSET: u32 = 0x4000;

/// The start of a thin Mach-O file of `filetype`.
fn thin(magic: u32, filetype: u32) -> Vec<u8> {
    let mut file: Vec<u8> = [magic, CPU_TYPE_ARM64, 0, filetype]
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect();
    file.resize(64, 0);
    file
}

/// A universal binary whose header claims `archs` architectures, the first of
/// them `slice`.
fn universal(magic: u32, archs: u32, slice: &[u8]) -> Vec<u8> {
    let mut file = Vec::new();
    file.extend(magic.to_be_bytes());
    file.extend(archs.to_be_bytes());
    file.extend(CPU_TYPE_ARM64.to_be_bytes());
    file.extend(0u32.to_be_bytes());
    if magic == FAT_MAGIC_64 {
        file.extend(u64::from(SLICE_OFFSET).to_be_bytes());
    } else {
        file.extend(SLICE_OFFSET.to_be_bytes());
    }
    file.resize(usize::try_from(SLICE_OFFSET).unwrap(), 0);
    file.extend(slice);
    file
}

fn is_main(file: &[u8]) -> bool {
    is_main_executable(&mut Cursor::new(file)).unwrap()
}

#[test]
fn thin_main_executables_are_main_executables() {
    assert!(is_main(&thin(MH_MAGIC_64, MH_EXECUTE)));
    assert!(is_main(&thin(MH_MAGIC, MH_EXECUTE)));
}

/// dyld resolves `@loader_path` from the path it opened, so libraries and
/// everything else may keep sharing an inode.
#[test]
fn other_mach_o_file_types_are_not_main_executables() {
    for filetype in [MH_OBJECT, MH_DYLIB, MH_DYLINKER, MH_BUNDLE] {
        assert!(!is_main(&thin(MH_MAGIC_64, filetype)), "{filetype:#x}");
    }
}

#[test]
fn universal_binaries_are_judged_by_their_first_slice() {
    for magic in [FAT_MAGIC, FAT_MAGIC_64] {
        assert!(is_main(&universal(
            magic,
            1,
            &thin(MH_MAGIC_64, MH_EXECUTE)
        )));
        assert!(!is_main(&universal(magic, 1, &thin(MH_MAGIC_64, MH_DYLIB))));
    }
}

/// A Java class file starts with `FAT_MAGIC` and its version, here 65 (Java
/// 21), in place of the architecture count. Whatever its bytes happen to
/// point at, it is not a universal binary.
#[test]
fn implausible_architecture_counts_are_not_universal_binaries() {
    let main = thin(MH_MAGIC_64, MH_EXECUTE);
    for archs in [0, 65] {
        assert!(!is_main(&universal(FAT_MAGIC, archs, &main)), "{archs}");
    }
}

#[test]
fn files_that_end_before_a_header_are_not_main_executables() {
    assert!(!is_main(&[]));
    let universal = universal(FAT_MAGIC, 1, &thin(MH_MAGIC_64, MH_EXECUTE));
    assert!(!is_main(&universal[..universal.len() - 60]));
}

#[test]
fn scripts_and_sources_are_not_main_executables() {
    assert!(!is_main(b"#!/usr/bin/env python3\nprint('hello, world')\n"));
    assert!(!is_main(
        b"#import <Foundation/Foundation.h>\n@interface Tool : NSObject\n@end\n"
    ));
}

#[cfg(target_os = "macos")]
#[test]
fn system_binaries() -> Result<(), std::io::Error> {
    use std::fs::File;

    // Both are universal binaries; `ls` is a main executable, dyld a dynamic
    // linker.
    assert!(is_main_executable(&mut File::open("/bin/ls")?)?);
    assert!(!is_main_executable(&mut File::open("/usr/lib/dyld")?)?);
    Ok(())
}
