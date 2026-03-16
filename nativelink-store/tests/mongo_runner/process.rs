use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use nativelink_error::{Error, make_err};
use tonic::Code;
use tracing::info;
use uuid::Uuid;

use super::downloader::Os;

#[derive(Debug)]
pub(crate) struct MongoProcess {
    child: Child,
    pub connection_string: String,
}

impl MongoProcess {
    pub(crate) fn start(
        extracted_path: &Path,
        port: u16,
        db_path: &Path,
        os: &Os,
        bind_ip: &str,
        auth: bool,
        connection_string: String,
    ) -> Result<Self, Error> {
        let binary_name = match os {
            Os::Windows => "mongod.exe",
            _ => "mongod",
        };

        let binary_path = find_binary(extracted_path, binary_name).ok_or_else(|| {
            make_err!(
                Code::Internal,
                "Could not find {} in extracted directory",
                binary_name
            )
        })?;

        // Ensure binary is executable on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&binary_path)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&binary_path, perms)?;
        }

        if !db_path.exists() {
            std::fs::create_dir_all(db_path)?;
        }

        let mut command = Command::new(binary_path);
        command
            .arg("--port")
            .arg(port.to_string())
            .arg("--dbpath")
            .arg(db_path)
            .arg("--bind_ip")
            .arg(bind_ip);

        if auth {
            command.arg("--auth");
        }

        let log_path = db_path
            .join(format!("mongo-{}.log", Uuid::new_v4().as_simple()))
            .into_os_string();
        info!(?log_path, "Logging mongo");
        command.arg("--quiet").arg("--logpath").arg(log_path);

        let child = command.spawn()?;

        Ok(Self {
            child,
            connection_string,
        })
    }

    pub(crate) fn kill(&mut self) -> Result<(), Error> {
        self.child.kill()?;
        self.child.wait()?;
        Ok(())
    }
}

fn find_binary(root: &Path, name: &str) -> Option<PathBuf> {
    if root.is_file() {
        if root.file_name()?.to_str()? == name {
            return Some(root.to_path_buf());
        }
        return None;
    }

    if root.is_dir() {
        for entry in std::fs::read_dir(root).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            if let Some(found) = find_binary(&path, name) {
                return Some(found);
            }
        }
    }
    None
}
