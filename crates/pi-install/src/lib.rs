//! pi on this computer: finding it, checking it, and installing it when it is not there.
//!
//! - [`find`] looks on the `PATH` (the app's and the login shell's), then where Pistachio
//!   installs it and where npm, Node.js version managers and package managers put global
//!   commands ([`known_places`]).
//! - [`check`] runs `pi --version`.
//! - [`install`] installs it without prompts or administrator rights into
//!   `~/.pistachio/server`, with a Node.js of its own when there is no recent one: on macOS and
//!   Linux with the same script as on SSH hosts ([`remote::install_script`]), on Windows with
//!   Windows' own `curl.exe` and `tar.exe`. Removing that folder undoes it.

use std::ffi::{OsStr, OsString};
use std::io::{BufRead as _, BufReader, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use platform::NoWindow as _;

#[cfg(windows)]
mod windows;

/// Installing can be slow (downloads); give up after this long.
#[cfg(not(windows))]
const INSTALL_TIMEOUT: Duration = Duration::from_mins(15);
/// `pi --version` starts Node.js, which can take a while the first time.
const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// The names `pi` has as a file.
fn names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["pi.exe", "pi.cmd", "pi.bat"]
    } else {
        &["pi"]
    }
}

/// The folders where a computer keeps things, for [`known_places`].
#[derive(Debug, Clone, Default)]
pub struct Folders {
    pub home: Option<PathBuf>,
    /// Windows: `%APPDATA%` (roaming).
    pub app_data: Option<PathBuf>,
    /// Windows: `%LOCALAPPDATA%`.
    pub local_app_data: Option<PathBuf>,
    /// Windows: `%ProgramFiles%`.
    pub program_files: Option<PathBuf>,
    /// Windows: nvm-windows' link to the Node.js in use (`%NVM_SYMLINK%`).
    pub nvm_symlink: Option<PathBuf>,
}

impl Folders {
    /// This computer's.
    pub fn here() -> Self {
        let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
        Self {
            home: dirs::home_dir(),
            app_data: var("APPDATA"),
            local_app_data: var("LOCALAPPDATA"),
            program_files: var("ProgramFiles"),
            nvm_symlink: var("NVM_SYMLINK"),
        }
    }
}

/// Where pi is on this computer: on `path` (a `PATH` value, like the login shell's), on this
/// process' `PATH`, or in one of the [`known_places`]. Only looks at files; nothing is run.
pub fn find(path: Option<&OsStr>) -> Option<PathBuf> {
    let paths: Vec<OsString> = path
        .map(OsStr::to_owned)
        .into_iter()
        .chain(std::env::var_os("PATH"))
        .collect();
    paths.iter().find_map(|path| on_path(path)).or_else(|| {
        known_places(&Folders::here(), cfg!(windows))
            .into_iter()
            .find(|file| is_program(file))
    })
}

/// `pi` in one of the folders of the `PATH` value `path`.
pub fn on_path(path: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path).find_map(|dir| {
        names()
            .iter()
            .map(|name| dir.join(name))
            .find(|file| is_program(file))
    })
}

fn is_program(file: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.is_file()
            && file
                .metadata()
                .is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    file.is_file()
}

