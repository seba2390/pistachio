//! Process execution for the `git` CLI.
//!
//! Every invocation goes through [`git_command`] so that all commands share the same safety
//! settings: no optional locks (we must never contend with pi for `index.lock`), literal
//! pathspecs, unquoted raw paths, no pagers/colors/external diff drivers, and a stable output
//! format regardless of the user's diff configuration.

use std::ffi::OsStr;
use std::io::{self, Read as _};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;

use crate::WorkspaceError;
use crate::location::{Location, RemoteShell, sh_quote};

/// `-c` overrides applied to every git invocation. They pin down the output format so user
/// configuration (e.g. `diff.noprefix`) can't break parsing in the UI.
const CONFIG_OVERRIDES: &[&str] = &[
    "core.quotepath=off",
    "color.ui=false",
    "diff.noprefix=false",
    "diff.mnemonicPrefix=false",
    "diff.relative=false",
];

/// Environment variables from a parent git process (e.g. when launched from a hook) that would
/// redirect our commands to a different repository.
const SCRUBBED_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_COMMON_DIR",
    "GIT_PREFIX",
];

/// Builds a `git` command running in `dir` with the crate's standard settings.
pub(crate) fn git_command<I, S>(dir: &Path, args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = platform::command("git");
    cmd.current_dir(dir);
    for setting in CONFIG_OVERRIDES {
        cmd.arg("-c").arg(setting);
    }
    cmd.args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        // Stable (English) error messages, so "not a repository" can be recognised.
        .env("LC_ALL", "C")
        .stdin(Stdio::null());
    for var in SCRUBBED_ENV {
        cmd.env_remove(var);
    }
    cmd
}

/// The `sh` script that runs `git <args>` in `dir` on a remote machine with the same settings
/// as [`git_command`]. A missing folder exits with 128 and a "not a directory" message; a
/// missing git with 127.
pub(crate) fn remote_script<I, S>(dir: &Path, args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let dir = sh_quote(&dir.to_string_lossy());
    let mut script = format!(
        "cd -- {dir} 2>/dev/null || {{ printf 'not a directory: %s\\n' {dir} >&2; exit 128; }}\n\
         unset {}\n\
         GIT_OPTIONAL_LOCKS=0 GIT_LITERAL_PATHSPECS=1 GIT_TERMINAL_PROMPT=0 GIT_PAGER=cat LC_ALL=C\n\
         export GIT_OPTIONAL_LOCKS GIT_LITERAL_PATHSPECS GIT_TERMINAL_PROMPT GIT_PAGER LC_ALL\n\
         command -v git >/dev/null 2>&1 || exit 127\n\
         exec git",
        SCRUBBED_ENV.join(" ")
    );
    for setting in CONFIG_OVERRIDES {
        script.push_str(" -c ");
        script.push_str(&sh_quote(setting));
    }
    for arg in args {
        script.push(' ');
        script.push_str(&sh_quote(&arg.as_ref().to_string_lossy()));
    }
    script.push_str(" </dev/null");
    script
}

fn run_remote<I, S>(
    shell: &dyn RemoteShell,
    dir: &Path,
    args: I,
    max_bytes: Option<usize>,
) -> Result<(GitOutput, bool), WorkspaceError>
where
    I: IntoIterator<Item = S> + Clone,
    S: AsRef<OsStr>,
{
    let script = remote_script(dir, args.clone());
    let output = shell
        .run(&script, None, max_bytes)
        .map_err(|error| WorkspaceError::Remote(error.to_string()))?;
    if output.code == Some(127) && !output.truncated {
        return Err(WorkspaceError::GitNotFound);
    }
    Ok((
        GitOutput {
            status: exit_status(output.code.unwrap_or(1)),
            stdout: output.stdout,
            stderr: output.stderr,
            args: render_args(args),
        },
        output.truncated,
    ))
}

/// An [`ExitStatus`] with exit code `code`, for results that came from another machine.
fn exit_status(code: i32) -> ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        ExitStatus::from_raw((code & 0xff) << 8)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt as _;
        ExitStatus::from_raw(code.cast_unsigned())
    }
}

/// Raw result of a finished git process.
#[derive(Debug)]
pub(crate) struct GitOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: String,
    /// Arguments rendered for error messages.
    pub args: String,
}

