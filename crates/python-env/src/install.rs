//! Installing uv, a Python and packages.

use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;

use crate::catalog::{self, Platform};
use crate::fetch::{ChecksumOf, download_tar, unpack};
use crate::run;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    Cancelled,
    Failed(String),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("cancelled"),
            Self::Failed(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for InstallError {}

/// Says which file a step failed at.
pub fn io<'a>(what: &'a str, path: &'a Path) -> impl FnOnce(std::io::Error) -> InstallError + 'a {
    move |error| InstallError::Failed(format!("{what} {} failed: {error}", path.display()))
}

/// Deletes a folder, if it is there.
pub fn remove(dir: &Path) -> std::io::Result<()> {
    match fs::remove_dir_all(dir) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The program of the Python in `dir` ([`Tools::python`]).
pub fn python_in(dir: &Path) -> PathBuf {
    dir.join("bin").join("python3")
}

/// uv in a folder of its own, which also holds what it caches: delete it when done.
pub struct Tools<'a> {
    dir: PathBuf,
    cancel: &'a AtomicBool,
}

impl<'a> Tools<'a> {
    /// Downloads uv into `dir`. Reports the bytes received so far.
    pub fn download(
        dir: &Path,
        platform: &Platform,
        cancel: &'a AtomicBool,
        progress: &mut dyn FnMut(u64),
    ) -> Result<Self, InstallError> {
        fs::create_dir_all(dir).map_err(io("Creating", dir))?;
        let tar = download_tar(
            &platform.uv_url(),
            platform.uv_sha256,
            ChecksumOf::Compressed,
            cancel,
            progress,
        )?;
        unpack(&tar, dir, |path| path == Path::new("uv")).map_err(io("Unpacking", dir))?;
        Ok(Self {
            dir: dir.to_path_buf(),
            cancel,
        })
    }

    /// Downloads Python into `target`, which must not exist yet.
    pub fn python(&self, target: &Path) -> Result<(), InstallError> {
        let pythons = self.dir.join("pythons");
        self.run(
            self.uv()
                .args(["python", "install", "--no-bin", "--install-dir"])
                .arg(&pythons)
                .arg(catalog::PYTHON_VERSION),
            "Downloading Python",
            &mut |_| {},
        )?;
        let python = only_python(&pythons).ok_or_else(|| {
            InstallError::Failed("Downloading Python failed: it is not where it should be".into())
        })?;
        fs::rename(&python, target).map_err(io("Installing", target))
    }

    /// Downloads the packages `requirements` locks into `target`, for the Python in `python`.
    /// Each line uv says about them goes to `line`.
    pub fn packages(
        &self,
        python: &Path,
        requirements: &str,
        target: &Path,
        line: &mut dyn FnMut(&str),
    ) -> Result<(), InstallError> {
        let file = self.dir.join("requirements.txt");
        fs::write(&file, requirements).map_err(io("Writing", &file))?;
        self.run(
            self.uv()
                .args(["pip", "sync", "--require-hashes", "--only-binary", ":all:"])
                .arg("--python")
                .arg(python_in(python))
                .arg("--target")
                .arg(target)
                .arg(&file),
            "Downloading the Python packages",
            line,
        )
    }

    fn run(
        &self,
        command: &mut Command,
        what: &str,
        line: &mut dyn FnMut(&str),
    ) -> Result<(), InstallError> {
        run(command, what, self.cancel, line)
    }

    /// uv, kept away from the user's own uv, Pythons and settings.
    fn uv(&self) -> Command {
        let tools = &self.dir;
        let mut command = platform::command(tools.join("uv"));
        command
            .env_clear()
            .envs(crate::network_env())
            .env("HOME", tools)
            // On a Mac uv looks for a developer tool it can do without, and finding Apple's
            // placeholder for it would open a window offering to install them.
            .env(
                "PATH",
                if cfg!(target_os = "macos") {
                    tools.join("none").into_os_string()
                } else {
                    OsStr::new("/usr/bin:/bin").to_owned()
                },
            )
            .env("UV_NO_CONFIG", "1")
            .env("UV_CACHE_DIR", tools.join("cache"))
            .env("UV_PYTHON_INSTALL_DIR", tools.join("pythons"))
            .env("UV_PYTHON_PREFERENCE", "only-managed")
            .env("UV_NO_PROGRESS", "1")
            .current_dir(tools)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        command
    }
}

/// The Python folder in `dir` (beside it is a link with a shorter name).
fn only_python(dir: &Path) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry.file_name().to_string_lossy().starts_with("cpython-")
        })
        .map(|entry| entry.path())
}
