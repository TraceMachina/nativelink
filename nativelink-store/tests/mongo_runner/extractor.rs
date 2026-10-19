// Copyright 2026 The NativeLink Authors. All rights reserved.
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

use std::fs::{self, File};
use std::path::Path;

use flate2::read::GzDecoder;
use nativelink_error::{Error, ResultExt, make_input_err};
use tar::Archive;
use zip::ZipArchive;

pub(crate) fn extract(archive_path: &Path, extract_to: &Path) -> Result<(), Error> {
    if !extract_to.exists() {
        fs::create_dir_all(extract_to)
            .err_tip(|| format!("Creating extraction directory {}", extract_to.display()))?;
    }

    let extension = archive_path
        .extension()
        .and_then(|e| e.to_str())
        .ok_or_else(|| make_input_err!("Unknown file extension"))?;

    match extension {
        "tgz" | "gz" => {
            let file = File::open(archive_path)
                .err_tip(|| format!("Opening {}", archive_path.display()))?;
            let tar = GzDecoder::new(file);
            let mut archive = Archive::new(tar);
            archive
                .unpack(extract_to)
                .err_tip(|| format!("Unpacking to {}", extract_to.display()))?;
        }
        "zip" => {
            let file = File::open(archive_path)
                .err_tip(|| format!("Opening {}", archive_path.display()))?;
            let mut archive = ZipArchive::new(file)?;
            archive
                .extract(extract_to)
                .err_tip(|| format!("Extracting to {}", extract_to.display()))?;
        }
        _ => return Err(make_input_err!("Unsupported archive format: {}", extension)),
    }

    Ok(())
}
