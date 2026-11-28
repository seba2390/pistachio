//! Installs a newer release in place: the Update button of the update prompt.
//!
//! Everything that can fail runs while the app is still open ([`install`]): the download, its
//! checksum and putting the new version where the old one was (a running program's files
//! can be replaced on every platform). Only then does the app quit; a small helper
//! ([`relaunch`]) waits for it to exit and opens the new version, which shows what changed
//! ([`crate::whats_new`]). If anything fails, the old version is still there and the prompt
//! says what happened.
//!
//! - macOS: `brew upgrade --cask pistachio` when Homebrew installed the app; otherwise (or if
//!   that did not bring the new version) the release zip is unpacked next to the app and the
//!   two bundles are swapped.
//! - Linux: the .deb or .rpm through `pkexec` (the system asks for the password); a binary the
//!   user installed (the tarball's `install.sh`, or one unpacked anywhere) is replaced.
//! - Windows: the running `pistachio.exe` is renamed (Windows allows that, not overwriting
//!   it) and the new one takes its place; the old one is deleted at the next start.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use sha2::{Digest as _, Sha256};

use crate::updates::{Asset, CURRENT_VERSION, Release};

/// How this copy of Pistachio was installed, so it can be updated the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // each system only finds its own kinds
pub enum Install {
    /// macOS: an app bundle; `brew` is set when Homebrew's cask installed it.
    MacApp { app: PathBuf, brew: Option<PathBuf> },
    /// Linux: the Debian package (`/usr/bin/pistachio`).
    Deb,
    /// Linux: the RPM package (`/usr/bin/pistachio`).
    Rpm,
    /// Linux: a binary the user can write; `install_script` when it is where the tarball's
    /// `install.sh` puts it (which also refreshes the launcher entry and icon).
    LinuxBinary { exe: PathBuf, install_script: bool },
    /// Windows: the unpacked `pistachio.exe`.
    Windows { exe: PathBuf },
}

/// A step of [`install`], for the prompt.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Before the download, for as long as that takes (Homebrew looking for updates).
    Preparing(&'static str),
    /// Downloading: bytes so far and in all.
    Downloading { received: u64, total: u64 },
    /// After the download, without a measure of progress (unpacking, the password prompt).
    Installing(&'static str),
}

/// What [`relaunch`] opens once the app has quit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relaunch {
    /// The app bundle (macOS) or executable.
    program: PathBuf,
    /// The replaced version, deleted once the app has quit (macOS).
    remove: Option<PathBuf>,
}

/// Why this copy can't update itself, in a sentence for the prompt (it then offers the
/// download in the browser), or how it was installed.
pub fn detect() -> Result<Install, String> {
    #[cfg(debug_assertions)]
    if std::env::var_os("PISTACHIO_PRETEND_VERSION").is_none() {
        return Err("A development build updates with cargo.".into());
    }
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|error| format!("Could not find Pistachio's own file: {error}"))?;
    detect_for(&exe)
}

#[cfg(target_os = "macos")]
fn detect_for(exe: &Path) -> Result<Install, String> {
    let app = exe
        .ancestors()
        .find(|path| path.extension().is_some_and(|ext| ext == "app"))
        .ok_or("Pistachio is not running from an app bundle.")?
        .to_owned();
    // Only the copy the cask installed (in its default Applications folder): `brew upgrade`
    // would leave any other copy as it is, and update that one instead.
    let home_apps = std::env::var_os("HOME").map(|home| Path::new(&home).join("Applications"));
    let cask_copy = app
        .parent()
        .is_some_and(|dir| dir == Path::new("/Applications") || home_apps.as_deref() == Some(dir));
    let brew = ["/opt/homebrew", "/usr/local"].iter().find_map(|prefix| {
        let prefix = Path::new(prefix);
        let brew = prefix.join("bin/brew");
        (cask_copy && prefix.join("Caskroom/pistachio").is_dir() && brew.is_file()).then_some(brew)
    });
    Ok(Install::MacApp { app, brew })
}

