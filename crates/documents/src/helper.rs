//! Running an installed parser: one process per document, with a file as input and a file of
//! Markdown as output. It has no network and a time limit, and it ends, with everything it
//! started, when this process does.

use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crate::Parser;
use crate::install::Layout;

/// More Markdown than this is refused.
const MAX_OUTPUT: u64 = 32 * 1024 * 1024;
const LOG_LINES: usize = 20;
/// Why a parser with models did not get its turn ([`turn`]).
pub(crate) const BUSY: &str = "it is busy with other documents";
/// What a run leaves behind when it is killed is deleted after this long at the latest.
const LEFT_OVER: Duration = Duration::from_hours(24);

/// Ends the parser and what it started when its standard input ends, which is when the app's
/// helper does, however it ends.
const WATCH: &str = r#"
import os, signal, sys, threading

def stop_with_the_app():
    while os.read(0, 4096):
        pass
    os.killpg(os.getpgrp(), signal.SIGKILL)

threading.Thread(target=stop_with_the_app, daemon=True).start()
source, target = sys.argv[1:3]
models = os.environ["PISTACHIO_MODELS"]
"#;

/// What a parser runs. `source`, `target` and `models` are set ([`WATCH`]).
fn script(parser: Parser) -> Option<&'static str> {
    match parser {
        Parser::Builtin => None,
        Parser::Pymupdf4llm => Some(include_str!("runners/pymupdf4llm.py")),
        Parser::Docling => Some(include_str!("runners/docling.py")),
        Parser::Mineru => Some(include_str!("runners/mineru.py")),
    }
}

/// Where the runs keep their files.
fn runs_dir(dir: &Path) -> PathBuf {
    dir.join("tmp")
}

/// A folder of its own for one run; deleted with it.
pub struct Run {
    pub dir: PathBuf,
}

