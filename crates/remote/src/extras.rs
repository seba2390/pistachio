//! More things to do with a host: interactive connections (password and passphrase prompts),
//! remote terminals, the [`workspace::RemoteShell`] used by the diff / files panels, uploading
//! a file, and replacing a changed host key.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use crate::RemoteError;
use crate::config::resolve_full;
use crate::error::meaningful_lines;
use crate::process;
use crate::ssh::{LEGACY_MANAGED_DIR, MANAGED_DIR, Ssh, remote_word, script_result, sh_quote};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(120);
const SHELL_TIMEOUT: Duration = Duration::from_secs(90);

/// Run by `/bin/sh` on the host for a terminal: changes to `$1` (falling back to home), puts
/// pistachio's own pi on `PATH`, then becomes the user's login shell. `$2`, when set, is a
/// command to run first in the login shell.
const TERMINAL_SCRIPT: &str = r#"d=$1
case $d in ""|"~") cd ;; "~/"*) cd -- "$HOME/${d#??}" ;; *) cd -- "$d" ;; esac 2>/dev/null || cd
for m in "$HOME/LEGACY_MANAGED_DIR/bin" "$HOME/MANAGED_DIR/bin"; do
  [ -d "$m" ] && PATH=$m:$PATH && export PATH
done
s=${SHELL:-/bin/sh}
if [ -n "$2" ]; then
  case ${s##*/} in fish) "$s" -l -c "$2" ;; *) "$s" -lc "$2" ;; esac
fi
exec "$s" -l"#;

/// [`TERMINAL_SCRIPT`] with its placeholders filled in (the newest install wins on `PATH`).
fn terminal_script() -> String {
    TERMINAL_SCRIPT
        .replace("LEGACY_MANAGED_DIR", LEGACY_MANAGED_DIR)
        .replace("MANAGED_DIR", MANAGED_DIR)
}

/// Writes stdin to `$1` atomically (mode 0600 for new files), keeping a copy of an existing
/// file as `$1.bak-pistachio` unless `$2` is `0`.
const UPLOAD_SCRIPT: &str = r#"f=$1
umask 077
mkdir -p -- "$(dirname -- "$f")" || exit 1
[ "$2" != 0 ] && [ -f "$f" ] && cp -p -- "$f" "$f.bak-pistachio"
cat > "$f.pistachio-tmp" && mv -- "$f.pistachio-tmp" "$f""#;

impl Ssh {
    /// Like [`Ssh::connect`], but lets ssh ask for passwords, passphrases or one-time codes
    /// through `askpass_env` (see [`crate::askpass`]) instead of failing in batch mode.
    /// With `trust_recorded_key`, only the host key the user confirmed is accepted (see
    /// [`Ssh::connect`]).
    pub fn connect_interactive(
        &self,
        askpass_env: &[(OsString, OsString)],
        trust_recorded_key: bool,
    ) -> Result<(), RemoteError> {
        let _changing = self.start_connecting()?;
        if self.is_connected() {
            return Ok(());
        }
        self.ensure_control_dir()?;
        // ssh keeps the first value it sees, so these override the usual `BatchMode=yes`.
        let mut options = ["-o", "BatchMode=no", "-o", "NumberOfPasswordPrompts=3"]
            .map(OsString::from)
            .to_vec();
        if trust_recorded_key {
            options.extend(self.pinned_key_args()?);
        }
        self.start_shared(&options, askpass_env, CONNECT_TIMEOUT)?;
        if trust_recorded_key {
            self.save_recorded_key()?;
        }
        Ok(())
    }

    /// The program and arguments for a terminal on the host: an interactive login shell in
    /// `cwd` (a remote path; `~` for home), with `~/.pistachio/server/bin` on `PATH`. With
    /// `command`, the login shell runs it first and stays open afterwards.
    pub fn terminal_command(&self, cwd: &str, command: Option<&str>) -> (PathBuf, Vec<OsString>) {
        let mut args: Vec<OsString> = vec!["-t".into()];
        args.extend(self.unshared_args());
        args.extend(self.base_args());
        args.push("--".into());
        let script = terminal_script();
        let mut remote = format!(
            "exec /bin/sh -c {} pistachio {}",
            remote_word(&script),
            remote_word(cwd)
        );
        if let Some(command) = command {
            remote.push(' ');
            remote.push_str(&remote_word(command));
        }
        args.push(remote.into());
        (Ssh::program(), args)
    }

    /// Writes `contents` to `path` on the host (parent folders created, an existing file kept
    /// as `<path>.bak-pistachio`, new files private).
    pub fn upload(&self, path: &str, contents: Vec<u8>) -> Result<(), RemoteError> {
        self.write_file(path, contents, true)
    }

    /// Writes `contents` to `path` on the host atomically (parent folders created, new files
    /// private), keeping the previous file as `<path>.bak-pistachio` when `backup` is set.
    pub fn write_file(
        &self,
        path: &str,
        contents: Vec<u8>,
        backup: bool,
    ) -> Result<(), RemoteError> {
        let script = format!(
            "set -- {} {}\n{UPLOAD_SCRIPT}",
            sh_quote(path),
            if backup { "1" } else { "0" }
        );
        let output = process::run(self.script_command(&script)?, Some(contents), SHELL_TIMEOUT)?;
        script_result(output).map(drop)
    }

    /// The contents of file `path` on the host, or `None` if there is no such file.
    pub fn read_file(&self, path: &str) -> Result<Option<Vec<u8>>, RemoteError> {
        let script = format!(
            "if [ -f {0} ]; then printf 1; cat -- {0}; else printf 0; fi",
            sh_quote(path)
        );
        let output = self.exec_bytes(&script, SHELL_TIMEOUT)?;
        match output.stdout.split_first() {
            Some((b'1', rest)) => Ok(Some(rest.to_vec())),
            Some((b'0', _)) => Ok(None),
            _ => Err(RemoteError::Failed(format!("could not read {path}"))),
        }
    }

    /// Runs a POSIX script on the host with `stdin` (for secrets that must not appear on a
    /// command line) and returns its stdout.
    pub fn exec_with_input(&self, script: &str, stdin: Vec<u8>) -> Result<Vec<u8>, RemoteError> {
        let output = process::run(self.script_command(script)?, Some(stdin), SHELL_TIMEOUT)?;
        script_result(output).map(|output| output.stdout)
    }

    /// Whether `path` exists on the host.
    pub fn exists(&self, path: &str) -> Result<bool, RemoteError> {
        let script = format!("[ -e {} ] && echo yes || echo no", sh_quote(path));
        Ok(self.exec(&script)?.trim() == "yes")
    }

    /// Removes the recorded key of this host from `known_hosts` (`ssh-keygen -R`, for the
    /// name and, on a non-standard port, `[name]:port`), after the user confirmed that its
    /// change is expected. Returns what was removed, for the UI.
    pub fn remove_known_host(&self) -> Result<String, RemoteError> {
        let full = resolve_full(self.target())?;
        let name = full
            .host_key_alias
            .clone()
            .unwrap_or_else(|| full.resolved.hostname.clone());
        let mut keys = vec![name.clone()];
        if full.resolved.port != 22 && full.host_key_alias.is_none() {
            keys.push(format!("[{name}]:{}", full.resolved.port));
        }
        let file = full.known_hosts.as_deref().map(|file| {
            match (file.strip_prefix("~/"), dirs::home_dir()) {
                (Some(rest), Some(home)) => home.join(rest),
                _ => PathBuf::from(file),
            }
        });
        for key in &keys {
            let mut command = platform::command("ssh-keygen");
            command.arg("-R").arg(key);
            if let Some(file) = &file {
                command.arg("-f").arg(file);
            }
            let output = process::run(command, None, Duration::from_secs(10))?;
            if !output.status.success() {
                let stderr = output.stderr_lossy();
                let lines = meaningful_lines(&stderr);
                return Err(RemoteError::Failed(lines.join("\n")));
            }
        }
        Ok(keys.join(", "))
    }
}

impl workspace::RemoteShell for Ssh {
    fn run(
        &self,
        script: &str,
        stdin: Option<Vec<u8>>,
        max_stdout: Option<usize>,
    ) -> std::io::Result<workspace::ShellOutput> {
        let command = self.script_command(script).map_err(std::io::Error::other)?;
        let result = match max_stdout {
            Some(max) => process::run_capped(command, stdin, SHELL_TIMEOUT, max),
            None => process::run(command, stdin, SHELL_TIMEOUT).map(|output| (output, false)),
        };
        let (output, truncated) = result.map_err(std::io::Error::other)?;
        // 255 is ssh's own failure (connection lost, …): the script did not run.
        if output.status.code() == Some(255) && !truncated {
            return Err(std::io::Error::other(crate::classify(
                &output.stderr_lossy(),
            )));
        }
        Ok(workspace::ShellOutput {
            code: if truncated {
                None
            } else {
                output.status.code()
            },
            stderr: output.stderr_lossy(),
            stdout: output.stdout,
            truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;
    use crate::host::Target;

    #[test]
    fn terminal_command_runs_a_login_shell_in_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = Ssh::with_control_dir(
            Target::Alias {
                alias: "box".into(),
            },
            dir.path().join("cm"),
        );
        let (program, args) = ssh.terminal_command("/srv/my app", Some("echo hi"));
        assert_eq!(program, PathBuf::from("ssh"));
        assert_eq!(args[0], "-t");
        let remote = args.last().unwrap().to_string_lossy().into_owned();
        assert!(remote.starts_with("exec /bin/sh -c "), "{remote}");
        assert!(
            remote.ends_with(" pistachio '/srv/my app' 'echo hi'"),
            "{remote}"
        );

        // The script itself: run it locally with a fake login shell that reports where it is.
        let home = tempfile::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        std::fs::create_dir_all(home.join("proj")).unwrap();
        std::fs::create_dir_all(home.join(MANAGED_DIR).join("bin")).unwrap();
        let fake = home.join("fakesh");
        std::fs::write(&fake, "#!/bin/sh\nif [ \"$1\" = -lc ]; then echo \"ran $2\"; exit 0; fi\npwd -P\nprintf '%s\\n' \"$PATH\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let script = terminal_script();
        let out = Command::new("/bin/sh")
            .args(["-c", &script, "pistachio", "~/proj", "echo hi"])
            .env("HOME", &home)
            .env("SHELL", &fake)
            .output()
            .unwrap();
        let stdout = String::from_utf8(out.stdout).unwrap();
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines[0], "ran echo hi");
        assert_eq!(lines[1], home.join("proj").to_str().unwrap());
        assert!(lines[2].starts_with(home.join(MANAGED_DIR).join("bin").to_str().unwrap()));
    }

    #[test]
    fn upload_script_keeps_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a/b/models.json");
        let run = |input: &str| {
            let mut child = Command::new("/bin/sh")
                .args(["-c", UPLOAD_SCRIPT, "sh", file.to_str().unwrap(), "1"])
                .stdin(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write as _;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
            assert!(child.wait().unwrap().success());
        };
        run("one");
        run("two");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "two");
        assert_eq!(
            std::fs::read_to_string(file.with_extension("json.bak-pistachio")).unwrap(),
            "one"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0);
        }
    }
}
