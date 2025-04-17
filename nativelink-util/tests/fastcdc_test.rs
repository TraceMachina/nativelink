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
use nativelink_util::fastcdc::FastCDC;
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
    let mut frame_reader = FramedRead::new(&mut cursor, FastCDC::new(64, 256, 1024));

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
async fn test_sekien_16k_chunks() -> Result<(), Box<dyn core::error::Error>> {
    let contents = include_bytes!("data/SekienAkashita.jpg");
    let mut cursor = Cursor::new(&contents);
    let mut frame_reader = FramedRead::new(&mut cursor, FastCDC::new(0x2000, 0x4000, 0x8000));

    let mut frames = vec![];
    let mut sum_frame_len = 0;
    while let Some(frame) = frame_reader.next().await {
        let frame = frame?;
        sum_frame_len += frame.len();
        frames.push(frame);
    }
    assert_eq!(frames.len(), 6);
    assert_eq!(frames[0].len(), 22365);
    assert_eq!(frames[1].len(), 8282);
    assert_eq!(frames[2].len(), 16303);
    assert_eq!(frames[3].len(), 18696);
    assert_eq!(frames[4].len(), 32768);
    assert_eq!(frames[5].len(), 11052);
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
    let mut frame_reader = FramedRead::new(&mut cursor, FastCDC::new(1024, 2048, 4096));

    let mut lens = vec![];
    for frame in get_frames(&mut frame_reader).await? {
        lens.push(frame.len());
    }
    lens.sort_unstable();
    Ok(())
}

#[nativelink_test]
async fn insert_garbage_check_boundaries_recover_test() -> Result<(), std::io::Error> {
    let mut rand_data = {
        let mut data = vec![0u8; 100_000];
        let mut rng = SmallRng::seed_from_u64(1);
        rng.fill(&mut data[..]);
        data
    };

    let fast_cdc = FastCDC::new(1024, 2048, 4096);
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

    // Replace 2k of bytes and append one byte to middle.
    let mut rng = SmallRng::seed_from_u64(2);
    rng.fill(&mut rand_data[0..4097]);
    rand_data.insert(rand_data.len() / 2, 0x71);

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
            .insert("33e2babb553746787fd56754bfd26f25aa2ccf556930fc0e675915e14487f55b".into());
        expected_missing_hashes
            .insert("7bae75fcdc38f2e24bfb5853d3494ab10fffd1ad412ef61ae3e876aa370a8065".into());
        expected_missing_hashes
            .insert("bd71822ebcef1d4dfde53d58afc15423ac02cab3588614ab920876cdf0f8d03b".into());
        expected_missing_hashes
            .insert("c0946faf7601cc708db30b31d5eecb14315f6e1339d79fa9da12416ee8841c1b".into());
        expected_missing_hashes
            .insert("e6d7c4916345db82a5b368834650c9afb51e519e559537efe107d4f386407af3".into());

        let mut left_keys = left_frames.keys().collect::<Vec<&String>>();
        left_keys.sort();

        for key in left_keys {
            let maybe_right_frame = right_frames.get(key);
            if maybe_right_frame.is_none() {
                println!("missing {key}");
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
            "57154d1c089eda6418a1e9ab5ca0b929eb0e1e0cd1a35b2907195e33e938554b".into(),
            0,
        );
        expected_new_hashes.insert(
            "a305c22e712c03c787352f34722d1f223c87f13d4e95253b8a55a6c06a482006".into(),
            2710,
        );
        expected_new_hashes.insert(
            "58e4340a3957d12042fc968cc955ac1059219b1e38198e7f11dac1bada8b7d3f".into(),
            4141,
        );
        expected_new_hashes.insert(
            "16df63de6819fc0565dbd1bdd8656c9557bb04639714e86655b562f7218c6e68".into(),
            5182,
        );
        expected_new_hashes.insert(
            "c75449da71b4e1aabfcab7c449602411c3c684e85b66b6c176f1ab2716059b69".into(),
            49909,
        );

        let mut right_frames = right_frames.into_iter().collect::<Vec<_>>();
        right_frames.sort_by_key(|(_, (_, pos))| *pos);

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