/// Where pi may be when it is not on the `PATH`, most likely first: Pistachio's own install,
/// npm's global folder, and the global folders of Node.js version managers (nvm, fnm, Volta,
/// asdf, mise, n, nvm-windows, Scoop) and other package managers (pnpm, Bun, Yarn, Homebrew).
/// Folders of several Node.js versions list the newest first. `windows` picks the system's
/// conventions (also for tests on other systems).
pub fn known_places(folders: &Folders, windows: bool) -> Vec<PathBuf> {
    let mut places = Vec::new();
    let home = folders.home.clone();
    let at = |base: &Option<PathBuf>, rest: &[&str]| {
        base.as_ref()
            .map(|base| rest.iter().fold(base.clone(), |path, part| path.join(part)))
    };
    if windows {
        let file = |dir: PathBuf, names: &[&str]| -> Vec<PathBuf> {
            names.iter().map(|name| dir.join(name)).collect()
        };
        let cmd = ["pi.cmd", "pi.exe"];
        let dirs: Vec<Option<PathBuf>> = vec![
            at(&home, &[".pistachio", "server", "node"]),
            at(&folders.app_data, &["npm"]),
            folders.nvm_symlink.clone(),
            at(&folders.program_files, &["nodejs"]),
            at(&folders.local_app_data, &["Volta", "bin"]),
            at(&folders.local_app_data, &["pnpm"]),
            at(&home, &["scoop", "persist", "nodejs", "bin"]),
            at(&home, &["scoop", "apps", "nodejs", "current", "bin"]),
            at(&home, &["scoop", "shims"]),
            at(&home, &[".bun", "bin"]),
        ];
        for dir in dirs.into_iter().flatten() {
            places.extend(file(dir, &cmd));
        }
        // nvm-windows keeps each version in %APPDATA%\nvm\v22.19.0.
        if let Some(nvm) = at(&folders.app_data, &["nvm"]) {
            for version in versions(&nvm) {
                places.extend(file(version, &cmd));
            }
        }
        // fnm on Windows.
        if let Some(fnm) = at(&folders.app_data, &["fnm", "node-versions"]) {
            for version in versions(&fnm) {
                places.extend(file(version.join("installation"), &cmd));
            }
        }
        return places;
    }
    let pi = |dir: Option<PathBuf>| dir.map(|dir| dir.join("pi"));
    places.extend(
        [
            pi(at(&home, &[".pistachio", "server", "bin"])),
            pi(at(&home, &[".npm-global", "bin"])),
            pi(at(&home, &[".npm", "bin"])),
            pi(at(&home, &[".local", "bin"])),
            pi(at(&home, &[".volta", "bin"])),
            pi(at(&home, &[".bun", "bin"])),
            pi(at(&home, &["Library", "pnpm"])),
            pi(at(&home, &[".local", "share", "pnpm"])),
            pi(at(&home, &[".yarn", "bin"])),
            pi(at(&home, &["n", "bin"])),
            Some(PathBuf::from("/opt/homebrew/bin/pi")),
            Some(PathBuf::from("/usr/local/bin/pi")),
            Some(PathBuf::from("/usr/bin/pi")),
        ]
        .into_iter()
        .flatten(),
    );
    let managers: [(Option<PathBuf>, &[&str]); 5] = [
        (at(&home, &[".nvm", "versions", "node"]), &["bin"]),
        (
            at(&home, &[".local", "share", "fnm", "node-versions"]),
            &["installation", "bin"],
        ),
        (
            at(
                &home,
                &["Library", "Application Support", "fnm", "node-versions"],
            ),
            &["installation", "bin"],
        ),
        (at(&home, &[".asdf", "installs", "nodejs"]), &["bin"]),
        (
            at(&home, &[".local", "share", "mise", "installs", "node"]),
            &["bin"],
        ),
    ];
    for (root, rest) in managers {
        let Some(root) = root else { continue };
        for version in versions(&root) {
            places.push(
                rest.iter()
                    .fold(version, |path, part| path.join(part))
                    .join("pi"),
            );
        }
    }
    places
}

/// The folders in `dir` named like versions (`v22.19.0`, `22.19.0`), newest first.
fn versions(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(Vec<u64>, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let numbers: Vec<u64> = name
                .trim_start_matches('v')
                .split('.')
                .map_while(|part| part.parse().ok())
                .collect();
            (!numbers.is_empty()).then(|| (numbers, entry.path()))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, path)| path).collect()
}

