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

    assert_eq!(frames.len(), 7);
    assert_eq!(frames[0].len(), 11736);
    assert_eq!(frames[1].len(), 12943);
    assert_eq!(frames[2].len(), 19338);
    assert_eq!(frames[3].len(), 13431);
    assert_eq!(frames[4].len(), 32085);
    assert_eq!(frames[5].len(), 8307);
    assert_eq!(frames[6].len(), 11626);
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

    let ultra_cdc = UltraCDC::new(1024, 2048, 4096);
    let left_frames = {
        let mut frame_reader = FramedRead::new(Cursor::new(&rand_data), ultra_cdc.clone());
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
        let mut frame_reader = FramedRead::new(Cursor::new(&rand_data), ultra_cdc.clone());
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
            .insert("a4f5dd18ae3e3755b1164e749be81fad67895855b1195a5dc48b0ff53b4557c9".into());
        expected_missing_hashes
            .insert("75a16622f32c04e14153ac6aa2cfc568ee22bb733d53e69fb25f4d1d8804529c".into());
        expected_missing_hashes
            .insert("387352569fbaed18df917f38a443a3e52bd955cd358f747820ac6b6fc01b84e0".into());

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
            "8bc461a793d5b03d141cd5e850ac0cf48c7930309fa125064fa72df55684824b".into(),
            0,
        );
        expected_new_hashes.insert(
            "40f17bb3b15bee181e994bd1f6786e694cb5575c1e12a5f90168f682335d06bd".into(),
            1414,
        );

        expected_new_hashes.insert(
            "2f156157df63ec06ea2e60b0cb30fec00120c3b903557b0d438156fe0dedc96a".into(),
            49156,
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
