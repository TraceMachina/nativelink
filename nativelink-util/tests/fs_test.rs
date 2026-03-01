#![cfg(not(target_family = "windows"))]
// Because windows does permissions differently

use std::env;
use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;

use nativelink_error::ResultExt;
use nativelink_macro::nativelink_test;
use nativelink_util::fs::remove_dir_all;

#[nativelink_test]
async fn remove_files_with_bad_permissions() -> Result<(), Box<dyn core::error::Error>> {
    let temp_dir = env::temp_dir();
    let bad_perms_directory = temp_dir.join("bad_perms_directory");
    if fs::exists(&bad_perms_directory)? {
        remove_dir_all(&bad_perms_directory)
            .await
            .err_tip(|| format!("first remove_dir_all for {bad_perms_directory:?}"))?;
    }
    fs::create_dir(&bad_perms_directory)?;
    let bad_perms_file = bad_perms_directory.join("bad_perms_file");
    if !fs::exists(&bad_perms_file)? {
        fs::write(&bad_perms_file, "").err_tip(|| "Can't create file")?;
    }

    fs::set_permissions(&bad_perms_directory, Permissions::from_mode(0o100)) // execute owner only
        .err_tip(|| "Can't set perms on directory")?;

    fs::set_permissions(&bad_perms_file, Permissions::from_mode(0o400)) // read owner only
        .err_tip(|| "Can't set perms on file")?;

    remove_dir_all(&bad_perms_directory)
        .await
        .err_tip(|| format!("second remove_dir_all for {bad_perms_directory:?}"))?;

    assert!(!fs::exists(&bad_perms_directory)?);
    Ok(())
}
