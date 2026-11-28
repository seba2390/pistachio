//! Spawning and supervising a `pi --mode rpc` child process.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command as OsCommand, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::client::{ClientEvent, PiClient};
use crate::protocol::ThinkingLevel;

/// How many stderr lines to keep for crash diagnostics.
const STDERR_TAIL_LINES: usize = 64;
/// How long pi gets for an orderly shutdown after stdin closes.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Options for one pi process. Mirrors pi's own CLI flags.
#[derive(Debug, Clone)]
pub struct SpawnOptions {
    /// The `pi` executable (resolved through `PATH` when relative).
    pub program: PathBuf,
    /// Working directory: pi runs tools and resolves project config from here.
    pub cwd: PathBuf,
    /// `--session <path>`: open an existing session file.
    pub session: Option<PathBuf>,
    /// `--model <provider/id>`.
    pub model: Option<String>,
    /// `--thinking <level>`.
    pub thinking: Option<ThinkingLevel>,
    /// Additional CLI arguments, appended verbatim.
    pub extra_args: Vec<OsString>,
    /// Environment overrides (e.g. a login-shell `PATH` for GUI launches).
    pub env: Vec<(OsString, OsString)>,
    /// Runs pi through another program instead of directly, e.g. `ssh host -- … pi …` for a
    /// remote machine. The launcher's arguments must already contain the pi command line
    /// (see [`SpawnOptions::args`]); `program`, `args` and `cwd` above are then only used to
    /// build it, and the launcher runs in `cwd` locally.
    pub launcher: Option<Launcher>,
}

/// A program that starts pi on pi-rpc's behalf; see [`SpawnOptions::launcher`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

impl SpawnOptions {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            program: PathBuf::from("pi"),
            cwd: cwd.into(),
            session: None,
            model: None,
            thinking: None,
            extra_args: Vec::new(),
            env: Vec::new(),
            launcher: None,
        }
    }

    /// The full argument vector passed to pi.
    pub fn args(&self) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec!["--mode".into(), "rpc".into()];
        if let Some(session) = &self.session {
            args.push("--session".into());
            args.push(session.into());
        }
        if let Some(model) = &self.model {
            args.push("--model".into());
            args.push(model.into());
        }
        if let Some(level) = self.thinking {
            args.push("--thinking".into());
            args.push(level.as_str().into());
        }
        args.extend(self.extra_args.iter().cloned());
        args
    }
}

/// A running pi process. Dropping it shuts pi down gracefully in the background.
pub struct PiProcess {
    client: PiClient,
    child: Arc<Mutex<Child>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
}

impl PiProcess {
    pub fn spawn(
        options: &SpawnOptions,
    ) -> std::io::Result<(Self, async_channel::Receiver<ClientEvent>)> {
        let mut command = match &options.launcher {
            Some(launcher) => {
                let mut command = OsCommand::new(&launcher.program);
                command.args(&launcher.args);
                command
            }
            None => {
                let mut command = OsCommand::new(&options.program);
                command.args(options.args());
                command
            }
        };
        // No console window on Windows (pi is `pi.cmd`, run by cmd.exe, then Node.js).
        platform::NoWindow::no_window(&mut command)
            .current_dir(&options.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in &options.env {
            command.env(key, value);
        }
        // Its own process group, so a pi that hangs on shutdown can be stopped together
        // with the commands its tools started.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child = command.spawn()?;

        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");

        let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)));
        let tail = Arc::clone(&stderr_tail);
        thread::Builder::new()
            .name("pi-stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    log::debug!(target: "pi", "{line}");
                    let mut tail = tail.lock().unwrap_or_else(PoisonError::into_inner);
                    if tail.len() == STDERR_TAIL_LINES {
                        tail.pop_front();
                    }
                    tail.push_back(line);
                }
            })?;

        let (client, events) = PiClient::connect(stdout, stdin);
        Ok((
            Self {
                client,
                child: Arc::new(Mutex::new(child)),
                stderr_tail,
            },
            events,
        ))
    }

    pub fn client(&self) -> &PiClient {
        &self.client
    }

    pub fn pid(&self) -> u32 {
        self.child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .id()
    }

    /// The last lines pi wrote to stderr, for showing why it exited.
    pub fn stderr_tail(&self) -> Vec<String> {
        self.stderr_tail
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }
}

impl Drop for PiProcess {
    fn drop(&mut self) {
        // Closing stdin requests an orderly shutdown; kill only if pi hangs.
        self.client.close();
        let child = Arc::clone(&self.child);
        let reaper = thread::Builder::new()
            .name("pi-reaper".into())
            .spawn(move || {
                let deadline = Instant::now() + SHUTDOWN_GRACE;
                loop {
                    let mut child = child.lock().unwrap_or_else(PoisonError::into_inner);
                    match child.try_wait() {
                        Ok(Some(_)) | Err(_) => return,
                        Ok(None) if Instant::now() >= deadline => {
                            kill_group(child.id());
                            let _ = child.kill();
                            let _ = child.wait();
                            return;
                        }
                        Ok(None) => {}
                    }
                    drop(child);
                    thread::sleep(Duration::from_millis(50));
                }
            });
        if let Err(error) = reaper {
            log::warn!("could not spawn pi reaper thread: {error}");
        }
    }
}

/// Kills the process group led by `pid` (pi and anything its tools left running).
fn kill_group(pid: u32) {
    #[cfg(unix)]
    {
        let _ = OsCommand::new("kill")
            .args(["-KILL", "--", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(unix))]
    let _ = pid;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_mirror_pi_cli_flags() {
        let mut options = SpawnOptions::new("/tmp");
        options.session = Some("/s/a.jsonl".into());
        options.model = Some("spark/qwen3.8-27b".into());
        options.thinking = Some(ThinkingLevel::High);
        let args: Vec<String> = options
            .args()
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect();
        assert_eq!(
            args,
            [
                "--mode",
                "rpc",
                "--session",
                "/s/a.jsonl",
                "--model",
                "spark/qwen3.8-27b",
                "--thinking",
                "high"
            ]
        );
    }
}