#[cfg(all(unix, not(target_os = "macos")))]
fn detect_for(exe: &Path) -> Result<Install, String> {
    if exe.starts_with("/usr") {
        let owns = |program: &str, args: &[&str]| {
            Command::new(program)
                .args(args)
                .arg(exe)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        };
        if owns("dpkg-query", &["-S"]) {
            return Ok(Install::Deb);
        }
        if owns("rpm", &["-qf"]) {
            return Ok(Install::Rpm);
        }
        return Err(format!(
            "{} was installed by a package manager Pistachio does not know.",
            exe.display()
        ));
    }
    let dir = exe.parent().ok_or("Pistachio's folder is unknown.")?;
    if !writable(dir) {
        return Err(format!("Pistachio can't write to {}.", dir.display()));
    }
    let script_bin = std::env::var_os("XDG_BIN_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/bin")))
        .and_then(|bin| std::fs::canonicalize(bin).ok());
    Ok(Install::LinuxBinary {
        exe: exe.to_owned(),
        install_script: script_bin.as_deref() == Some(dir) && exe.ends_with("pistachio"),
    })
}

#[cfg(windows)]
fn detect_for(exe: &Path) -> Result<Install, String> {
    let dir = exe.parent().ok_or("Pistachio's folder is unknown.")?;
    if !writable(dir) {
        return Err(format!("Pistachio can't write to {}.", dir.display()));
    }
    Ok(Install::Windows {
        exe: exe.to_owned(),
    })
}

#[cfg(not(any(unix, windows)))]
fn detect_for(_: &Path) -> Result<Install, String> {
    Err("Updating is not supported on this system.".into())
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".pistachio-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Downloads `release` and puts it where this copy is (see the module docs). Blocks for as
/// long as that takes; run it in the background. The app keeps working meanwhile.
pub fn install(
    how: &Install,
    release: &Release,
    progress: &(dyn Fn(Step) + Sync),
) -> Result<Relaunch, String> {
    let version = release.version.as_str();
    match how {
        Install::MacApp { app, brew } => install_mac(app, brew.as_deref(), release, progress),
        Install::Deb => {
            let arch = output("dpkg", &["--print-architecture"])?;
            let name = format!("pistachio_{version}-1_{arch}.deb");
            let package = download_to_temp(release, &name, progress)?;
            progress(Step::Installing("Waiting for your password…"));
            let path = package.to_string_lossy();
            root(&["apt-get", "install", "-y", "--allow-downgrades", &path]).map_err(|error| {
                with_command(
                    &error,
                    &format!(
                        "curl -fLO {url} && sudo apt install ./{name}",
                        url = release.asset(&name).map_or("", |asset| &asset.url)
                    ),
                )
            })?;
            Ok(relaunch_exe("/usr/bin/pistachio"))
        }
        Install::Rpm => {
            let arch = output("uname", &["-m"])?;
            let name = format!("pistachio-{version}-1.{arch}.rpm");
            let package = download_to_temp(release, &name, progress)?;
            progress(Step::Installing("Waiting for your password…"));
            let path = package.to_string_lossy();
            let installed = if on_path("dnf") {
                root(&["dnf", "install", "-y", &path])
            } else {
                root(&["rpm", "-U", "--force", &path])
            };
            installed.map_err(|error| {
                let url = release.asset(&name).map_or("", |asset| &asset.url);
                with_command(&error, &format!("sudo dnf install {url}"))
            })?;
            Ok(relaunch_exe("/usr/bin/pistachio"))
        }
        Install::LinuxBinary {
            exe,
            install_script,
        } => install_linux_binary(exe, *install_script, release, progress),
        Install::Windows { exe } => install_windows(exe, release, progress),
    }
}

/// Opens the new version once this process has exited: a helper that outlives the app waits
/// for it, deletes what [`install`] replaced, and starts the new version. Call it right
/// before quitting.
pub fn relaunch(relaunch: &Relaunch) -> Result<(), String> {
    let pid = std::process::id().to_string();
    let spawned = spawn_relaunch(relaunch, &pid);
    spawned.map_err(|error| format!("Could not restart Pistachio: {error}"))
}

#[cfg(unix)]
fn spawn_relaunch(relaunch: &Relaunch, pid: &str) -> std::io::Result<()> {
    spawn_unix_relaunch(relaunch, pid, cfg!(target_os = "macos"))
}

/// `open`: start an app bundle with `open` (macOS) rather than run the program.
#[cfg(unix)]
fn spawn_unix_relaunch(relaunch: &Relaunch, pid: &str, open: bool) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt as _;
    // Positional parameters: nothing in a path is read by the shell.
    // `open` starts the app from launchd, without this environment: a pi settings folder
    // chosen with `PI_CODING_AGENT_DIR` is handed on.
    let start = if open {
        r#"if [ -n "$4" ]; then exec /usr/bin/open --env "PI_CODING_AGENT_DIR=$4" -- "$2"; else exec /usr/bin/open -- "$2"; fi"#
    } else {
        r#"exec "$2""#
    };
    let script = format!(
        r#"while kill -0 "$1" 2>/dev/null; do sleep 0.1; done; [ -n "$3" ] && rm -rf -- "$3"; {start}"#
    );
    let remove = relaunch
        .remove
        .as_ref()
        .map_or_else(Default::default, |path| path.as_os_str().to_owned());
    Command::new("/bin/sh")
        .args(["-c", &script, "pistachio-relaunch", pid])
        .arg(&relaunch.program)
        .arg(remove)
        .arg(std::env::var_os("PI_CODING_AGENT_DIR").unwrap_or_default())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Its own process group, so it is not stopped with the app.
        .process_group(0)
        .spawn()
        .map(drop)
}

