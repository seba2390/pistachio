//! Reading a document in a helper process ([`crate::cli`]) that can be stopped.

use std::io::{Read as _, Write as _};
use std::path::Path;
use std::process::{ChildStdin, Stdio};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::Value;

use crate::cli::{FLAG, Reply, Request, WATCHED};
use crate::config::DIR_ENV;

/// Stops a [`read`] when it is dropped.
#[derive(Debug, Default)]
pub struct Stop(Arc<Mutex<State>>);

/// The other end of a [`Stop`].
#[derive(Debug)]
pub struct Stopped(Arc<Mutex<State>>);

#[derive(Debug, Default)]
struct State {
    stopped: bool,
    /// The helper ends when this closes.
    stdin: Option<ChildStdin>,
}

impl Stop {
    pub fn new() -> (Self, Stopped) {
        let stop = Self::default();
        let stopped = Stopped(Arc::clone(&stop.0));
        (stop, stopped)
    }
}

impl Drop for Stop {
    fn drop(&mut self) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.stopped = true;
        state.stdin = None;
    }
}

/// Reads the document at `path` with the settings in `dir`, running `exe` as the helper. Blocks
/// until it is done or `stopped`.
pub fn read(exe: &Path, dir: &Path, path: &Path, stopped: &Stopped) -> Result<Reply, String> {
    let mut child = platform::command(exe)
        .args([FLAG, WATCHED])
        .env(DIR_ENV, dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("its reader did not start: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let request = Request {
            path: path.to_path_buf(),
        };
        let request = serde_json::to_string(&request).map_err(|error| error.to_string())?;
        // A helper that is gone says so below.
        let _ = writeln!(stdin, "{request}").and_then(|()| stdin.flush());
        let mut state = stopped.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.stopped {
            state.stdin = Some(stdin);
        }
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    let _ = child.wait();
    reply(&out)
}

/// The reply is the last line: a library may print something before it.
fn reply(out: &str) -> Result<Reply, String> {
    let reply: Value = out
        .trim_end()
        .lines()
        .last()
        .and_then(|line| serde_json::from_str(line).ok())
        .unwrap_or_default();
    if reply["ok"].as_bool() == Some(true) {
        return serde_json::from_value(reply).map_err(|error| error.to_string());
    }
    Err(reply["error"]
        .as_str()
        .unwrap_or("its reader stopped")
        .to_owned())
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use pretty_assertions::assert_eq;

    use super::*;
    use crate::Parser;

    fn helper(dir: &Path, script: &str) -> PathBuf {
        let path = dir.join("app");
        crate::write_program(&path, script);
        path
    }

    fn try_it(script: &str) -> Result<Reply, String> {
        let dir = tempfile::tempdir().unwrap();
        let exe = helper(dir.path(), script);
        let (_stop, stopped) = Stop::new();
        read(&exe, dir.path(), Path::new("/a b.pdf"), &stopped)
    }

    #[test]
    fn returns_what_the_helper_says() {
        let reply = r##"{"ok":true,"markdown":"# Hi","parser":"docling","note":null}"##;
        let script = format!(
            "read line; test \"$1 $2\" = '--documents-tool watched' || exit 1\n\
             test \"$line\" = '{{\"path\":\"/a b.pdf\"}}' || exit 1\n\
             echo noise; echo; echo '{reply}'"
        );
        let reply = try_it(&script).unwrap();
        assert_eq!(reply.markdown, "# Hi");
        assert_eq!(reply.parser, Parser::Docling);
        let error = try_it(r#"echo '{"ok":false,"error":"it is not a PDF file"}'; exit 1"#);
        assert_eq!(error.unwrap_err(), "it is not a PDF file");
        assert_eq!(try_it("exit 3").unwrap_err(), "its reader stopped");
        let (_stop, stopped) = Stop::new();
        let missing = Path::new("/nonexistent/app");
        let error = read(missing, missing, missing, &stopped).unwrap_err();
        assert!(error.contains("did not start"), "{error}");
    }

    #[test]
    fn it_is_stopped_when_nobody_waits_for_it() {
        let dir = tempfile::tempdir().unwrap();
        // Ends with its input, like the real one.
        let exe = helper(dir.path(), "cat > /dev/null");
        let (stop, stopped) = Stop::new();
        let started = Instant::now();
        std::thread::scope(|scope| {
            let reading = scope.spawn(|| read(&exe, dir.path(), Path::new("/a.pdf"), &stopped));
            std::thread::sleep(Duration::from_millis(200));
            assert!(!reading.is_finished());
            drop(stop);
            assert_eq!(reading.join().unwrap().unwrap_err(), "its reader stopped");
        });
        assert!(started.elapsed() < Duration::from_secs(10));
        // Also before it started.
        let (stop, stopped) = Stop::new();
        drop(stop);
        assert!(read(&exe, dir.path(), Path::new("/a.pdf"), &stopped).is_err());
    }
}
