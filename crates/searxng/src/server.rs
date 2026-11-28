//! Running the installed SearXNG.

use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead as _, BufReader, Read as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::install::{Installed, Layout, installed};

/// How long starting may take: the first time Python compiles everything.
const START_TIMEOUT: Duration = Duration::from_secs(60);
const LOG_LINES: usize = 20;

/// Ends SearXNG when its standard input does, which is when the app ends, however it ends.
const LAUNCH: &str = r#"
import os, runpy, threading

def stop_with_the_app():
    while os.read(0, 4096):
        pass
    os._exit(0)

threading.Thread(target=stop_with_the_app, daemon=True).start()
runpy.run_module("searx.webapp", run_name="__main__", alter_sys=True)
"#;

/// The user's to edit; only written when missing.
const SETTINGS: &str = "\
# SearXNG's settings for Pistachio, used the next time it starts. See
# https://docs.searxng.org/admin/settings/ for what else there is. Pistachio needs the json format.
use_default_settings: true
search:
  formats: [html, json]
  autocomplete: \"\"
server:
  limiter: false
  public_instance: false
  image_proxy: false
";

/// A running SearXNG that only this computer can reach; dropping it stops it.
#[derive(Debug)]
pub struct Server {
    child: Child,
    port: u16,
    root: PathBuf,
}

impl Server {
    /// Starts the SearXNG installed in `root` and waits until it answers. Blocks.
    pub fn start(root: &Path) -> Result<Self, String> {
        if installed(root) == Installed::No {
            return Err("it is not installed".into());
        }
        let settings = root.join("settings.yml");
        if !settings.exists() {
            fs::write(&settings, SETTINGS)
                .map_err(|e| format!("writing {}: {e}", settings.display()))?;
        }
        let port = free_port().map_err(|e| format!("no port is free: {e}"))?;
        let command = command(root, &settings, port)?;
        Self::run(command, root, port)
    }

    fn run(mut command: Command, root: &Path, port: u16) -> Result<Self, String> {
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let log = Arc::new(Mutex::new(VecDeque::new()));
        let stderr = child.stderr.take().expect("stderr is piped");
        let lines = Arc::clone(&log);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut lines = lines.lock().unwrap_or_else(PoisonError::into_inner);
                if lines.len() == LOG_LINES {
                    lines.pop_front();
                }
                lines.push_back(line);
            }
        });
        // From here on an early return stops it.
        let mut server = Self {
            child,
            port,
            root: root.to_path_buf(),
        };
        let deadline = Instant::now() + START_TIMEOUT;
        while !answers(port) {
            let ended = server.child.try_wait().map_err(|e| e.to_string())?;
            if ended.is_some() || Instant::now() > deadline {
                let log = log.lock().unwrap_or_else(PoisonError::into_inner);
                let last = log.iter().rev().find(|line| !line.trim().is_empty());
                return Err(match (last, ended) {
                    (Some(line), _) => line.trim().to_owned(),
                    (None, Some(status)) => format!("it stopped ({status})"),
                    (None, None) => "it does not answer".into(),
                });
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let file = port_file(root);
        fs::write(&file, port.to_string())
            .map_err(|e| format!("writing {}: {e}", file.display()))?;
        Ok(server)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Another app's server may have been started since.
        if read_port(&self.root) == Some(self.port) {
            let _ = fs::remove_file(port_file(&self.root));
        }
    }
}