#[cfg(windows)]
fn spawn_relaunch(relaunch: &Relaunch, pid: &str) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let program = relaunch.program.to_string_lossy().replace('\'', "''");
    let script = format!(
        "Wait-Process -Id {pid} -ErrorAction SilentlyContinue; Start-Process -FilePath '{program}'"
    );
    Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &script,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()
        .map(drop)
}

#[cfg(not(any(unix, windows)))]
fn spawn_relaunch(_: &Relaunch, _: &str) -> std::io::Result<()> {
    Err(std::io::Error::other("not supported on this system"))
}

/// Deletes what an earlier update left behind (the old `pistachio.exe` on Windows, an
/// interrupted download next to the macOS app). At start, in the background.
pub fn clean_up() {
    let Ok(exe) = std::env::current_exe().and_then(std::fs::canonicalize) else {
        return;
    };
    if cfg!(windows) {
        let _ = std::fs::remove_file(exe.with_file_name("pistachio.exe.old"));
    }
    if cfg!(target_os = "macos")
        && let Some(parent) = exe
            .ancestors()
            .find(|path| path.extension().is_some_and(|ext| ext == "app"))
            .and_then(Path::parent)
        && let Ok(entries) = std::fs::read_dir(parent)
    {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(".Pistachio-")
                && (name.ends_with(".update") || name.ends_with(".old.app"))
            {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    let _ = std::fs::remove_dir_all(temp_dir());
}

// ---- macOS -----------------------------------------------------------------------------

fn install_mac(
    app: &Path,
    brew: Option<&Path>,
    release: &Release,
    progress: &(dyn Fn(Step) + Sync),
) -> Result<Relaunch, String> {
    let version = release.version.as_str();
    if let Some(brew) = brew {
        progress(Step::Preparing("Updating with Homebrew…"));
        let mut command = Command::new(brew);
        command
            .args(["upgrade", "--cask", "pistachio"])
            .env("HOMEBREW_NO_ENV_HINTS", "1")
            .env(
                "PATH",
                format!(
                    "{}:/usr/bin:/bin:/usr/sbin:/sbin",
                    brew.parent().unwrap_or(Path::new("/")).display()
                ),
            );
        // Homebrew prints no progress to a pipe, so its download is measured in its cache.
        let name = mac_asset_name(version);
        let total = release.asset(&name).map_or(0, |asset| asset.size);
        let mut watch =
            brew_downloads(brew).map(|downloads| BrewDownload::new(downloads, &name, total));
        let poll = || {
            if let Some(step) = watch.as_mut().and_then(BrewDownload::poll) {
                progress(step);
            }
        };
        match run_polling(command, Duration::from_secs(600), poll) {
            Ok(_) if bundle_version(app).as_deref() == Some(version) => {
                return Ok(Relaunch {
                    program: app.to_owned(),
                    remove: None,
                });
            }
            Ok(_) => log::warn!(
                "brew upgrade did not install Pistachio {version}; installing the download"
            ),
            Err(error) => log::warn!("brew upgrade failed ({error}); installing the download"),
        }
    }
    let name = mac_asset_name(version);
    let parent = app.parent().ok_or("The app's folder is unknown.")?;
    // Next to the app, so the swap is a rename on the same disk.
    let stage = parent.join(format!(".Pistachio-{version}.update"));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir(&stage).map_err(|error| {
        format!(
            "Pistachio can't write to {} ({error}). Update with the download instead.",
            parent.display()
        )
    })?;
    let result = (|| {
        let zip = stage.join(&name);
        download(release, &name, &zip, progress)?;
        progress(Step::Installing("Installing…"));
        let mut unzip = Command::new("/usr/bin/ditto");
        unzip.args(["-x", "-k"]).arg(&zip).arg(&stage);
        run(unzip, Duration::from_secs(300)).map_err(|error| format!("Unpacking: {error}"))?;
        let new_app = stage.join("Pistachio.app");
        let found = bundle_version(&new_app);
        if found.as_deref() != Some(version) {
            return Err(format!(
                "The download holds {} instead of Pistachio {version}.",
                found.map_or("no app".to_owned(), |v| format!("version {v}"))
            ));
        }
        // Not downloaded by a browser, so not quarantined; but the release notes ask for it.
        let mut xattr = Command::new("/usr/bin/xattr");
        xattr.args(["-dr", "com.apple.quarantine"]).arg(&new_app);
        let _ = run(xattr, Duration::from_secs(60));
        let old = parent.join(format!(".Pistachio-{CURRENT_VERSION}.old.app"));
        let _ = std::fs::remove_dir_all(&old);
        std::fs::rename(app, &old).map_err(|error| format!("Moving the old version: {error}"))?;
        if let Err(error) = std::fs::rename(&new_app, app) {
            let _ = std::fs::rename(&old, app);
            return Err(format!("Moving the new version in place: {error}"));
        }
        Ok(Relaunch {
            program: app.to_owned(),
            remove: Some(old),
        })
    })();
    let _ = std::fs::remove_dir_all(&stage);
    result
}

/// The release's file for macOS: Apple Silicon only since 0.4.7 (0.4.6 was the last universal
/// build).
fn mac_asset_name(version: &str) -> String {
    format!("Pistachio-v{version}-macos-arm64.zip")
}

/// The folder Homebrew downloads to.
fn brew_downloads(brew: &Path) -> Option<PathBuf> {
    let mut command = Command::new(brew);
    command.arg("--cache");
    let cache = run(command, Duration::from_secs(10)).ok()?;
    let cache = cache.trim();
    (!cache.is_empty()).then(|| Path::new(cache).join("downloads"))
}

/// Homebrew's download of the file `name`: it grows in `downloads` as
/// `<hash of the URL>--<name>.incomplete`, and loses the `.incomplete` when it is done.
struct BrewDownload {
    downloads: PathBuf,
    suffix: String,
    total: u64,
    /// What an interrupted download left; Homebrew continues it, or starts over.
    left_over: Option<u64>,
    /// The size last reported.
    reported: Option<u64>,
}

impl BrewDownload {
    fn new(downloads: PathBuf, name: &str, total: u64) -> Self {
        let mut watch = Self {
            downloads,
            suffix: format!("--{name}.incomplete"),
            total,
            left_over: None,
            reported: None,
        };
        watch.left_over = watch.size();
        watch
    }

    /// The size of the unfinished file, while there is one.
    fn size(&self) -> Option<u64> {
        std::fs::read_dir(&self.downloads)
            .ok()?
            .flatten()
            .find(|entry| entry.file_name().to_string_lossy().ends_with(&self.suffix))
            .and_then(|entry| entry.metadata().ok())
            .map(|metadata| metadata.len())
    }

    /// The step to show, if there is a new one.
    fn poll(&mut self) -> Option<Step> {
        let Some(received) = self.size() else {
            self.left_over = None;
            // It was being downloaded and is there now: Homebrew installs it.
            return self
                .reported
                .take()
                .map(|_| Step::Installing("Installing…"));
        };
        // Not touched yet: Homebrew is still looking for updates.
        if self.left_over == Some(received) || self.reported == Some(received) {
            return None;
        }
        self.left_over = None;
        self.reported = Some(received);
        Some(Step::Downloading {
            received,
            total: self.total,
        })
    }
}

/// `CFBundleShortVersionString` of an app bundle.
fn bundle_version(app: &Path) -> Option<String> {
    let plist = app.join("Contents/Info.plist");
    let mut command = Command::new("/usr/libexec/PlistBuddy");
    command
        .args(["-c", "Print :CFBundleShortVersionString"])
        .arg(plist);
    run(command, Duration::from_secs(10))
        .ok()
        .map(|out| out.trim().to_owned())
}

// ---- Linux -----------------------------------------------------------------------------

fn install_linux_binary(
    exe: &Path,
    install_script: bool,
    release: &Release,
    progress: &(dyn Fn(Step) + Sync),
) -> Result<Relaunch, String> {
    let version = release.version.as_str();
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    let folder = format!("pistachio-v{version}-linux-{arch}");
    let name = format!("{folder}.tar.gz");
    let archive = download_to_temp(release, &name, progress)?;
    progress(Step::Installing("Installing…"));
    let dir = archive.parent().ok_or("no download folder")?;
    let mut untar = Command::new("tar");
    untar.arg("-xzf").arg(&archive).arg("-C").arg(dir);
    run(untar, Duration::from_secs(300)).map_err(|error| format!("Unpacking: {error}"))?;
    let unpacked = dir.join(&folder);
    if install_script {
        let mut script = Command::new("/bin/sh");
        script.arg(unpacked.join("install.sh"));
        run(script, Duration::from_secs(120)).map_err(|error| format!("install.sh: {error}"))?;
    } else {
        replace_file(&unpacked.join("pistachio"), exe)?;
    }
    Ok(Relaunch {
        program: exe.to_owned(),
        remove: None,
    })
}

#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
fn relaunch_exe(path: &str) -> Relaunch {
    Relaunch {
        program: PathBuf::from(path),
        remove: None,
    }
}

/// Runs `argv` as root through `pkexec`, which asks for the password in a system dialog.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
fn root(argv: &[&str]) -> Result<String, String> {
    if !on_path("pkexec") {
        return Err("Installing needs the administrator password, and pkexec is missing.".into());
    }
    let mut command = Command::new("pkexec");
    command.args(argv);
    run(command, Duration::from_mins(15)).map_err(|error| {
        if error.contains("exit status: 126") || error.contains("exit status: 127") {
            "The password prompt was dismissed.".to_owned()
        } else {
            error
        }
    })
}

#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
fn with_command(error: &str, command: &str) -> String {
    format!("{error}\nInstall it from a terminal instead:\n{command}")
}

/// Copies `from` over `to` without writing into `to` (which may be running): a temporary file
/// next to it is renamed over it.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn replace_file(from: &Path, to: &Path) -> Result<(), String> {
    let name = to
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    let tmp = to.with_file_name(format!(".{}.new", name.unwrap_or_default()));
    std::fs::copy(from, &tmp).map_err(|error| format!("Copying the new version: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }
    std::fs::rename(&tmp, to).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        format!("Moving the new version in place: {error}")
    })
}

// ---- Windows ---------------------------------------------------------------------------

#[cfg_attr(not(windows), allow(dead_code))]
fn install_windows(
    exe: &Path,
    release: &Release,
    progress: &(dyn Fn(Step) + Sync),
) -> Result<Relaunch, String> {
    let version = release.version.as_str();
    let folder = format!("pistachio-v{version}-windows-x86_64");
    let name = format!("{folder}.zip");
    let archive = download_to_temp(release, &name, progress)?;
    progress(Step::Installing("Installing…"));
    let dir = archive.parent().ok_or("no download folder")?;
    // tar.exe has come with Windows since 10 (1803) and reads zip files.
    let mut untar = ::platform::command("tar");
    untar.arg("-xf").arg(&archive).arg("-C").arg(dir);
    run(untar, Duration::from_secs(300)).map_err(|error| format!("Unpacking: {error}"))?;
    let unpacked = dir.join(&folder);
    let new_exe = unpacked.join("pistachio.exe");
    if !new_exe.is_file() {
        return Err("The download holds no pistachio.exe.".into());
    }
    let staged = exe.with_file_name("pistachio.exe.new");
    std::fs::copy(&new_exe, &staged)
        .map_err(|error| format!("Copying the new version: {error}"))?;
    let old = exe.with_file_name("pistachio.exe.old");
    let _ = std::fs::remove_file(&old);
    // A running program can be renamed, not overwritten.
    std::fs::rename(exe, &old).map_err(|error| format!("Moving the old version: {error}"))?;
    if let Err(error) = std::fs::rename(&staged, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(format!("Moving the new version in place: {error}"));
    }
    let licenses = unpacked.join("THIRD-PARTY-LICENSES.txt");
    if licenses.is_file() {
        let _ = std::fs::copy(licenses, exe.with_file_name("THIRD-PARTY-LICENSES.txt"));
    }
    Ok(Relaunch {
        program: exe.to_owned(),
        remove: None,
    })
}

// ---- shared ----------------------------------------------------------------------------

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join("pistachio-update")
}

