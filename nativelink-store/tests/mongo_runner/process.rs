use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};

use nativelink_error::{Error, ResultExt, make_err};
use tonic::Code;
use tracing::info;
use uuid::Uuid;

use super::downloader::Os;

#[derive(Debug)]
pub(crate) struct MongoProcess {
    child: Child,
    pub connection_string: String,
    /// The TCP port this mongod was asked to bind (0 for unix sockets).
    pub port: u16,
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

            use tracing::trace;

            let mut perms = fs::metadata(&binary_path)
                .err_tip(|| format!("Getting metadata for {}", binary_path.display()))?
                .permissions();
            let target_mode = 0o555;
            if perms.mode() & target_mode == target_mode {
                trace!(
                    existing_permissions = perms.mode(),
                    target_mode,
                    ?binary_path,
                    "Permissions already set"
                );
            } else {
                trace!(
                    existing_permissions = perms.mode(),
                    target_mode,
                    ?binary_path,
                    "Setting permissions"
                );
                perms.set_mode(target_mode);
                fs::set_permissions(&binary_path, perms)
                    .err_tip(|| format!("Setting permissions for {}", binary_path.display()))?;
            }
        }

        if !db_path.exists() {
            fs::create_dir_all(db_path).err_tip(|| format!("Creating {}", db_path.display()))?;
        }

        let mut command = Command::new(&binary_path);
        command
            .arg("--port")
            .arg(port.to_string())
            .arg("--dbpath")
            .arg(db_path)
            .arg("--bind_ip")
            .arg(bind_ip)
            // Tests run many mongod instances concurrently; without a cap each
            // instance sizes its WiredTiger cache off total system memory
            // (~50% of RAM - 1GB), starving parallel tests.
            .arg("--wiredTigerCacheSizeGB")
            .arg("0.25");

        #[allow(clippy::case_sensitive_file_extension_comparisons)]
        let is_unix_socket = bind_ip.contains('/') || bind_ip.ends_with(".sock");
        if !is_unix_socket {
            // Skip the default /tmp/mongodb-<port>.sock; TCP is all we use and
            // sandboxed CI may not allow writing to /tmp.
            command.arg("--nounixsocket");
        }

        if auth {
            command.arg("--auth");
        }

        let log_path = db_path
            .join(format!("mongo-{}.log", Uuid::new_v4().as_simple()))
            .into_os_string();
        info!(?log_path, "Logging mongo");
        command.arg("--quiet").arg("--logpath").arg(log_path);

        let child = command
            .spawn()
            .err_tip(|| format!("Spawning mongo from {}", binary_path.display()))?;

        Ok(Self {
            child,
            connection_string,
            port,
        })
    }

    pub(crate) fn id(&self) -> u32 {
        self.child.id()
    }

    /// Returns `Some(status)` if the child has exited.
    pub(crate) fn try_wait(&mut self) -> Result<Option<ExitStatus>, Error> {
        Ok(self.child.try_wait()?)
    }

    pub(crate) fn kill(&mut self) -> Result<(), Error> {
        self.child.kill()?;
        self.child.wait()?;
        Ok(())
    }
}

impl Drop for MongoProcess {
    fn drop(&mut self) {
        // Kill the child on every exit path (a mongod left behind keeps its
        // port and memory for the rest of the test binary's life, or longer).
        // Never panic here: Drop also runs while unwinding from a failed test
        // assertion, and a double panic aborts the whole test binary.
        if let Ok(Some(_)) = self.child.try_wait() {
            return; // Already exited and reaped.
        }
        if let Err(e) = self.child.kill() {
            eprintln!("Failed to kill mongod: {e}");
        }
        if let Err(e) = self.child.wait() {
            eprintln!("Failed to reap mongod: {e}");
        }
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
        for entry in fs::read_dir(root).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            if let Some(found) = find_binary(&path, name) {
                return Some(found);
            }
        }
    }
    None
}