fn command(root: &Path, settings: &Path, port: u16) -> Result<Command, String> {
    let layout = Layout::installed(root);
    let data = root.join("data");
    let tmp = data.join("tmp");
    fs::create_dir_all(&tmp).map_err(|e| format!("creating {}: {e}", tmp.display()))?;
    let modules =
        std::env::join_paths([layout.searxng(), layout.packages()]).map_err(|e| e.to_string())?;
    let secret = secret().map_err(|e| format!("no random numbers: {e}"))?;
    let mut command = platform::command(layout.python());
    command
        .args(["-c", LAUNCH])
        .env_clear()
        .envs(python_env::network_env())
        .env("HOME", &data)
        .env("TMPDIR", &tmp)
        // It needs no other programs.
        .env("PATH", data.join("none"))
        .env("PYTHONPATH", modules)
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONSAFEPATH", "1")
        .env("PYTHONUTF8", "1")
        .env("SEARXNG_SETTINGS_PATH", settings)
        .env("SEARXNG_BIND_ADDRESS", "127.0.0.1")
        .env("SEARXNG_PORT", port.to_string())
        .env("SEARXNG_SECRET", secret)
        .current_dir(&data)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    Ok(command)
}

fn secret() -> std::io::Result<String> {
    let mut bytes = [0; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(python_env::fetch::hex(&bytes))
}

fn free_port() -> std::io::Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

fn port_file(root: &Path) -> PathBuf {
    root.join("port")
}

fn read_port(root: &Path) -> Option<u16> {
    fs::read_to_string(port_file(root))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// Whether a SearXNG answers on `port` of this computer.
fn answers(port: u16) -> bool {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(2)))
        .proxy(None)
        .build()
        .new_agent();
    agent
        .get(format!("{}/healthz", url(port)))
        .call()
        .and_then(|mut response| response.body_mut().read_to_string())
        .is_ok_and(|text| text.trim() == "OK")
}

/// The address of the running SearXNG, started by this or another process.
pub fn running_url(root: &Path) -> Option<String> {
    read_port(root).filter(|port| answers(*port)).map(url)
}

/// [`running_url`], waiting for one that is being started.
pub fn wait_for_url(root: &Path, timeout: Duration) -> Result<String, String> {
    if installed(root) == Installed::No {
        return Err(
            "SearXNG is not installed on this computer yet. Download it in Pistachio's Settings \
             → Web search, or choose another search service there."
                .into(),
        );
    }
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(url) = running_url(root) {
            return Ok(url);
        }
        if Instant::now() >= deadline {
            return Err(
                "SearXNG is not running on this computer. Pistachio's Settings → Web search says \
                 why."
                    .into(),
            );
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::io::Write as _;

    use super::*;

    /// Answers like SearXNG's health check, until the test ends.
    fn healthy() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let _ = stream.read(&mut [0; 2048]);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
                );
            }
        });
        port
    }

    fn sh(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        command
    }

    #[test]
    fn a_server_is_found_until_it_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let port = healthy();
        let mut server = Server::run(sh("exec sleep 60"), dir.path(), port).unwrap();
        assert!(server.is_running());
        assert_eq!(running_url(dir.path()), Some(url(port)));
        drop(server);
        assert_eq!(running_url(dir.path()), None);
        assert!(!port_file(dir.path()).exists());
    }

    #[test]
    fn another_apps_server_stays_known() {
        let dir = tempfile::tempdir().unwrap();
        let server = Server::run(sh("exec sleep 60"), dir.path(), healthy()).unwrap();
        let other = healthy();
        fs::write(port_file(dir.path()), other.to_string()).unwrap();
        drop(server);
        assert_eq!(running_url(dir.path()), Some(url(other)));
    }

    #[test]
    fn says_why_it_did_not_start() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port().unwrap();
        let script = "echo first >&2; echo 'ImportError: no flask' >&2; sleep 0.2; exit 1";
        let error = Server::run(sh(script), dir.path(), port).unwrap_err();
        assert_eq!(error, "ImportError: no flask");
        assert!(!port_file(dir.path()).exists());
        assert_eq!(
            Server::start(dir.path()).unwrap_err(),
            "it is not installed"
        );
    }

    #[test]
    fn a_stale_port_is_not_an_address() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(port_file(dir.path()), free_port().unwrap().to_string()).unwrap();
        assert_eq!(running_url(dir.path()), None);
        let error = wait_for_url(dir.path(), Duration::ZERO).unwrap_err();
        assert!(error.contains("not installed"), "{error}");
    }
}