/// Downloads the asset `name` into a fresh temporary folder and returns its path.
fn download_to_temp(
    release: &Release,
    name: &str,
    progress: &(dyn Fn(Step) + Sync),
) -> Result<PathBuf, String> {
    let dir = temp_dir().join(&release.version);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("Creating {}: {error}", dir.display()))?;
    let path = dir.join(name);
    download(release, name, &path, progress)?;
    Ok(path)
}

/// Downloads the asset `name` of `release` to `path` and checks it against the size and
/// checksum GitHub lists.
fn download(
    release: &Release,
    name: &str,
    path: &Path,
    progress: &(dyn Fn(Step) + Sync),
) -> Result<(), String> {
    let asset = release.asset(name).ok_or_else(|| {
        format!(
            "Pistachio {} has no {name} to download for this system.",
            release.version
        )
    })?;
    download_asset(asset, path, progress)
}

fn download_asset(
    asset: &Asset,
    path: &Path,
    progress: &(dyn Fn(Step) + Sync),
) -> Result<(), String> {
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .user_agent(concat!("pistachio/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent();
    let mut response = agent
        .get(&asset.url)
        .call()
        .map_err(|error| format!("Downloading {}: {error}", asset.name))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(format!(
            "Downloading {}: GitHub answered with status {status}",
            asset.name
        ));
    }
    // Releases are tens of megabytes; this only stops a runaway answer.
    let limit = if asset.size > 0 {
        asset.size + 1
    } else {
        1 << 30
    };
    let mut reader = response.body_mut().with_config().limit(limit).reader();
    let mut out = std::fs::File::create(path)
        .map_err(|error| format!("Creating {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 256 * 1024];
    let mut received = 0_u64;
    let mut reported = Instant::now();
    progress(Step::Downloading {
        received,
        total: asset.size,
    });
    loop {
        let n = reader
            .read(&mut buffer)
            .map_err(|error| format!("Downloading {}: {error}", asset.name))?;
        if n == 0 {
            break;
        }
        out.write_all(&buffer[..n])
            .map_err(|error| format!("Writing {}: {error}", path.display()))?;
        hasher.update(&buffer[..n]);
        received += n as u64;
        if reported.elapsed() >= Duration::from_millis(100) {
            progress(Step::Downloading {
                received,
                total: asset.size,
            });
            reported = Instant::now();
        }
    }
    out.flush()
        .map_err(|error| format!("Writing {}: {error}", path.display()))?;
    drop(out);
    progress(Step::Downloading {
        received,
        total: asset.size,
    });
    let size_ok = asset.size == 0 || received == asset.size;
    let hash = hex(&hasher.finalize());
    let hash_ok = asset.sha256().is_none_or(|expected| expected == hash);
    if !size_ok || !hash_ok {
        let _ = std::fs::remove_file(path);
        return Err(format!(
            "{} did not match its checksum; nothing was changed. Try again.",
            asset.name
        ));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// The trimmed output of a short command.
#[cfg_attr(not(all(unix, not(target_os = "macos"))), allow(dead_code))]
fn output(program: &str, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new(program);
    command.args(args);
    run(command, Duration::from_secs(10)).map(|out| out.trim().to_owned())
}

/// Runs `command` with no input, waiting at most `timeout`; its standard output, or why it
/// failed (with the last lines it printed).
fn run(command: Command, timeout: Duration) -> Result<String, String> {
    run_polling(command, timeout, || {})
}

/// [`run`], calling `poll` every now and then while the command runs.
fn run_polling(
    mut command: Command,
    timeout: Duration,
    mut poll: impl FnMut(),
) -> Result<String, String> {
    let name = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not run {name}: {error}"))?;
    // Read both pipes while waiting, so a chatty command never blocks on a full pipe.
    let stdout = child.stdout.take().map(read_in_background);
    let stderr = child.stderr.take().map(read_in_background);
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{name} took too long"));
            }
            Ok(None) => {
                poll();
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(format!("{name}: {error}")),
        }
    };
    let collect = |handle: Option<std::thread::JoinHandle<String>>| {
        handle.and_then(|h| h.join().ok()).unwrap_or_default()
    };
    let (stdout, stderr) = (collect(stdout), collect(stderr));
    if status.success() {
        return Ok(stdout);
    }
    let tail: Vec<&str> = stderr
        .lines()
        .chain(stdout.lines())
        .filter(|line| !line.trim().is_empty())
        .collect();
    let tail = tail[tail.len().saturating_sub(4)..].join("\n");
    log::warn!("{name} failed ({status}): {stderr}{stdout}");
    Err(if tail.is_empty() {
        format!("{name} failed ({status})")
    } else {
        format!("{name} failed ({status}): {tail}")
    })
}

fn read_in_background(
    mut pipe: impl std::io::Read + Send + 'static,
) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(assets: Vec<Asset>) -> Release {
        Release {
            version: "9.9.9".into(),
            page_url: String::new(),
            download_url: None,
            notes: String::new(),
            assets,
        }
    }

    /// Serves `body` once on a loopback port; the URL to it.
    fn serve(body: &'static [u8]) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut request = [0_u8; 1024];
                let _ = std::io::Read::read(&mut stream, &mut request);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(body);
            }
        });
        format!("http://127.0.0.1:{port}/file")
    }

    #[test]
    fn downloads_are_checked_against_githubs_checksum() {
        let url = serve(b"hello");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file");
        let good = Asset {
            name: "file".into(),
            url: url.clone(),
            size: 5,
            // sha256("hello")
            digest: Some(
                "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824".into(),
            ),
        };
        let steps = std::sync::Mutex::new(Vec::new());
        download(&release(vec![good.clone()]), "file", &path, &|step| {
            steps.lock().unwrap().push(step);
        })
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
        assert_eq!(
            steps.lock().unwrap().last(),
            Some(&Step::Downloading {
                received: 5,
                total: 5
            })
        );

        let bad = Asset {
            digest: Some(format!("sha256:{}", "0".repeat(64))),
            ..good
        };
        let error = download(&release(vec![bad]), "file", &path, &|_| {}).unwrap_err();
        assert!(error.contains("checksum"), "{error}");
        assert!(!path.exists(), "a download that does not match is deleted");

        let error = download(&release(Vec::new()), "file", &path, &|_| {}).unwrap_err();
        assert!(error.contains("no file to download"), "{error}");
    }

    #[test]
    fn homebrews_download_is_measured_in_its_cache() {
        let dir = tempfile::tempdir().unwrap();
        let name = mac_asset_name("9.9.9");
        let part = dir.path().join(format!("abc--{name}.incomplete"));
        let downloading = |received| {
            Some(Step::Downloading {
                received,
                total: 100,
            })
        };
        let watch = || BrewDownload::new(dir.path().to_owned(), &name, 100);

        let mut download = watch();
        assert_eq!(download.poll(), None, "Homebrew has not started it");
        // Another version, and a finished download, are not this one.
        std::fs::write(
            dir.path().join("abc--Pistachio-v9.9.8.zip.incomplete"),
            "other",
        )
        .unwrap();
        std::fs::write(dir.path().join(format!("abc--{name}")), "done").unwrap();
        assert_eq!(download.poll(), None);
        std::fs::write(&part, [0; 10]).unwrap();
        assert_eq!(download.poll(), downloading(10));
        assert_eq!(download.poll(), None, "nothing new");
        std::fs::write(&part, [0; 60]).unwrap();
        assert_eq!(download.poll(), downloading(60));
        std::fs::remove_file(&part).unwrap();
        assert_eq!(download.poll(), Some(Step::Installing("Installing…")));
        assert_eq!(download.poll(), None);

        // What an interrupted download left counts once Homebrew continues it.
        std::fs::write(&part, [0; 60]).unwrap();
        let mut download = watch();
        assert_eq!(download.poll(), None);
        std::fs::write(&part, [0; 70]).unwrap();
        assert_eq!(download.poll(), downloading(70));
    }

    #[cfg(unix)]
    #[test]
    fn a_running_command_is_polled() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 0.5; echo out"]);
        let mut polls = 0;
        let out = run_polling(command, Duration::from_secs(20), || polls += 1).unwrap();
        assert_eq!(out.trim(), "out");
        assert!(polls >= 2, "{polls}");
    }

    /// Watches Homebrew download the installed version again:
    /// `cargo test -p pistachio brew_fetch -- --ignored --nocapture`.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "needs Homebrew, the tap and the network"]
    fn brew_fetch_reports_its_progress() {
        let brew = Path::new("/opt/homebrew/bin/brew");
        let mut info = Command::new(brew);
        info.args(["--cache", "--cask", "pistachio"]);
        let cached = PathBuf::from(run(info, Duration::from_secs(60)).unwrap().trim());
        let file = cached.file_name().unwrap().to_string_lossy().into_owned();
        let name = file.split_once("--").unwrap().1;
        let mut fetch = Command::new(brew);
        fetch.args(["fetch", "--force", "--cask", "pistachio"]);
        let mut watch = BrewDownload::new(brew_downloads(brew).unwrap(), name, 0);
        let mut steps = Vec::new();
        run_polling(fetch, Duration::from_secs(300), || {
            steps.extend(watch.poll());
        })
        .unwrap();
        println!("{steps:#?}");
        assert!(matches!(steps.first(), Some(Step::Downloading { .. })));
        assert_eq!(steps.last(), Some(&Step::Installing("Installing…")));
    }

    #[cfg(unix)]
    #[test]
    fn replacing_a_file_keeps_it_executable() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let (new, old) = (dir.path().join("new"), dir.path().join("pistachio"));
        std::fs::write(&new, "v2").unwrap();
        std::fs::write(&old, "v1").unwrap();
        replace_file(&new, &old).unwrap();
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "v2");
        assert_eq!(
            std::fs::metadata(&old).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(!dir.path().join(".pistachio.new").exists());
    }

    #[cfg(unix)]
    #[test]
    fn relaunch_waits_for_the_app_to_exit() {
        // A stand-in "app" that records it was started, and a short-lived process standing
        // in for this one (an exited child counts as running until it is reaped).
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("started");
        let program = dir.path().join("app");
        std::fs::write(
            &program,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let old = dir.path().join("old");
        std::fs::create_dir(&old).unwrap();
        let mut waited_for = Command::new("sleep").arg("0.5").spawn().unwrap();
        spawn_unix_relaunch(
            &Relaunch {
                program: program.clone(),
                remove: Some(old.clone()),
            },
            &waited_for.id().to_string(),
            false,
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert!(!marker.exists(), "not before the app has exited");
        waited_for.wait().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(marker.exists(), "started once the app exited");
        assert!(!old.exists(), "the old version is deleted");
    }
}