impl GitOutput {
    /// Converts a non-accepted exit code into [`WorkspaceError::Git`].
    pub fn into_error(self) -> WorkspaceError {
        WorkspaceError::Git {
            args: self.args,
            status: self.status.to_string(),
            stderr: self.stderr.trim().to_owned(),
        }
    }

    fn accepted(&self, ok_codes: &[i32]) -> bool {
        self.status
            .code()
            .is_some_and(|code| ok_codes.contains(&code))
    }
}

fn render_args<I, S>(args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    args.into_iter()
        .map(|a| a.as_ref().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

fn spawn_error(err: io::Error) -> WorkspaceError {
    if err.kind() == io::ErrorKind::NotFound {
        WorkspaceError::GitNotFound
    } else {
        WorkspaceError::Io(err)
    }
}

/// Runs git to completion, returning its output whatever the exit code.
///
/// Fails only if the process could not be spawned ([`WorkspaceError::GitNotFound`] when git is
/// missing). Callers must make sure `dir` exists, otherwise that is reported as missing git too.
pub(crate) fn run<I, S>(loc: &Location, dir: &Path, args: I) -> Result<GitOutput, WorkspaceError>
where
    I: IntoIterator<Item = S> + Clone,
    S: AsRef<OsStr>,
{
    if let Location::Remote(shell) = loc {
        return run_remote(shell.as_ref(), dir, args, None).map(|(output, _)| output);
    }
    let output = git_command(dir, args.clone())
        .output()
        .map_err(spawn_error)?;
    Ok(GitOutput {
        status: output.status,
        stdout: output.stdout,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        args: render_args(args),
    })
}

/// Runs git and returns stdout, treating any exit code outside `ok_codes` as an error.
pub(crate) fn run_ok<I, S>(
    loc: &Location,
    dir: &Path,
    args: I,
    ok_codes: &[i32],
) -> Result<Vec<u8>, WorkspaceError>
where
    I: IntoIterator<Item = S> + Clone,
    S: AsRef<OsStr>,
{
    let output = run(loc, dir, args)?;
    if output.accepted(ok_codes) {
        Ok(output.stdout)
    } else {
        Err(output.into_error())
    }
}

/// Runs git, reading at most `max_bytes` of stdout.
///
/// Returns the captured bytes and whether output was cut off. When the limit is hit the process
/// is killed instead of buffering the rest, so huge diffs never land in memory.
pub(crate) fn run_capped<I, S>(
    loc: &Location,
    dir: &Path,
    args: I,
    max_bytes: usize,
    ok_codes: &[i32],
) -> Result<(Vec<u8>, bool), WorkspaceError>
where
    I: IntoIterator<Item = S> + Clone,
    S: AsRef<OsStr>,
{
    if let Location::Remote(shell) = loc {
        let (output, truncated) = run_remote(shell.as_ref(), dir, args, Some(max_bytes))?;
        if truncated {
            return Ok((output.stdout, true));
        }
        return if output.accepted(ok_codes) {
            Ok((output.stdout, false))
        } else {
            Err(output.into_error())
        };
    }
    let mut child = git_command(dir, args.clone())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(spawn_error)?;

    // Drain stderr concurrently so a chatty process can't deadlock on a full pipe.
    let stderr_reader = child.stderr.take().map(|mut stderr| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf);
            buf
        })
    });

    let mut stdout_buf = Vec::new();
    let read_result = match child.stdout.take() {
        Some(stdout) => stdout
            .take(
                u64::try_from(max_bytes)
                    .unwrap_or(u64::MAX)
                    .saturating_add(1),
            )
            .read_to_end(&mut stdout_buf)
            .map(drop),
        None => Ok(()),
    };

    let truncated = stdout_buf.len() > max_bytes;
    if truncated || read_result.is_err() {
        // Either we have enough, or reading failed; don't wait for the rest.
        let _ = child.kill();
    }
    let status = child.wait()?;
    let stderr = stderr_reader
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();
    read_result?;

    if truncated {
        stdout_buf.truncate(max_bytes);
        return Ok((stdout_buf, true));
    }

    let output = GitOutput {
        status,
        stdout: stdout_buf,
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        args: render_args(args),
    };
    if output.accepted(ok_codes) {
        Ok((output.stdout, false))
    } else {
        Err(output.into_error())
    }
}
