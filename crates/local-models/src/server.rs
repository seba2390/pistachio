//! Running a setup's engine with its model, for this computer only.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::catalog::{Catalog, Format, Install};
use crate::install::{engine_binary, engine_dir, model_dir, model_installed};
use crate::setups::Setup;

/// Loading a large model from a slow disk takes a while.
const START_TIMEOUT: Duration = Duration::from_secs(600);
const LOG_LINES: usize = 30;

/// A running engine; dropping it stops it.
#[derive(Debug)]
pub struct Server {
    child: Child,
    port: u16,
    pid_file: PathBuf,
}

impl Server {
    /// Starts `setup` and waits until it answers. Blocks (up to minutes, while it loads).
    /// `path` is the `PATH` it runs with.
    pub fn start(
        root: &Path,
        catalog: &Catalog,
        setup: &Setup,
        path: Option<&OsStr>,
    ) -> Result<Self, String> {
        let engine = catalog
            .engine(&setup.engine)
            .ok_or("its engine is no longer in the catalog")?;
        let model = catalog
            .model(&setup.model)
            .ok_or("its model is no longer in the catalog")?;
        let build = model
            .build(&setup.build)
            .ok_or("its build is no longer in the catalog")?;
        let binary = engine_binary(root, engine).ok_or("its engine is not installed")?;
        if !model_installed(root, model, build) {
            return Err("its model is not downloaded".into());
        }
        let models = model_dir(root, model, build);
        let model_file = if build.format == Format::Gguf {
            // The first part of a split GGUF; the engine finds the rest.
            let mut ggufs: Vec<_> = build
                .files
                .iter()
                .filter(|f| f.name.to_ascii_lowercase().ends_with(".gguf"))
                .collect();
            ggufs.sort_by(|a, b| a.name.cmp(&b.name));
            ggufs.first().map(|file| models.join(&file.name))
        } else {
            None
        };
        let model_path = match build.format {
            Format::Gturbo => models.join("model.gturbo"),
            _ => models.clone(),
        };
        let fill = |arg: &str| -> String {
            arg.replace("{bin}", &binary.to_string_lossy())
                .replace(
                    "{python}",
                    &python_env::python_in(&binary).to_string_lossy(),
                )
                .replace(
                    "{model}",
                    &model_file
                        .as_deref()
                        .unwrap_or(&model_path)
                        .to_string_lossy(),
                )
                .replace("{model_dir}", &model_path.to_string_lossy())
                .replace("{upstream}", &build.repo)
                .replace("{id}", &model.id)
                .replace("{port}", &setup.port.to_string())
                .replace("{ctx}", &setup.context.to_string())
        };
        let args: Vec<String> = engine.launch.iter().map(|arg| fill(arg)).collect();
        let (program, rest) = args.split_first().ok_or("the engine has no command")?;
        let mut command = platform::command(program);
        command
            .args(rest)
            .current_dir(engine_dir(root, engine))
            .env("HF_HUB_OFFLINE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(path) = path {
            command.env("PATH", path);
        }
        if let Install::Python { .. } = engine.install {
            command
                .env("PYTHONPATH", engine_dir(root, engine).join("site"))
                .env("PYTHONNOUSERSITE", "1");
        }
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let pid_file = root.join("run").join(format!("{}.pid", setup.port));
        stop_leftover(&pid_file);
        Self::run(command, setup.port, &engine.health, pid_file)
    }

    fn run(
        mut command: Command,
        port: u16,
        health: &str,
        pid_file: PathBuf,
    ) -> Result<Self, String> {
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        if let Some(dir) = pid_file.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(&pid_file, child.id().to_string());
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
            pid_file,
        };
        let deadline = Instant::now() + START_TIMEOUT;
        while !answers(port, health) {
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
            std::thread::sleep(Duration::from_millis(250));
        }
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
        stop_group(self.child.id());
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.pid_file);
    }
}

/// Stops an engine an earlier run left behind (the app ended without stopping it), so its port
/// is free again.
fn stop_leftover(pid_file: &Path) {
    let Some(pid) = fs::read_to_string(pid_file)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
    else {
        return;
    };
    stop_group(pid);
    std::thread::sleep(Duration::from_millis(500));
    let _ = fs::remove_file(pid_file);
}

/// Asks the process group an engine started (it may have started more processes) to end.
fn stop_group(pid: u32) {
    if cfg!(unix) {
        let _ = Command::new("kill")
            .args(["-TERM", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Whether the engine on `port` answers `health` with success.
pub fn answers(port: u16, health: &str) -> bool {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(2)))
        .proxy(None)
        .build()
        .new_agent();
    agent
        .get(format!("http://127.0.0.1:{port}{health}"))
        .call()
        .is_ok()
}
