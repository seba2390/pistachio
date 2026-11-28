//! Installing pi on a host automatically, the way editors install their remote server.
//!
//! Everything goes into `~/.pistachio/server` on the host ([`MANAGED_DIR`](crate::ssh::MANAGED_DIR)):
//! a standalone Node.js when the host has none recent enough, and pi from npm. Nothing needs
//! root, nothing outside that folder changes, and removing the folder undoes it.

use std::time::Duration;

use crate::RemoteError;
use crate::process;
use crate::ssh::{LOGIN_PATH, Ssh, script_result};

/// Downloads can be slow; give up after this long.
const INSTALL_TIMEOUT: Duration = Duration::from_mins(15);

const INSTALL_SCRIPT: &str = include_str!("scripts/install-pi.sh");

/// The installer script with its login-`PATH` prelude filled in: POSIX `sh` that installs pi into
/// `~/.pistachio/server` (also run on this computer by the `pi-install` crate).
pub fn install_script() -> String {
    INSTALL_SCRIPT.replace("LOGIN_PATH", LOGIN_PATH)
}

impl Ssh {
    /// Installs pi into `~/.pistachio/server` on the host (blocking; may take minutes) and returns
    /// the path of the installed `pi`. `on_step` receives short progress messages such as
    /// "Downloading Node.js".
    pub fn install_pi(&self, mut on_step: impl FnMut(&str)) -> Result<String, RemoteError> {
        let script = format!("{{\n{}\n}} 2>&1", install_script());
        let mut installed = None;
        let mut log: Vec<String> = Vec::new();
        let output =
            process::run_streaming(self.script_command(&script)?, INSTALL_TIMEOUT, |line| {
                if let Some(step) = line.strip_prefix("PISTACHIO_STEP=") {
                    on_step(step);
                } else if let Some(path) = line.strip_prefix("PISTACHIO_INSTALLED=") {
                    installed = Some(path.trim().to_owned());
                } else if !line.trim().is_empty() {
                    log.push(line.to_owned());
                }
            })?;
        let tail = log
            .iter()
            .rev()
            .take(8)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        match script_result(output) {
            Ok(_) => installed.ok_or_else(|| {
                RemoteError::Failed(format!("The installer did not finish.\n{tail}"))
            }),
            Err(RemoteError::Failed(stderr)) => Err(RemoteError::Failed(if tail.is_empty() {
                stderr
            } else {
                tail
            })),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_is_valid_sh() {
        let script = install_script();
        assert!(!script.contains("LOGIN_PATH"));
        let status = std::process::Command::new("/bin/sh")
            .args(["-n", "-c", &script])
            .status()
            .unwrap();
        assert!(status.success());
    }

    /// Installs into a scratch HOME with the machine's real network: slow, so ignored.
    #[test]
    #[ignore = "downloads Node.js and pi"]
    fn installs_into_a_scratch_home() {
        let home = tempfile::tempdir().unwrap();
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", &install_script()])
            .env("HOME", home.path())
            .env("SHELL", "/bin/sh")
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        eprintln!("{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success());
        assert!(stdout.contains("PISTACHIO_INSTALLED="));
        assert!(home.path().join(".pistachio/server/bin/pi").exists());
    }
}
