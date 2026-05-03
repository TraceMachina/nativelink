use std::env;

use nativelink_error::{Error, ResultExt, make_input_err};

#[derive(Debug, Clone)]
pub(crate) enum Os {
    Linux,
    #[allow(clippy::enum_variant_names)]
    MacOs,
    Windows,
}

#[derive(Debug, Clone)]
pub(crate) enum Arch {
    X86_64,
    Aarch64,
}

pub(crate) struct MongoUrl {
    pub(crate) url: String,
    pub(crate) filename: String,
}

pub(crate) fn get_os() -> Result<Os, Error> {
    match env::consts::OS {
        "linux" => Ok(Os::Linux),
        "macos" => Ok(Os::MacOs),
        "windows" => Ok(Os::Windows),
        os => Err(make_input_err!("Unsupported OS: {}", os)),
    }
}

pub(crate) fn get_arch() -> Result<Arch, Error> {
    match env::consts::ARCH {
        "x86_64" => Ok(Arch::X86_64),
        "aarch64" => Ok(Arch::Aarch64),
        arch => Err(make_input_err!("Unsupported Architecture: {}", arch)),
    }
}

pub(crate) fn get_download_url(version: &str) -> Result<MongoUrl, Error> {
    let os = get_os()?;
    let arch = get_arch()?;

    let (_platform, package_format) = match (&os, &arch) {
        (Os::Linux, Arch::X86_64) => ("linux-x86_64", "tgz"),
        (Os::Linux, Arch::Aarch64) => ("linux-aarch64", "tgz"),
        (Os::MacOs, Arch::X86_64) => ("macos-x86_64", "tgz"),
        (Os::MacOs, Arch::Aarch64) => ("macos-aarch64", "tgz"),
        (Os::Windows, Arch::X86_64) => ("windows-x86_64", "zip"),
        _ => return Err(make_input_err!("Unsupported OS/Arch combination")),
    };

    // Note: This is a simplified URL constructor.
    // Real MongoDB URLs are more complex and depend on specific distros for Linux.
    // We might need to handle specific linux distros.
    // For now, let's try to find a generic linux binary or handle specific common ones.

    // Example: https://fastdl.mongodb.org/linux/mongodb-linux-x86_64-ubuntu2004-7.0.2.tgz
    // Example: https://fastdl.mongodb.org/osx/mongodb-macos-x86_64-7.0.2.tgz
    // Example: https://fastdl.mongodb.org/windows/mongodb-windows-x86_64-7.0.2.zip

    // For generic linux, we often need to specify a distro.
    // Let's assume ubuntu2204 for now for linux x64 as a safeish default or try to detect.
    // Or use the "generic" linux legacy if available, but modern mongo usually target distros.

    let base_url = "https://fastdl.mongodb.org";

    let _filename = match os {
        Os::Linux => format!(
            "mongodb-linux-{}-{}.{}",
            "x86_64-ubuntu2204", version, package_format
        ), // HARDCODING ubuntu2204 for x64 linux for now
        Os::MacOs => format!("mongodb-macos-{version}-x86_64.{package_format}"), // Need to fix arch for mac
        Os::Windows => format!("mongodb-windows-x86_64-{version}.{package_format}"),
    };

    // Refined logic
    let url = match (&os, &arch) {
        (Os::Linux, Arch::X86_64) => {
            format!("{base_url}/linux/mongodb-linux-x86_64-ubuntu2204-{version}.tgz",)
        }
        (Os::Linux, Arch::Aarch64) => {
            format!("{base_url}/linux/mongodb-linux-aarch64-ubuntu2204-{version}.tgz",)
        }
        (Os::MacOs, Arch::X86_64) => {
            format!("{base_url}/osx/mongodb-macos-x86_64-{version}.tgz")
        }
        (Os::MacOs, Arch::Aarch64) => {
            format!("{base_url}/osx/mongodb-macos-arm64-{version}.tgz")
        }
        (Os::Windows, Arch::X86_64) => {
            format!("{base_url}/windows/mongodb-windows-x86_64-{version}.zip")
        }
        _ => return Err(make_input_err!("Unsupported OS/Arch combination")),
    };

    let filename = url.split('/').next_back().unwrap().to_string();

    Ok(MongoUrl { url, filename })
}

#[derive(Debug)]
pub(crate) struct DownloadProgress {
    pub(crate) downloaded: u64,
    pub(crate) total: Option<u64>,
    pub(crate) percentage: Option<f32>,
}

pub(crate) async fn download_file(url: &str, destination: &std::path::Path) -> Result<(), Error> {
    download_file_with_callback(url, destination, |_| {}).await
}

pub(crate) async fn download_file_with_callback<F>(
    url: &str,
    destination: &std::path::Path,
    mut callback: F,
) -> Result<(), Error>
where
    F: FnMut(DownloadProgress),
{
    use std::fs::File;
    use std::io::Write;

    let response = reqwest::get(url).await?;
    let total = response.content_length();

    let mut part_path = destination.to_path_buf();
    part_path.set_extension("part");

    let mut file =
        File::create(&part_path).err_tip(|| format!("Creating {}", part_path.display()))?;
    let mut downloaded: u64 = 0;

    let mut stream = response;
    while let Some(chunk) = stream.chunk().await? {
        file.write_all(&chunk)?;
        downloaded += chunk.len() as u64;

        let percentage = total.map(|t| (downloaded as f32 / t as f32) * 100.0);
        callback(DownloadProgress {
            downloaded,
            total,
            percentage,
        });
    }

    std::fs::rename(part_path, destination)?;

    Ok(())
}
