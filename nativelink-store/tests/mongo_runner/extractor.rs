use std::fs::File;
use std::path::Path;

use flate2::read::GzDecoder;
use nativelink_error::{Error, make_input_err};
use tar::Archive;
use zip::ZipArchive;

pub(crate) fn extract(archive_path: &Path, extract_to: &Path) -> Result<(), Error> {
    if !extract_to.exists() {
        std::fs::create_dir_all(extract_to)?;
    }

    let extension = archive_path
        .extension()
        .and_then(|e| e.to_str())
        .ok_or_else(|| make_input_err!("Unknown file extension"))?;

    match extension {
        "tgz" | "gz" => {
            let file = File::open(archive_path)?;
            let tar = GzDecoder::new(file);
            let mut archive = Archive::new(tar);
            archive.unpack(extract_to)?;
        }
        "zip" => {
            let file = File::open(archive_path)?;
            let mut archive = ZipArchive::new(file)?;
            archive.extract(extract_to)?;
        }
        _ => return Err(make_input_err!("Unsupported archive format: {}", extension)),
    }

    Ok(())
}
