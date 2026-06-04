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