impl Run {
    pub fn new(dir: &Path) -> Result<Self, String> {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let runs = runs_dir(dir);
        sweep(&runs);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = runs.join(format!("{}-{n}", std::process::id()));
        private_dir(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        Ok(Self { dir })
    }
}

/// Only the user's: it holds a copy of their document.
fn private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Deletes what runs that were killed left.
fn sweep(runs: &Path) {
    let Ok(entries) = fs::read_dir(runs) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let age = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok());
        let name = entry.file_name();
        if age.is_some_and(|age| age > LEFT_OVER) || is_over(&name.to_string_lossy()) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Whether the process that made the run called `name` has ended.
fn is_over(name: &str) -> bool {
    #[cfg(unix)]
    if let Some(id) = name.split('-').next().and_then(|id| id.parse().ok())
        && let Some(id) = rustix::process::Pid::from_raw(id)
    {
        let gone = rustix::process::test_kill_process(id);
        return gone.is_err_and(|error| error == rustix::io::Errno::SRCH);
    }
    let _ = name;
    false
}

/// Reads `file` with the `parser` installed at `layout`, as Markdown. Blocks, for at most
/// `timeout` and until it is cancelled.
pub(crate) fn parse(
    dir: &Path,
    layout: &Layout,
    parser: Parser,
    file: &Path,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let _turn = if parser.has_models() {
        Some(turn(dir, timeout, cancel)?)
    } else {
        None
    };
    let run = Run::new(dir)?;
    let command = command(layout, parser, file, &run.dir)?;
    execute(command, &output(&run.dir), timeout, cancel)
}

/// Waits until no other parser with models runs, in any process: several would fill the memory.
/// The turn ends when the file is dropped.
fn turn(dir: &Path, timeout: Duration, cancel: &AtomicBool) -> Result<fs::File, String> {
    let path = runs_dir(dir).join("lock");
    let failed = |e: &dyn std::fmt::Display| format!("opening {}: {e}", path.display());
    fs::create_dir_all(runs_dir(dir)).map_err(|e| failed(&e))?;
    let file = fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| failed(&e))?;
    let deadline = Instant::now() + timeout;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(fs::TryLockError::Error(error)) => return Err(failed(&error)),
            Err(fs::TryLockError::WouldBlock) => {}
        }
        if Instant::now() >= deadline || cancel.load(Ordering::Relaxed) {
            return Err(BUSY.into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn output(run: &Path) -> PathBuf {
    run.join("out.md")
}

fn command(layout: &Layout, parser: Parser, file: &Path, run: &Path) -> Result<Command, String> {
    let script = script(parser).ok_or("it is not a separate program")?;
    let home = run.join("home");
    fs::create_dir_all(&home).map_err(|e| format!("creating {}: {e}", home.display()))?;
    let mut command = platform::command(layout.python());
    command
        .arg("-c")
        .arg(format!("{WATCH}{script}"))
        .arg(file)
        .arg(output(run))
        .env_clear()
        .env("HOME", &home)
        .env("TMPDIR", run)
        .env("XDG_CACHE_HOME", home.join("cache"))
        // No folder with Apple's placeholders for developer tools: running one opens a window
        // offering to install them.
        .env("PATH", "/bin:/usr/sbin:/sbin")
        .env("LANG", "en_US.UTF-8")
        .env("PYTHONPATH", layout.packages())
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONSAFEPATH", "1")
        .env("PYTHONUTF8", "1")
        .env("PISTACHIO_MODELS", layout.models())
        .env("HF_HOME", home.join("huggingface"))
        .env("HF_HUB_OFFLINE", "1")
        .env("TRANSFORMERS_OFFLINE", "1")
        // Nothing to reach a network with: what is not installed is not downloaded behind one's
        // back, and no document leaves the computer.
        .env("http_proxy", "http://127.0.0.1:9")
        .env("https_proxy", "http://127.0.0.1:9")
        .env("all_proxy", "http://127.0.0.1:9")
        .current_dir(run)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    Ok(command)
}

/// Runs `command` until it ends or `timeout` is over, and returns what it wrote to `output`.
fn execute(
    mut command: Command,
    output: &Path,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<String, String> {
    // Its own group, so that all it started is stopped with it.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let deadline = Instant::now() + timeout;
    let mut child = command
        .spawn()
        .map_err(|error| format!("it did not start: {error}"))?;
    let stderr = child.stderr.take().expect("stderr is piped");
    let log = std::thread::spawn(move || {
        let mut last = VecDeque::new();
        for line in BufReader::new(stderr).split(b'\n').map_while(Result::ok) {
            if last.len() == LOG_LINES {
                last.pop_front();
            }
            last.push_back(String::from_utf8_lossy(&line).trim().to_owned());
        }
        last
    });
    // Held until the end: the parser stops when it closes.
    let stdin = child.stdin.take();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if cancel.load(Ordering::Relaxed) => {
                kill(&mut child);
                return Err("cancelled".into());
            }
            Ok(None) if Instant::now() >= deadline => {
                kill(&mut child);
                return Err(format!("it took longer than {}", seconds(timeout)));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(30)),
            Err(error) => {
                kill(&mut child);
                return Err(error.to_string());
            }
        }
    };
    // What it left running would keep its standard error open.
    kill(&mut child);
    drop(stdin);
    let log = log.join().unwrap_or_default();
    if !status.success() {
        let last = log.iter().rev().find(|line| !line.is_empty());
        return Err(last.map_or_else(
            || format!("it stopped ({status})"),
            |line| line.chars().take(300).collect(),
        ));
    }
    let size = fs::metadata(output).map_or(0, |meta| meta.len());
    if size > MAX_OUTPUT {
        return Err("the text is too large".into());
    }
    let bytes = fs::read(output).map_err(|_| "it wrote no text")?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Ends the process and everything in its group.
fn kill(child: &mut Child) {
    #[cfg(unix)]
    if let Ok(id) = i32::try_from(child.id())
        && let Some(group) = rustix::process::Pid::from_raw(id)
    {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn seconds(duration: Duration) -> String {
    match duration.as_secs() {
        minutes if minutes >= 120 && minutes % 60 == 0 => format!("{} minutes", minutes / 60),
        seconds => format!("{seconds} seconds"),
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    const LONG: Duration = Duration::from_secs(20);

    fn sh(script: &str, dir: &Path) -> Command {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script])
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        command
    }

    fn try_it(script: &str, timeout: Duration) -> Result<String, String> {
        let dir = tempfile::tempdir().unwrap();
        let go = AtomicBool::new(false);
        execute(sh(script, dir.path()), &output(dir.path()), timeout, &go)
    }

    #[test]
    fn returns_what_it_wrote_or_why_not() {
        assert_eq!(
            try_it("echo noise; printf '# Hi' > out.md", LONG).unwrap(),
            "# Hi"
        );
        assert_eq!(try_it("exit 0", LONG).unwrap_err(), "it wrote no text");
        let script = "echo Traceback >&2; echo 'ValueError: bad PDF' >&2; echo >&2; exit 1";
        assert_eq!(try_it(script, LONG).unwrap_err(), "ValueError: bad PDF");
        assert!(try_it("exit 3", LONG).unwrap_err().contains("stopped"));
        // Bytes that are not text do not break it.
        assert!(
            try_it("printf 'a\\377b' > out.md", LONG)
                .unwrap()
                .starts_with('a')
        );
        let too_large = format!(
            "dd if=/dev/zero of=out.md bs=1 count=0 seek={}",
            MAX_OUTPUT + 1
        );
        assert!(try_it(&too_large, LONG).unwrap_err().contains("too large"));
        let missing = Command::new("/nonexistent/python");
        let go = AtomicBool::new(false);
        let error = execute(missing, Path::new("out.md"), LONG, &go).unwrap_err();
        assert!(error.contains("did not start"), "{error}");
    }

    #[test]
    fn one_that_takes_too_long_is_killed_with_what_it_started() {
        let dir = tempfile::tempdir().unwrap();
        // The grandchild would write a file, and keeps standard error open.
        let script = "(sleep 1; echo late > late.txt) & sleep 60";
        let started = Instant::now();
        let error = execute(
            sh(script, dir.path()),
            &output(dir.path()),
            Duration::from_millis(200),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(error, "it took longer than 0 seconds");
        assert!(started.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(1300));
        assert!(!dir.path().join("late.txt").exists());
    }

    #[test]
    fn a_cancelled_one_is_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let stop = AtomicBool::new(true);
        let started = Instant::now();
        let command = sh("sleep 60", dir.path());
        let result = execute(command, &output(dir.path()), LONG, &stop);
        assert_eq!(result.unwrap_err(), "cancelled");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn what_a_finished_parser_left_running_is_not_waited_for() {
        let started = Instant::now();
        assert_eq!(
            try_it("sleep 60 & printf done > out.md", LONG).unwrap(),
            "done"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn parsers_run_offline_with_only_their_own_files() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::installed(dir.path(), Parser::Docling);
        let file = Path::new("/docs/Søren's \"report\".pdf");
        let command = command(&layout, Parser::Docling, file, dir.path()).unwrap();
        assert_eq!(command.get_program(), layout.python());
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args.len(), 4);
        assert_eq!(args[0], "-c");
        assert!(args[1].to_string_lossy().contains("killpg"));
        assert_eq!(args[2], file);
        assert_eq!(args[3], output(dir.path()));
        let env = |key: &str| {
            command
                .get_envs()
                .find(|(name, _)| *name == key)
                .and_then(|(_, value)| value)
        };
        assert_eq!(env("HF_HUB_OFFLINE").unwrap(), "1");
        assert_eq!(env("https_proxy").unwrap(), "http://127.0.0.1:9");
        assert_eq!(env("PYTHONPATH").unwrap(), layout.packages());
        assert_eq!(env("PISTACHIO_MODELS").unwrap(), layout.models());
        assert!(env("HOME").unwrap().to_string_lossy().ends_with("/home"));
        assert!(super::command(&layout, Parser::Builtin, file, dir.path()).is_err());
    }

    #[test]
    fn parsers_with_models_take_turns() {
        let dir = tempfile::tempdir().unwrap();
        let go = AtomicBool::new(false);
        let short = Duration::from_millis(150);
        let first = turn(dir.path(), short, &go).unwrap();
        assert_eq!(turn(dir.path(), short, &go).unwrap_err(), BUSY);
        std::thread::scope(|scope| {
            let second = scope.spawn(|| turn(dir.path(), LONG, &go).is_ok());
            std::thread::sleep(short);
            drop(first);
            assert!(second.join().unwrap());
        });
        // The lock is not taken for what a killed run left.
        sweep(&runs_dir(dir.path()));
        assert!(runs_dir(dir.path()).join("lock").exists());
    }

    #[test]
    fn what_killed_runs_left_is_deleted_later() {
        let dir = tempfile::tempdir().unwrap();
        // Of a process that runs (this one's parent), and of one that does not.
        let parent = rustix::process::getppid().unwrap().as_raw_nonzero();
        let old = runs_dir(dir.path()).join(format!("{parent}-0"));
        let ended = Command::new("/bin/sh").arg("-c").arg("exit").spawn();
        let mut ended = ended.unwrap();
        ended.wait().unwrap();
        let over = runs_dir(dir.path()).join(format!("{}-0", ended.id()));
        for left in [&old, &over] {
            fs::create_dir_all(left).unwrap();
        }
        let run = Run::new(dir.path()).unwrap();
        assert!(old.exists() && run.dir.exists() && !over.exists());
        let long_ago = SystemTime::now() - 2 * LEFT_OVER;
        fs::File::open(&old)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        let path = run.dir.clone();
        drop(run);
        assert!(!path.exists());
        drop(Run::new(dir.path()).unwrap());
        assert!(!old.exists());
    }
}