/// Runs `program --version` with `envs` added (the login shell's `PATH`, so pi finds Node.js):
/// the version it prints, or why it is not pi.
pub fn check(program: &Path, envs: &[(OsString, OsString)]) -> Result<String, String> {
    let mut command = Command::new(program);
    command
        .no_window()
        .arg("--version")
        .envs(envs.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = run(command, CHECK_TIMEOUT, &mut |_| {})
        .map_err(|error| format!("{} could not be started: {error}", program.display()))?;
    if !output.success {
        let all = format!("{}{}", output.stdout, output.stderr);
        let detail = all
            .lines()
            .rfind(|line| !line.trim().is_empty())
            .unwrap_or("it failed");
        return Err(format!(
            "{} is not a working pi: {detail}",
            program.display()
        ));
    }
    let version = output
        .stdout
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .trim();
    // pi prints its version (`0.73.1`), perhaps after a line or two of Node.js warnings.
    if version.is_empty() || !version.starts_with(|c: char| c.is_ascii_digit()) {
        return Err(format!(
            "{} does not look like pi (it says \"{version}\" when asked for its version).",
            program.display()
        ));
    }
    Ok(version.to_owned())
}

/// Installs pi into `~/.pistachio/server` and returns the program to start. Blocks for as long
/// as the downloads take (up to 15 minutes); `on_step` hears short steps like "Downloading
/// Node.js".
pub fn install(on_step: &mut dyn FnMut(&str)) -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        windows::install(on_step)
    }
    #[cfg(not(windows))]
    {
        install_with_sh(on_step)
    }
}

#[cfg(not(windows))]
fn install_with_sh(on_step: &mut dyn FnMut(&str)) -> Result<PathBuf, String> {
    let script = format!("{{\n{}\n}} 2>&1", remote::install_script());
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut installed = None;
    let output = run(command, INSTALL_TIMEOUT, &mut |line| {
        if let Some(step) = line.strip_prefix("PISTACHIO_STEP=") {
            on_step(step.trim());
        } else if let Some(path) = line.strip_prefix("PISTACHIO_INSTALLED=") {
            installed = Some(PathBuf::from(path.trim()));
        }
    })
    .map_err(|error| format!("Could not run the installer: {error}"))?;
    match installed {
        Some(path) if output.success => Ok(path),
        _ => Err(failure(&format!("{}{}", output.stdout, output.stderr))),
    }
}

/// What went wrong, from the end of a program's output.
fn failure(text: &str) -> String {
    let tail: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with("PISTACHIO_"))
        .collect();
    let tail = tail[tail.len().saturating_sub(6)..].join("\n");
    if tail.is_empty() {
        "The installer stopped without saying why.".to_owned()
    } else {
        tail
    }
}

/// What a program printed and whether it succeeded.
struct Output {
    success: bool,
    stdout: String,
    stderr: String,
}

/// Runs `command` (stdout and stderr piped), hands each line of its stdout to `on_line` as it
/// comes, and stops it after `timeout`.
fn run(
    mut command: Command,
    timeout: Duration,
    on_line: &mut dyn FnMut(&str),
) -> std::io::Result<Output> {
    let mut child = command.spawn()?;
    let stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(stderr) = stderr.as_mut() {
            let _ = stderr.read_to_string(&mut text);
        }
        text
    });
    let (done, finished) = std::sync::mpsc::channel::<()>();
    let pid = child.id();
    let watchdog = std::thread::spawn(move || {
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) if started.elapsed() > timeout => {
                    log::warn!("process {pid} took longer than {timeout:?}; stopping it");
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                Ok(None) => {}
                Err(_) => return None,
            }
            if finished.recv_timeout(Duration::from_millis(100)).is_ok() {
                // Output ended: the process is exiting.
                return child.wait().ok();
            }
        }
    });
    let mut text = String::new();
    if let Some(stdout) = stdout {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            on_line(&line);
            text.push_str(&line);
            text.push('\n');
        }
    }
    let _ = done.send(());
    let status = watchdog.join().ok().flatten();
    Ok(Output {
        success: status.is_some_and(|status| status.success()),
        stdout: text,
        stderr: errors.join().unwrap_or_default(),
    })
}

