//! Installing pi on Windows, like the POSIX installer does elsewhere: the official Node.js 22
//! zip from nodejs.org (checked against its SHA-256) into `~\.pistachio\server\node`, and pi
//! from npm into that folder, where npm's `pi.cmd` starts it with that Node.js. Downloads and
//! unpacking use the `curl.exe` and `tar.exe` that come with Windows 10 and 11.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use platform::NoWindow as _;
use sha2::{Digest as _, Sha256};

const NODE: &str = "https://nodejs.org/dist/latest-v22.x";
const PACKAGE: &str = "@earendil-works/pi-coding-agent";

pub(crate) fn install(on_step: &mut dyn FnMut(&str)) -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("Your user folder is unknown.")?;
    let root = home.join(".pistachio").join("server");
    std::fs::create_dir_all(&root).map_err(|e| format!("Creating {}: {e}", root.display()))?;
    let node_dir = root.join("node");
    let node = node_dir.join("node.exe");
    on_step("Looking for Node.js");
    if !(node.is_file() && node_ok(&node)) {
        download_node(&root, &node_dir, on_step)?;
    }
    on_step("Installing pi");
    npm_install(&node_dir, &["--min-release-age=0"]).or_else(|_| npm_install(&node_dir, &[]))?;
    let pi = node_dir.join("pi.cmd");
    if !pi.is_file() {
        return Err("npm did not install pi.".into());
    }
    on_step("Checking pi");
    crate::check(&pi, &[])?;
    Ok(pi)
}

/// Node.js 22.19 or newer.
fn node_ok(node: &Path) -> bool {
    let script = "const [a,b]=process.versions.node.split('.').map(Number);\
                  process.exit(a>22||(a===22&&b>=19)?0:1)";
    Command::new(node)
        .no_window()
        .args(["-e", script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn download_node(
    root: &Path,
    node_dir: &Path,
    on_step: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    };
    let downloads = root.join("download");
    let _ = std::fs::remove_dir_all(&downloads);
    std::fs::create_dir_all(&downloads)
        .map_err(|e| format!("Creating {}: {e}", downloads.display()))?;
    on_step("Downloading Node.js");
    let sums = downloads.join("SHASUMS256.txt");
    fetch(&format!("{NODE}/SHASUMS256.txt"), &sums)?;
    let sums = std::fs::read_to_string(&sums).map_err(|e| e.to_string())?;
    let (name, want) = crate::node_zip(&sums, arch)
        .ok_or_else(|| format!("No Node.js build is available for Windows on {arch}."))?;
    let zip = downloads.join(&name);
    fetch(&format!("{NODE}/{name}"), &zip)?;
    on_step("Verifying Node.js");
    let bytes = std::fs::read(&zip).map_err(|e| e.to_string())?;
    let got = Sha256::digest(&bytes)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            use std::fmt::Write as _;
            let _ = write!(hex, "{byte:02x}");
            hex
        });
    if got != want.to_ascii_lowercase() {
        return Err("The Node.js download is damaged (checksum mismatch).".into());
    }
    on_step("Unpacking Node.js");
    let unpacked = root.join("node.new");
    let _ = std::fs::remove_dir_all(&unpacked);
    std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    let status = Command::new(system_tool("tar.exe"))
        .no_window()
        .arg("-xf")
        .arg(&zip)
        .arg("-C")
        .arg(&unpacked)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| format!("Could not run tar.exe: {e}"))?;
    if !status.success() {
        return Err("Unpacking Node.js failed.".into());
    }
    let folder = unpacked.join(name.trim_end_matches(".zip"));
    if !folder.join("node.exe").is_file() {
        return Err("The Node.js download holds no node.exe.".into());
    }
    let _ = std::fs::remove_dir_all(node_dir);
    std::fs::rename(&folder, node_dir).map_err(|e| format!("Moving Node.js in place: {e}"))?;
    let _ = std::fs::remove_dir_all(&unpacked);
    let _ = std::fs::remove_dir_all(&downloads);
    Ok(())
}

/// A program in `%SystemRoot%\System32` (Windows' own, not another one on the `PATH`).
fn system_tool(name: &str) -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(|root| PathBuf::from(root).join("System32").join(name))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

fn fetch(url: &str, to: &Path) -> Result<(), String> {
    let output = Command::new(system_tool("curl.exe"))
        .no_window()
        .args(["-fsSL", "--retry", "2", "-o"])
        .arg(to)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Could not run curl.exe: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(format!("Downloading {url} failed: {error}"))
    }
}

/// `npm install -g` of pi into the Node.js folder, run by that Node.js.
fn npm_install(node_dir: &Path, extra: &[&str]) -> Result<(), String> {
    let npm = node_dir
        .join("node_modules")
        .join("npm")
        .join("bin")
        .join("npm-cli.js");
    let mut path = std::ffi::OsString::from(node_dir.as_os_str());
    if let Some(rest) = std::env::var_os("PATH") {
        path.push(";");
        path.push(rest);
    }
    let output = Command::new(node_dir.join("node.exe"))
        .no_window()
        .arg(npm)
        .args(["install", "-g", "--prefix"])
        .arg(node_dir)
        .args([
            "--ignore-scripts",
            "--no-fund",
            "--no-audit",
            "--loglevel=error",
            "--progress=false",
        ])
        .args(extra)
        .arg(PACKAGE)
        .env("PATH", path)
        .env("NPM_CONFIG_UPDATE_NOTIFIER", "false")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Could not run npm: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Err(crate::failure(&text))
    }
}
