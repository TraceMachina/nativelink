// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use std::collections::{HashMap, HashSet};
use std::io::Cursor;

use bytes::Bytes;
use futures::stream::StreamExt;
use nativelink_macro::nativelink_test;
use nativelink_util::ultracdc::UltraCDC;
use pretty_assertions::assert_eq;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use sha2::{Digest, Sha256};
use tokio::io::AsyncRead;
use tokio_util::codec::{Decoder, FramedRead};

const MEGABYTE_SZ: usize = 1024 * 1024;

/// Extracts the frames of the reader into a vector.
async fn get_frames<T: AsyncRead + Unpin, D: Decoder>(
    frame_reader: &mut FramedRead<T, D>,
) -> Result<Vec<D::Item>, D::Error> {
    let mut frames = vec![];
    while let Some(frame) = frame_reader.next().await {
        frames.push(frame?);
    }
    Ok(frames)
}

#[nativelink_test]
async fn test_all_zeros() -> Result<(), std::io::Error> {
    // For all zeros, always returns chunks of maximum size.
    const ZERO_DATA: [u8; 10240] = [0u8; 10240];
    let mut cursor = Cursor::new(&ZERO_DATA);
    let mut frame_reader = FramedRead::new(&mut cursor, UltraCDC::new(64, 256, 1024));

    let mut total_data = 0;
    while let Some(frame) = frame_reader.next().await {
        let frame = frame?;
        assert_eq!(frame.len(), 1024);
        total_data += frame.len();
    }
    assert_eq!(ZERO_DATA.len(), total_data);
    Ok(())
}

#[nativelink_test]
async fn test_sekien_16k_chunks() -> Result<(), Box<dyn std::error::Error>> {
    let contents = include_bytes!("data/SekienAkashita.jpg");
    let mut cursor = Cursor::new(&contents);
    let mut frame_reader = FramedRead::new(&mut cursor, UltraCDC::new(8192, 16384, 32768));

    let mut frames = vec![];
    let mut sum_frame_len = 0;
    while let Some(frame) = frame_reader.next().await {
        let frame = frame?;
        sum_frame_len += frame.len();
        frames.push(frame);
    }

    assert_eq!(frames.len(), 9);
    assert_eq!(frames[0].len(), 10926);
    assert_eq!(frames[1].len(), 17398);
    assert_eq!(frames[2].len(), 12026);
    assert_eq!(frames[3].len(), 16883);
    assert_eq!(frames[4].len(), 12264);
    assert_eq!(frames[5].len(), 10269);
    assert_eq!(frames[6].len(), 11570);
    assert_eq!(frames[7].len(), 11491);
    assert_eq!(frames[8].len(), 6639);
    assert_eq!(sum_frame_len, contents.len());
    Ok(())
}

#[nativelink_test]
async fn test_random_20mb_16k_chunks() -> Result<(), std::io::Error> {
    let data = {
        let mut data = vec![0u8; 20 * MEGABYTE_SZ];
        let mut rng = SmallRng::seed_from_u64(1);
        rng.fill(&mut data[..]);
        data
    };
    let mut cursor = Cursor::new(&data);
    let mut frame_reader = FramedRead::new(&mut cursor, UltraCDC::new(1024, 2048, 4096));

    let mut lens = vec![];
    for frame in get_frames(&mut frame_reader).await? {
        lens.push(frame.len());
    }
    lens.sort();
    Ok(())
}

#[nativelink_test]
async fn insert_garbage_check_boundarys_recover_test() -> Result<(), std::io::Error> {
    let mut rand_data = {
        let mut data = vec![0u8; 100_000];
        let mut rng = SmallRng::seed_from_u64(1);
        rng.fill(&mut data[..]);
        data
    };

    let fast_cdc = UltraCDC::new(1024, 2048, 4096);
    let left_frames = {
        let mut frame_reader = FramedRead::new(Cursor::new(&rand_data), fast_cdc.clone());
        let frames: Vec<Bytes> = get_frames(&mut frame_reader).await?;

        let mut frames_map = HashMap::new();
        let mut pos = 0;
        for frame in frames {
            let frame_len = frame.len();
            frames_map.insert(hex::encode(Sha256::digest(&frame[..])), (frame, pos));
            pos += frame_len;
        }
        frames_map
    };

    {
        // Replace 2k of bytes and append one byte to middle.
        let mut rng = SmallRng::seed_from_u64(2);
        rng.fill(&mut rand_data[0..2000]);
        rand_data.insert(rand_data.len() / 2, 0x71);
    }

    let mut right_frames = {
        let mut frame_reader = FramedRead::new(Cursor::new(&rand_data), fast_cdc.clone());
        let frames: Vec<Bytes> = get_frames(&mut frame_reader).await?;

        let mut frames_map = HashMap::new();
        let mut pos = 0;
        for frame in frames {
            let frame_len = frame.len();
            frames_map.insert(hex::encode(Sha256::digest(&frame[..])), (frame, pos));
            pos += frame_len;
        }
        frames_map
    };

    let mut expected_missing_hashes = HashSet::<String>::new();
    {
        expected_missing_hashes
            .insert("3461d18541f76cbaf93921775c75f1cae6a907883753c1b6ca0d5d06996d9b20".into());
        expected_missing_hashes
            .insert("74a991a6dfe28f57a367c9c2ba5920dd922c321ab4be46c066cc2a255c886d63".into());
        expected_missing_hashes
            .insert("f1c367e2648ac8b3a77af867a786a51803f96c8ce33ec8ab19fc466d92fade57".into());
        expected_missing_hashes
            .insert("c304bd66045d69171ecf1124b6d2d32801fcf74e242c38e9bf9d54c242d60e33".into());

        for key in left_frames.keys() {
            let maybe_right_frame = right_frames.get(key);
            if maybe_right_frame.is_none() {
                assert_eq!(
                    expected_missing_hashes.contains(key),
                    true,
                    "Expected to find: {}",
                    key
                );
                expected_missing_hashes.remove(key);
            } else {
                right_frames.remove(key);
            }
        }
    }
    let mut expected_new_hashes = HashMap::<String, usize>::new();
    {
        expected_new_hashes.insert(
            "9e9891cc82822f6ed325efa5d749d25f7c65dade324e5fc7fe5b206d1c89d642".into(),
            0,
        );
        expected_new_hashes.insert(
            "6cc0066e9f85f21cd36fd130666fd95f3ed2398f46725438cf3225c23b773c94".into(),
            1235,
        );

        expected_new_hashes.insert(
            "b355bd47dd26b1ce40caa2df54d3b39ee766d9c556dccc44971f8315fe0da66a".into(),
            3087,
        );
        expected_new_hashes.insert(
            "e1140aa52c81cf9055129574d4749d622c33599768e96b1e4e3cdc8ecb9e5c7d".into(),
            49940,
        );
        

        for (key, (_, pos)) in right_frames {
            let expected_pos = expected_new_hashes.get(&key);
            assert_eq!(Some(&pos), expected_pos, "For hash {}", key);
            expected_new_hashes.remove(&key);
        }
    }
    assert_eq!(
        expected_missing_hashes.len(),
        0,
        "Some old hashes were not found"
    );
    assert_eq!(
        expected_new_hashes.len(),
        0,
        "Some new hashes were not consumed"
    );

    Ok(())
}