/// `node-v22.19.0-win-x64.zip` and its SHA-256 from Node.js' `SHASUMS256.txt`, for `arch`
/// (`x64`, `arm64`).
pub fn node_zip(shasums: &str, arch: &str) -> Option<(String, String)> {
    let suffix = format!("-win-{arch}.zip");
    shasums.lines().find_map(|line| {
        let (sha, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim();
        (name.starts_with("node-v") && name.ends_with(&suffix) && sha.len() == 64)
            .then(|| (name.to_owned(), sha.to_owned()))
    })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn reads_node_s_checksums() {
        let shasums = "\
1111111111111111111111111111111111111111111111111111111111111111  node-v22.21.1-darwin-arm64.tar.gz
2222222222222222222222222222222222222222222222222222222222222222  node-v22.21.1-win-arm64.zip
3333333333333333333333333333333333333333333333333333333333333333  node-v22.21.1-win-x64.7z
4444444444444444444444444444444444444444444444444444444444444444  node-v22.21.1-win-x64.zip
";
        assert_eq!(
            node_zip(shasums, "x64"),
            Some(("node-v22.21.1-win-x64.zip".to_owned(), "4".repeat(64)))
        );
        assert_eq!(
            node_zip(shasums, "arm64").unwrap().0,
            "node-v22.21.1-win-arm64.zip"
        );
        assert_eq!(node_zip(shasums, "x86"), None);
    }

    #[test]
    fn knows_where_version_managers_put_pi() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path();
        for version in ["v20.11.0", "v22.19.0", "v22.9.0", "system"] {
            std::fs::create_dir_all(home.join(".nvm/versions/node").join(version).join("bin"))
                .unwrap();
        }
        let folders = Folders {
            home: Some(home.to_owned()),
            ..Folders::default()
        };
        let places = known_places(&folders, false);
        assert_eq!(places[0], home.join(".pistachio/server/bin/pi"));
        let nvm: Vec<_> = places
            .iter()
            .filter(|place| place.starts_with(home.join(".nvm")))
            .map(|place| place.strip_prefix(home).unwrap().to_owned())
            .collect();
        assert_eq!(
            nvm,
            [
                PathBuf::from(".nvm/versions/node/v22.19.0/bin/pi"),
                PathBuf::from(".nvm/versions/node/v22.9.0/bin/pi"),
                PathBuf::from(".nvm/versions/node/v20.11.0/bin/pi"),
            ]
        );

        let windows = Folders {
            home: Some(PathBuf::from("C:/Users/me")),
            app_data: Some(PathBuf::from("C:/Users/me/AppData/Roaming")),
            ..Folders::default()
        };
        let places = known_places(&windows, true);
        assert_eq!(
            places[0],
            PathBuf::from("C:/Users/me/.pistachio/server/node/pi.cmd")
        );
        assert!(places.contains(&PathBuf::from("C:/Users/me/AppData/Roaming/npm/pi.cmd")));
    }

    #[test]
    #[cfg(unix)]
    fn finds_pi_on_a_path_and_checks_it() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let pi = dir.path().join("pi");
        std::fs::write(&pi, "#!/bin/sh\necho 'node warning' >&2\necho 0.73.1\n").unwrap();
        assert_eq!(on_path(dir.path().as_os_str()), None, "not executable");
        std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(on_path(dir.path().as_os_str()), Some(pi.clone()));
        assert_eq!(find(Some(dir.path().as_os_str())), Some(pi.clone()));
        assert_eq!(check(&pi, &[]), Ok("0.73.1".to_owned()));

        let other = dir.path().join("other");
        std::fs::write(&other, "#!/bin/sh\necho 'usage: other'\n").unwrap();
        std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            check(&other, &[])
                .unwrap_err()
                .contains("does not look like pi")
        );
        let failing = dir.path().join("failing");
        std::fs::write(&failing, "#!/bin/sh\necho broken >&2\nexit 3\n").unwrap();
        std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(check(&failing, &[]).unwrap_err().contains("broken"));
        assert!(check(&dir.path().join("missing"), &[]).is_err());
    }

    #[test]
    fn says_why_an_install_failed() {
        assert_eq!(
            failure("PISTACHIO_STEP=Installing pi\nnpm ERR! network\n\nnpm ERR! offline\n"),
            "npm ERR! network\nnpm ERR! offline"
        );
        assert_eq!(failure(""), "The installer stopped without saying why.");
    }

    /// Installs into a scratch home with the real network: slow, so ignored.
    #[test]
    #[ignore = "downloads Node.js and pi"]
    fn installs_into_a_scratch_home() {
        let mut steps = Vec::new();
        let pi = install(&mut |step| steps.push(step.to_owned())).unwrap();
        eprintln!("{steps:?} {}", pi.display());
        assert!(check(&pi, &[]).is_ok());
    }
}
