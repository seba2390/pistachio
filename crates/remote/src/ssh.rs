//! Talking to one remote host through the system `ssh` binary.
//!
//! Every invocation shares one master connection per host (OpenSSH `ControlMaster`), so after
//! the first connection a remote command costs a few tens of milliseconds instead of a full
//! handshake. The master lingers for ten minutes after its last user, and any call quietly
//! starts a new one when needed.
//!
//! Remote scripts are POSIX `sh` only (the host may be Linux or macOS). ssh hands the command
//! line to the user's login shell, so it is quoted in a way bash, zsh, dash, ksh and fish all
//! read the same ([`remote_word`]); the scripts themselves always run under `/bin/sh`, and a
//! fish user's login `PATH` is taken from fish. csh/tcsh login shells are not supported.

use std::ffi::OsString;
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use crate::config::resolve_full;
use crate::error::meaningful_lines;
use crate::host::Target;
use crate::local_network;
use crate::process::{self, Output};
use crate::tunnel::Tunnel;
use crate::{RemoteError, classify};

/// Unix socket paths are limited to 104 bytes (including the NUL) on macOS. ssh binds
/// `<dir>/<40-char %C hash>.<16 random chars>` before renaming it into place, so the control
/// directory itself may be at most 104 - 1 - 1 - 40 - 17 = 45 bytes long.
const MAX_CONTROL_DIR_LEN: usize = 45;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(45);
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const EXEC_TIMEOUT: Duration = Duration::from_secs(60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const LIST_TIMEOUT: Duration = Duration::from_secs(20);
const KEYSCAN_TIMEOUT: Duration = Duration::from_secs(20);

/// Run by the remote login shell to start a program: `$1` is the working folder (empty for
/// none; `~` and `~/…` expand to `$HOME`), then `PISTACHIO_ENV:NAME=value` words to export,
/// and the rest is the command. A leading `~/` in the
/// program expands too, and an absolute program's own folder is put in front of `PATH` so a
/// node script installed next to its `node` (nvm, Homebrew) finds it.
///
/// With [`RELAY_PORT_ENV`] set, the program only starts when this computer's model relay proves
/// that it answers on the forwarded port: the forward is gone when the shared connection was made
/// again just before (pi's own `ssh` then became the new master, without the forward), and on a
/// shared host someone else may listen on its port by then. It is asked the challenge in the
/// managed folder's `relay-proof` ([`crate::link`]), whose answer only the relay knows. curl or bash ask, without
/// the challenge among their arguments, where others could see it; a host with neither starts the
/// program anyway.
pub(crate) const LAUNCH_SCRIPT: &str = concat!(
    r#"d=$1; shift; while :; do case $1 in PISTACHIO_ENV:*=*) export "${1#PISTACHIO_ENV:}"; shift ;; *) break ;; esac; done; "#,
    r#"case ${PISTACHIO_RELAY_PORT-} in "") ;; *) q=; c=; w=; g=; read -r q c w 2>/dev/null <"${PI_CODING_AGENT_DIR-}/relay-proof"; "#,
    r#"if command -v curl >/dev/null 2>&1; then g=$(printf 'url = "http://127.0.0.1:%s/_pistachio/prove/%s"' "$q" "$c" | curl -q -s --noproxy '*' -m 5 -K - 2>/dev/null); "#,
    r#"elif command -v bash >/dev/null 2>&1; then g=$(printf '%s %s' "$q" "$c" | bash -c 'read -r q c; exec 3<>"/dev/tcp/127.0.0.1/$q" || exit; printf "GET /_pistachio/prove/%s HTTP/1.0\r\n\r\n" "$c" >&3; l=; while IFS= read -r -t 5 l <&3; do :; done; printf %s "$l"' 2>/dev/null); "#,
    r#"else g=$w; fi; [ -n "$w" ] && [ "$g" = "$w" ] || { echo "This computer's models can't be reached from the host: the connection was made again. Reconnect to use them." >&2; exit 1; } ;; esac; "#,
    r#"case $d in "") ;; "~") cd || exit 1 ;; "~/"*) cd -- "$HOME/${d#??}" || exit 1 ;; *) cd -- "$d" || exit 1 ;; esac; case $1 in "~/"*) p=$HOME/${1#??}; shift; set -- "$p" "$@" ;; esac; case $1 in /*) PATH=${1%/*}:$PATH; export PATH ;; esac; exec "$@""#
);

/// Set for a program on the host to check first that the model relay answers on its forwarded
/// port (see [`LAUNCH_SCRIPT`]).
pub const RELAY_PORT_ENV: &str = "PISTACHIO_RELAY_PORT";

/// Where pistachio installs its own copy of pi on a host (relative to `$HOME`).
pub const MANAGED_DIR: &str = ".pistachio/server";

/// Where installs made before the rename from PI-gui live; still used when found.
pub const LEGACY_MANAGED_DIR: &str = ".pi-gui/server";

/// Sets `PATH` to the login shell's (run through fish's own syntax when that is the login
/// shell). Shared by the scripts below; `$s` is the login shell afterwards.
pub(crate) const LOGIN_PATH: &str = r#"s=${SHELL:-/bin/sh}
case ${s##*/} in
  fish) lp=$("$s" -lc 'string join : $PATH' </dev/null 2>/dev/null | tail -n 1) ;;
  *) lp=$("$s" -lc 'printf "%s\n" "$PATH"' </dev/null 2>/dev/null | tail -n 1) ;;
esac
case $lp in /*) PATH=$lp:$PATH; export PATH ;; esac"#;

/// Probes the host. `$1` is the pi command. Prints `PISTACHIO_*=` marker lines so whatever the
/// user's profile scripts print is ignored. pi is looked up on the login shell's `PATH`, then
/// an interactive shell's (where `.bashrc` additions such as nvm live), then pistachio's own
/// install. Also reports pi's agent and sessions folders (honouring `PI_CODING_AGENT_DIR`,
/// `PI_CODING_AGENT_SESSION_DIR` and an absolute `sessionDir` setting).
const PROBE_SCRIPT: &str = r#"printf 'PISTACHIO_HOME=%s\n' "$HOME"
printf 'PISTACHIO_SYSTEM=%s\n' "$(uname -sm)"
c=$1; case $c in "~/"*) c=$HOME/${c#??} ;; esac
ENV_SCRIPT
p=$(command -v "$c" 2>/dev/null)
case ${s##*/} in fish) ;; *)
  case $p in /*) ;; *) p=$("$s" -ic 'command -v "$1"' sh "$c" </dev/null 2>/dev/null | tail -n 1) ;; esac ;;
esac
m=$HOME/MANAGED_DIR/bin/pi
[ -x "$m" ] || m=$HOME/LEGACY_MANAGED_DIR/bin/pi
case $p in /*) ;; *) [ "$c" = pi ] && [ -x "$m" ] && p=$m ;; esac
case $p in /*)
  printf 'PISTACHIO_PI=%s\n' "$p"
  v=$(PATH=${p%/*}:$PATH "$p" --version </dev/null 2>/dev/null | tail -n 1)
  printf 'PISTACHIO_VERSION=%s\n' "$v" ;;
esac
ev() { "$s" -lc "printenv $1 | sed 's/^/PISTACHIO_EV=/'" </dev/null 2>/dev/null | sed -n 's/^PISTACHIO_EV=//p' | tail -n 1; }
a=$(ev PI_CODING_AGENT_DIR)
case $a in /*|"~/"*) ;; *) a= ;; esac
[ -n "$a" ] || a=$HOME/.pi/agent
case $a in "~/"*) a=$HOME/${a#??} ;; esac
d=$(ev PI_CODING_AGENT_SESSION_DIR)
case $d in /*) ;; "~/"*) d=$HOME/${d#??} ;; *) d= ;; esac
if [ -z "$d" ] && [ -f "$a/settings.json" ]; then
  d=$(sed -n 's/.*"sessionDir"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$a/settings.json" | head -n 1)
  case $d in /*) ;; *) d= ;; esac
fi
[ -n "$d" ] || d=$a/sessions
printf 'PISTACHIO_AGENT=%s\n' "$a"
printf 'PISTACHIO_SESSIONS=%s\n' "$d"
exit 0"#;

/// Lists subfolders of `$1` (empty or `~` = home): the canonical path, then every directory
/// name (dot folders too when `$2` is 1), each terminated by a NUL byte.
const LIST_SCRIPT: &str = r#"fail() { printf '%s\n' "$1" >&2; exit 1; }
case $1 in ""|"~") d=$HOME ;; "~/"*) d=$HOME/${1#??} ;; *) d=$1 ;; esac
[ -d "$d" ] || fail "No such folder: $d"
cd -- "$d" 2>/dev/null || fail "Permission denied: $d"
printf '%s\000' "$(pwd -P)"
for e in *; do [ -d "$e" ] && printf '%s\000' "$e"; done
if [ "$2" = 1 ]; then
  for e in .*; do case $e in .|..) ;; *) [ -d "$e" ] && printf '%s\000' "$e" ;; esac; done
fi
exit 0"#;

/// Creates folder `$2` inside `$1` (or `$2` itself when absolute) and prints its canonical
/// path.
const MKDIR_SCRIPT: &str = r#"case $1 in ""|"~") b=$HOME ;; "~/"*) b=$HOME/${1#??} ;; *) b=$1 ;; esac
case $2 in /*) d=$2 ;; "~") d=$HOME ;; "~/"*) d=$HOME/${2#??} ;; *) d=$b/$2 ;; esac
mkdir -p -- "$d" || exit 1
cd -- "$d" && pwd -P"#;

/// A connection to one remote host. Cheap to clone; holds no live resources itself (the shared
/// master connection lives in a background `ssh` process), but for the tunnel there is instead
/// where OpenSSH cannot share connections (Windows).
#[derive(Debug, Clone)]
pub struct Ssh {
    target: Target,
    control_dir: PathBuf,
    /// Shared by clones (see [`Ssh::close`]).
    switch: Arc<Switch>,
    /// Whether commands share one connection (OpenSSH `ControlMaster`).
    sharing: bool,
}

/// Whether the user turned a connection off.
#[derive(Debug, Default)]
struct Switch {
    closed: AtomicBool,
    /// Held while the master connection is started or stopped, so that turning a connection
    /// off ends one that was still being made, and turning it on again keeps it.
    changing: Mutex<()>,
    /// The connection where none can be shared.
    tunnel: Mutex<Option<Tunnel>>,
}

/// What [`Ssh::probe`] found on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// The remote `$HOME`.
    pub home: String,
    /// pi's agent folder there (`~/.pi/agent` unless overridden).
    pub agent_dir: String,
    /// Where pi keeps session files there.
    pub sessions_dir: String,
    /// Absolute path of the pi executable, if found.
    pub pi_path: Option<String>,
    /// Last line `pi --version` printed, if pi was found and printed anything.
    pub pi_version: Option<String>,
    /// `uname -sm`, e.g. `Linux aarch64` or `Darwin arm64`.
    pub system: String,
}

/// Subfolders of a remote folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirListing {
    /// Canonical absolute path of the listed folder.
    pub path: String,
    /// Its parent, or `None` at `/`.
    pub parent: Option<String>,
    /// Names of subfolders (symlinks to folders included; dot folders only when asked for),
    /// sorted case-insensitively.
    pub dirs: Vec<String>,
}

impl Ssh {
    /// A connection using the default control-socket directory ([`Ssh::default_control_dir`]).
    pub fn new(target: Target) -> Self {
        Self::with_control_dir(target, Self::default_control_dir())
    }

    /// A connection keeping its control socket in `dir` (at most 45 bytes long; see
    /// [`Ssh::default_control_dir`]).
    pub fn with_control_dir(target: Target, dir: PathBuf) -> Self {
        Self {
            target,
            control_dir: dir,
            switch: Arc::default(),
            sharing: cfg!(unix),
        }
    }

    /// As on Windows, whose OpenSSH has no connection sharing (it needs Unix sockets): each
    /// command opens its own connection, and a tunnel stands for the shared one.
    #[must_use]
    pub fn without_sharing(mut self) -> Self {
        self.sharing = false;
        self
    }

    pub(crate) fn shares(&self) -> bool {
        self.sharing
    }

    pub(crate) fn tunnel_slot(&self) -> &Mutex<Option<Tunnel>> {
        &self.switch.tunnel
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    pub fn control_dir(&self) -> &Path {
        &self.control_dir
    }

    /// The ssh executable (found on `PATH`).
    pub fn program() -> PathBuf {
        PathBuf::from("ssh")
    }

    /// `<agent dir>/pistachio/cm`, or `/tmp/pistachio-cm-<uid>` when that path is too long for a
    /// control socket.
    pub fn default_control_dir() -> PathBuf {
        pi_sessions::agent_dir()
            .map(|agent| agent.join(pi_sessions::APP_DIR).join("cm"))
            .filter(|dir| dir.is_absolute() && dir.as_os_str().len() <= MAX_CONTROL_DIR_LEN)
            .unwrap_or_else(|| PathBuf::from(format!("/tmp/pistachio-cm-{}", user_tag())))
    }

    /// Options shared by every invocation, then the target's own options and destination:
    ///
    /// `-o ControlMaster=auto -o ControlPath=<dir>/%C -o ControlPersist=600 -o BatchMode=yes
    /// -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=3
    /// -o LogLevel=ERROR`, plus for a custom target `-p`, `-i` (with `IdentitiesOnly=yes`),
    /// `-J` and `-l`; last the alias or host.
    ///
    /// ssh uses the first value it sees for an option, so options that must win are placed
    /// before these.
    pub fn base_args(&self) -> Vec<OsString> {
        let mut control_path = OsString::from("ControlPath=");
        control_path.push(escape_percent(&self.control_dir));
        control_path.push("/%C");
        let mut args: Vec<OsString> = Vec::new();
        let sharing = if self.sharing {
            vec![
                OsString::from("ControlMaster=auto"),
                control_path,
                OsString::from("ControlPersist=600"),
            ]
        } else {
            Vec::new()
        };
        for option in sharing.into_iter().chain([
            OsString::from("BatchMode=yes"),
            OsString::from("ConnectTimeout=10"),
            OsString::from("ServerAliveInterval=15"),
            OsString::from("ServerAliveCountMax=3"),
            OsString::from("LogLevel=ERROR"),
        ]) {
            args.push("-o".into());
            args.push(option);
        }
        args.extend(Self::target_args(&self.target));
        args
    }

    /// Only the target's own options and its destination (what `ssh -G` needs).
    pub(crate) fn target_args(target: &Target) -> Vec<OsString> {
        match target {
            Target::Alias { alias } => vec![alias.into()],
            Target::Custom {
                host,
                user,
                port,
                identity_file,
                proxy_jump,
            } => {
                let mut args: Vec<OsString> = Vec::new();
                if let Some(port) = port {
                    args.extend(["-p".into(), port.to_string().into()]);
                }
                if let Some(identity) = identity_file {
                    args.extend([
                        "-i".into(),
                        identity.into(),
                        "-o".into(),
                        "IdentitiesOnly=yes".into(),
                    ]);
                }
                if let Some(jump) = proxy_jump.as_deref().filter(|jump| !jump.is_empty()) {
                    args.extend(["-J".into(), jump.into()]);
                }
                if let Some(user) = user.as_deref().filter(|user| !user.is_empty()) {
                    args.extend(["-l".into(), user.into()]);
                }
                args.push(host.into());
                args
            }
        }
    }

    /// Makes sure the shared master connection is up, starting it if needed (blocking, up to
    /// ~45 s). With `trust_recorded_key` the connection accepts exactly the host key that
    /// [`Self::host_key_fingerprint`] recorded and showed to the user (nothing else, so a key
    /// swapped in between is refused), and that key is then added to `known_hosts`. A changed
    /// key is never accepted.
    ///
    /// Implemented as `ssh -o ControlMaster=auto … true` (with `ControlPersist` the first
    /// client backgrounds a master) rather than `ssh -fN -o ControlMaster=yes`: `auto` replaces
    /// a stale socket and never leaves a second, unshared connection behind when a master
    /// already runs.
    ///
    /// On macOS, a host on the local network that can't be reached is tried again for a while
    /// (~15 s): macOS may just be asking the user whether to allow it. Turning the connection
    /// off ends that.
    pub fn connect(&self, trust_recorded_key: bool) -> Result<(), RemoteError> {
        local_network::retrying(|| self.connect_once(trust_recorded_key), thread::sleep)
    }

    fn connect_once(&self, trust_recorded_key: bool) -> Result<(), RemoteError> {
        let _changing = self.start_connecting()?;
        if self.is_connected() {
            return Ok(());
        }
        self.ensure_control_dir()?;
        let options = if trust_recorded_key {
            self.pinned_key_args()?
        } else {
            Vec::new()
        };
        self.start_shared(&options, &[], CONNECT_TIMEOUT)?;
        if trust_recorded_key {
            self.save_recorded_key()?;
        }
        Ok(())
    }

    /// Starts the shared connection, or the tunnel there is instead. `options` go before the
    /// usual ones.
    pub(crate) fn start_shared(
        &self,
        options: &[OsString],
        env: &[(OsString, OsString)],
        timeout: Duration,
    ) -> Result<(), RemoteError> {
        if !self.sharing {
            return self.open_tunnel(options, env, timeout);
        }
        let mut command = self.ssh_command();
        command
            .arg("-T")
            .args(options)
            .args(self.base_args())
            .args(["--", "true"])
            .envs(env.iter().map(|(key, value)| (key, value)));
        let output = process::run(command, None, timeout)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(ssh_failure(&output))
        }
    }

    /// Where [`Self::host_key_fingerprint`] records the host's key until the user decides
    /// (in the private control directory).
    fn recorded_key_file(&self) -> PathBuf {
        use std::hash::{DefaultHasher, Hash as _, Hasher as _};
        let mut hasher = DefaultHasher::new();
        self.target.display().hash(&mut hasher);
        self.control_dir
            .join(format!("hostkey-{:016x}.known_hosts", hasher.finish()))
    }

    /// Options that make ssh accept only the recorded key (ssh keeps the first value of an
    /// option, so these win over the user's config).
    pub(crate) fn pinned_key_args(&self) -> Result<Vec<OsString>, RemoteError> {
        let file = self.recorded_key_file();
        if fs::metadata(&file).map_or(true, |meta| meta.len() == 0) {
            return Err(RemoteError::Failed(
                "The host key was not fetched; connect again to see it.".to_owned(),
            ));
        }
        let mut known = OsString::from("UserKnownHostsFile=");
        known.push(&file);
        Ok(vec![
            "-o".into(),
            known,
            "-o".into(),
            "GlobalKnownHostsFile=/dev/null".into(),
            "-o".into(),
            "StrictHostKeyChecking=yes".into(),
        ])
    }

    /// After a connection trusted the recorded key: appends it to the user's `known_hosts`
    /// (the first `UserKnownHostsFile`, `~/.ssh/known_hosts` by default) and forgets it.
    pub(crate) fn save_recorded_key(&self) -> Result<(), RemoteError> {
        use std::io::Write as _;
        let file = self.recorded_key_file();
        let Ok(keys) = fs::read(&file) else {
            return Ok(());
        };
        let full = resolve_full(&self.target)?;
        let known = match full.known_hosts.as_deref() {
            Some(path) => match (path.strip_prefix("~/"), dirs::home_dir()) {
                (Some(rest), Some(home)) => home.join(rest),
                _ => PathBuf::from(path),
            },
            None => dirs::home_dir()
                .map(|home| home.join(".ssh/known_hosts"))
                .ok_or_else(|| RemoteError::Failed("no home directory".to_owned()))?,
        };
        let fail = |error: io::Error| {
            RemoteError::Failed(format!("could not update {}: {error}", known.display()))
        };
        if let Some(dir) = known.parent()
            && !dir.exists()
        {
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            builder.mode(0o700);
            builder.create(dir).map_err(fail)?;
        }
        let needs_newline = fs::read(&known)
            .is_ok_and(|existing| !existing.is_empty() && !existing.ends_with(b"\n"));
        let mut options = fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut out = options.open(&known).map_err(fail)?;
        if needs_newline {
            out.write_all(b"\n").map_err(fail)?;
        }
        out.write_all(&keys).map_err(fail)?;
        let _ = fs::remove_file(&file);
        Ok(())
    }

    /// Whether a master connection is running (`ssh -O check`, local only), or the tunnel there
    /// is instead.
    pub fn is_connected(&self) -> bool {
        if !self.sharing {
            return self.tunnel_pid().is_some();
        }
        let mut command = self.ssh_command();
        command.args(["-O", "check"]).args(self.base_args());
        process::run(command, None, CHECK_TIMEOUT).is_ok_and(|output| output.status.success())
    }

    /// Stops the master connection (`ssh -O exit`), which also ends every session running
    /// over it. Errors are ignored.
    pub fn disconnect(&self) {
        if !self.sharing {
            self.close_tunnel();
            return;
        }
        let mut command = self.ssh_command();
        command.args(["-O", "exit"]).args(self.base_args());
        if let Err(error) = process::run(command, None, CHECK_TIMEOUT) {
            log::debug!("ssh -O exit for {}: {error}", self.target.display());
        }
    }

    /// Turns the connection off: until [`Ssh::reopen`], nothing here connects or runs scripts
    /// (any call would quietly start a new master). Returns at once; stop the running master
    /// with [`Ssh::disconnect_closed`].
    pub fn close(&self) {
        self.switch.closed.store(true, Ordering::SeqCst);
    }

    /// Allows connecting again after [`Ssh::close`].
    pub fn reopen(&self) {
        self.switch.closed.store(false, Ordering::SeqCst);
    }

    pub fn is_closed(&self) -> bool {
        self.switch.closed.load(Ordering::SeqCst)
    }

    /// Stops the master connection of a closed `Ssh` (blocking), waiting for one that is still
    /// being made. Does nothing when it was reopened meanwhile.
    pub fn disconnect_closed(&self) {
        let _changing = self.lock_changing();
        if self.is_closed() {
            self.disconnect();
        }
    }

    fn lock_changing(&self) -> MutexGuard<'_, ()> {
        self.switch
            .changing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// To hold while starting the master connection; fails when it is turned off.
    pub(crate) fn start_connecting(&self) -> Result<MutexGuard<'_, ()>, RemoteError> {
        let guard = self.lock_changing();
        self.check_open()?;
        Ok(guard)
    }

    fn check_open(&self) -> Result<(), RemoteError> {
        if self.is_closed() {
            return Err(RemoteError::Failed(
                "The connection is turned off.".to_owned(),
            ));
        }
        Ok(())
    }

    /// Options for a program the user starts on a connection that is turned off: it gets a
    /// connection of its own, which ends with it. (ssh keeps the first value of an option, so
    /// these go before [`Ssh::base_args`].)
    pub(crate) fn unshared_args(&self) -> Vec<OsString> {
        if self.is_closed() {
            ["-o", "ControlMaster=no", "-o", "ControlPath=none"]
                .map(OsString::from)
                .to_vec()
        } else {
            Vec::new()
        }
    }

    /// Runs a POSIX `sh` script on the host (`sh -c <script>`, stdin `/dev/null`) and returns
    /// its stdout. Times out after 60 s. A failure of ssh itself (exit status 255) is
    /// classified with [`classify`]; any other non-zero exit becomes [`RemoteError::Failed`]
    /// with the script's stderr.
    pub fn exec(&self, script: &str) -> Result<String, RemoteError> {
        self.exec_bytes(script, EXEC_TIMEOUT)
            .map(|output| output.stdout_lossy())
    }

    pub(crate) fn exec_bytes(
        &self,
        script: &str,
        timeout: Duration,
    ) -> Result<Output, RemoteError> {
        let output = process::run(self.script_command(script)?, None, timeout)?;
        script_result(output)
    }

    /// `ssh … -- sh -c <script>`: runs a POSIX script on the host whatever the login shell.
    /// Fails when the connection is turned off.
    pub(crate) fn script_command(&self, script: &str) -> Result<Command, RemoteError> {
        self.check_open()?;
        let mut command = self.ssh_command();
        command
            .arg("-T")
            .args(self.base_args())
            .arg("--")
            .arg(format!("sh -c {}", remote_word(script)));
        Ok(command)
    }
}

/// Turns a finished `ssh … sh -c` into its stdout, a classified ssh failure (exit 255), or
/// [`RemoteError::Failed`] with the script's stderr.
pub(crate) fn script_result(output: Output) -> Result<Output, RemoteError> {
    {
        if output.status.success() {
            Ok(output)
        } else if output.status.code() == Some(255) {
            Err(ssh_failure(&output))
        } else {
            let stderr = output.stderr_lossy();
            let lines = meaningful_lines(&stderr);
            Err(RemoteError::Failed(if lines.is_empty() {
                format!("remote command failed ({})", output.status)
            } else {
                lines.join("\n")
            }))
        }
    }
}

impl Ssh {
    /// The program and arguments that run `argv` on the host in `cwd` with piped stdio and no
    /// TTY, for a long-running process such as `pi --mode rpc`.
    ///
    /// The remote command is
    /// `exec "${SHELL:-/bin/sh}" -lc '<launch script>' pistachio <cwd> <argv…>`: the login shell
    /// provides the user's full `PATH`, and the working folder and arguments travel as
    /// positional parameters, so they are quoted exactly once and the login shell never
    /// re-parses them. `cwd` and a program may start with `~/`; an absolute program's folder is
    /// prepended to `PATH`. Stopping: close the process's stdin (pi exits on EOF) or kill the
    /// local ssh, which closes the channel.
    pub fn command(&self, cwd: Option<&str>, argv: &[String]) -> (PathBuf, Vec<OsString>) {
        self.command_with_env(cwd, &[], argv)
    }

    /// [`Ssh::command`] with environment variables set for the program (after the login
    /// shell's own, so they win).
    pub fn command_with_env(
        &self,
        cwd: Option<&str>,
        env: &[(String, String)],
        argv: &[String],
    ) -> (PathBuf, Vec<OsString>) {
        let mut args = vec![OsString::from("-T")];
        if let Err(error) = self.ensure_control_dir() {
            log::warn!("{error}");
            args.extend(["-o", "ControlMaster=no", "-o", "ControlPath=none"].map(OsString::from));
        }
        args.extend(self.unshared_args());
        args.extend(self.base_args());
        args.push("--".into());
        args.push(remote_launch_command(cwd, env, argv).into());
        (Self::program(), args)
    }

    /// One round trip that reports the remote home, where `pi_command` resolves under the
    /// user's shells, its `--version`, and `uname -sm`.
    pub fn probe(&self, pi_command: &str) -> Result<Probe, RemoteError> {
        let script = format!("set -- {}\n{}", sh_quote(pi_command), probe_script());
        let output = self.exec_bytes(&script, PROBE_TIMEOUT)?;
        parse_probe(&output.stdout_lossy())
    }

    /// Lists the subfolders of `path` (absolute; `""` or `"~"` is the home folder, `~/…` is
    /// relative to it), dot folders too with `show_hidden`.
    pub fn list_dirs(&self, path: &str, show_hidden: bool) -> Result<DirListing, RemoteError> {
        let script = format!(
            "set -- {} {}\n{LIST_SCRIPT}",
            sh_quote(path),
            if show_hidden { "1" } else { "0" }
        );
        let output = self.exec_bytes(&script, LIST_TIMEOUT)?;
        parse_listing(&output.stdout)
    }

    /// Creates folder `name` inside `parent` (`name` may also be absolute or start with `~/`)
    /// and returns its canonical path.
    pub fn make_dir(&self, parent: &str, name: &str) -> Result<String, RemoteError> {
        let script = format!(
            "set -- {} {}\n{MKDIR_SCRIPT}",
            sh_quote(parent),
            sh_quote(name)
        );
        let path = self.exec(&script)?.trim().to_owned();
        if path.starts_with('/') {
            Ok(path)
        } else {
            Err(RemoteError::Failed(format!("could not create {name}")))
        }
    }

    /// The SHA256 fingerprints of the host's key, one per line (e.g.
    /// `256 SHA256:… host (ED25519)`), for asking the user whether to trust it.
    ///
    /// ssh itself connects (through any `ProxyJump`/`ProxyCommand`, without authenticating)
    /// with a private, empty `known_hosts` that it records the key in. That exact key is what
    /// [`Self::connect`] then trusts, so the fingerprint shown is the key accepted.
    pub fn host_key_fingerprint(&self) -> Result<String, RemoteError> {
        self.ensure_control_dir()?;
        let file = self.recorded_key_file();
        let _ = fs::remove_file(&file);
        let mut known = OsString::from("UserKnownHostsFile=");
        known.push(&file);
        let mut command = platform::command(Self::program());
        command
            .arg("-T")
            .args(["-o", "ControlMaster=no", "-o", "ControlPath=none"])
            .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10"])
            .args([
                "-o",
                "StrictHostKeyChecking=accept-new",
                "-o",
                "HashKnownHosts=no",
            ])
            .arg("-o")
            .arg(known)
            .args([
                "-o",
                "GlobalKnownHostsFile=/dev/null",
                "-o",
                "UpdateHostKeys=no",
            ])
            .args(["-o", "PreferredAuthentications=none"])
            .args([
                "-o",
                "PubkeyAuthentication=no",
                "-o",
                "PasswordAuthentication=no",
            ])
            .args(["-o", "KbdInteractiveAuthentication=no"])
            .args([
                "-o",
                "HostbasedAuthentication=no",
                "-o",
                "GSSAPIAuthentication=no",
            ])
            .args(Self::target_args(&self.target))
            .args(["--", "true"]);
        // Authentication is meant to fail; the key is recorded before it starts.
        let output = process::run(command, None, KEYSCAN_TIMEOUT)?;
        if fs::metadata(&file).map_or(true, |meta| meta.len() == 0) {
            let stderr = output.stderr_lossy();
            return Err(match classify(&stderr) {
                RemoteError::Failed(_) | RemoteError::AuthFailed(_) => RemoteError::Unreachable(
                    format!("could not fetch the host key of {}", self.target.display()),
                ),
                other => other,
            });
        }
        let mut keygen = platform::command("ssh-keygen");
        keygen.arg("-l").arg("-f").arg(&file);
        let prints = process::run(keygen, None, KEYSCAN_TIMEOUT)?;
        if !prints.status.success() {
            return Err(RemoteError::Failed(prints.stderr_lossy().trim().to_owned()));
        }
        Ok(prints.stdout_lossy().trim().to_owned())
    }

    /// `ssh` for this host. If the control directory is not private, sharing is switched off
    /// (ssh keeps the first value of an option) so no socket in it is ever used.
    pub(crate) fn ssh_command(&self) -> Command {
        let mut command = platform::command(Self::program());
        if let Err(error) = self.ensure_control_dir() {
            log::warn!("{error}");
            command.args(["-o", "ControlMaster=no", "-o", "ControlPath=none"]);
        }
        command
    }

    /// Creates the control directory (mode 0700) if needed, and refuses one that is not
    /// private (e.g. created in the shared `/tmp` by another user): its sockets would let that
    /// user see or take over the connection.
    pub(crate) fn ensure_control_dir(&self) -> Result<(), RemoteError> {
        prepare_private_dir(&self.control_dir).map_err(|error| {
            RemoteError::Failed(format!(
                "the connection folder {} is not safe to use: {error}",
                self.control_dir.display()
            ))
        })
    }
}

/// Creates `dir` with mode 0700, or checks that the existing one is a real directory owned by
/// the current user (it may live in the shared `/tmp`) and tightens its mode.
#[cfg(unix)]
pub(crate) fn prepare_private_dir(dir: &Path) -> io::Result<()> {
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent)?;
    }
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let meta = fs::symlink_metadata(dir)?;
    if !meta.is_dir() {
        return Err(io::Error::other("exists but is not a directory"));
    }
    if current_uid().is_some_and(|uid| uid != meta.uid()) {
        return Err(io::Error::other("owned by another user"));
    }
    if meta.mode() & 0o077 != 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Windows: the folder lives in the user's profile, which other users cannot write to.
#[cfg(not(unix))]
pub(crate) fn prepare_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

/// The current user's uid, taken from the owner of their home directory.
#[cfg(unix)]
fn current_uid() -> Option<u32> {
    dirs::home_dir()
        .and_then(|home| fs::metadata(home).ok())
        .map(|meta| meta.uid())
}

#[cfg(not(unix))]
fn current_uid() -> Option<u32> {
    None
}

fn user_tag() -> String {
    current_uid().map_or_else(
        || {
            std::env::var("USER")
                .or_else(|_| std::env::var("USERNAME"))
                .unwrap_or_else(|_| "user".to_owned())
        },
        |uid| uid.to_string(),
    )
}

/// `%` starts a token in `ControlPath`; a literal one is written `%%`.
fn escape_percent(path: &Path) -> OsString {
    let text = path.as_os_str().to_string_lossy();
    if text.contains('%') {
        text.replace('%', "%%").into()
    } else {
        path.as_os_str().to_owned()
    }
}

fn ssh_failure(output: &Output) -> RemoteError {
    classify(&output.stderr_lossy())
}

/// [`PROBE_SCRIPT`] with its placeholders filled in.
fn probe_script() -> String {
    PROBE_SCRIPT
        .replace("ENV_SCRIPT", LOGIN_PATH)
        .replace("LEGACY_MANAGED_DIR", LEGACY_MANAGED_DIR)
        .replace("MANAGED_DIR", MANAGED_DIR)
}

/// Run by `/bin/sh` on the host to start [`LAUNCH_SCRIPT`] in the user's login environment:
/// through their login shell, or for fish (which cannot run POSIX scripts) under `/bin/sh`
/// with fish's login `PATH`.
fn outer_launch_script() -> String {
    format!(
        r#"L={}
s=${{SHELL:-/bin/sh}}
case ${{s##*/}} in
  fish)
    lp=$("$s" -lc 'string join : $PATH' </dev/null 2>/dev/null | tail -n 1)
    case $lp in /*) PATH=$lp; export PATH ;; esac
    exec /bin/sh -c "$L" pistachio "$@" ;;
  *) exec "$s" -lc "$L" pistachio "$@" ;;
esac"#,
        sh_quote(LAUNCH_SCRIPT)
    )
}

/// The command string ssh hands to the remote user's shell for [`Ssh::command`].
fn remote_launch_command(cwd: Option<&str>, env: &[(String, String)], argv: &[String]) -> String {
    let mut remote = String::from("exec /bin/sh -c ");
    remote.push_str(&remote_word(&outer_launch_script()));
    remote.push_str(" pistachio ");
    remote.push_str(&remote_word(cwd.unwrap_or("")));
    for (name, value) in env {
        remote.push(' ');
        remote.push_str(&remote_word(&format!("PISTACHIO_ENV:{name}={value}")));
    }
    for arg in argv {
        remote.push(' ');
        remote.push_str(&remote_word(arg));
    }
    remote
}

/// Quotes `s` as one word for the remote user's login shell, read the same by POSIX shells
/// and fish: like [`sh_quote`], but a backslash is never left inside single quotes (fish
/// would treat `\\` there as an escape), so it is written as `'\\'` outside them.
pub fn remote_word(s: &str) -> String {
    if !s.contains('\\') {
        return sh_quote(s);
    }
    let mut quoted = String::with_capacity(s.len() + 8);
    quoted.push('\'');
    for c in s.chars() {
        match c {
            '\'' => quoted.push_str(r"'\''"),
            '\\' => quoted.push_str(r"'\\'"),
            c => quoted.push(c),
        }
    }
    quoted.push('\'');
    quoted
}

/// Quotes `s` as one word for a POSIX shell: words made only of safe characters stay as they
/// are, everything else is wrapped in single quotes (`'` becomes `'\''`).
pub fn sh_quote(s: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "@%+:,./_-".contains(c);
    if !s.is_empty() && s.chars().all(safe) {
        return s.to_owned();
    }
    let mut quoted = String::with_capacity(s.len() + 2);
    quoted.push('\'');
    for c in s.chars() {
        if c == '\'' {
            quoted.push_str(r"'\''");
        } else {
            quoted.push(c);
        }
    }
    quoted.push('\'');
    quoted
}

/// Parses the marker lines printed by [`PROBE_SCRIPT`].
fn parse_probe(stdout: &str) -> Result<Probe, RemoteError> {
    let mut home = None;
    let mut system = None;
    let mut pi_path = None;
    let mut pi_version = None;
    let mut agent_dir = None;
    let mut sessions_dir = None;
    for line in stdout.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = Some(value.trim().to_owned()).filter(|value| !value.is_empty());
        match key {
            "PISTACHIO_HOME" => home = value,
            "PISTACHIO_SYSTEM" => system = value,
            "PISTACHIO_PI" => pi_path = value,
            "PISTACHIO_VERSION" => pi_version = value,
            "PISTACHIO_AGENT" => agent_dir = value,
            "PISTACHIO_SESSIONS" => sessions_dir = value,
            _ => {}
        }
    }
    let home = home.ok_or_else(|| {
        RemoteError::Failed(format!("unexpected reply from the host: {}", stdout.trim()))
    })?;
    let agent_dir = agent_dir.unwrap_or_else(|| format!("{home}/.pi/agent"));
    let sessions_dir = sessions_dir.unwrap_or_else(|| format!("{agent_dir}/sessions"));
    Ok(Probe {
        home,
        agent_dir,
        sessions_dir,
        pi_path,
        pi_version,
        system: system.unwrap_or_default(),
    })
}

/// Parses the NUL-separated output of [`LIST_SCRIPT`].
fn parse_listing(stdout: &[u8]) -> Result<DirListing, RemoteError> {
    let mut fields = stdout
        .split(|&byte| byte == 0)
        .map(|field| String::from_utf8_lossy(field).into_owned());
    let path = fields
        .next()
        .filter(|path| path.starts_with('/'))
        .ok_or_else(|| {
            RemoteError::Failed(format!(
                "unexpected folder listing: {}",
                String::from_utf8_lossy(stdout).trim()
            ))
        })?;
    let mut dirs: Vec<String> = fields.filter(|name| !name.is_empty()).collect();
    dirs.sort_by(|a, b| {
        a.to_lowercase()
            .cmp(&b.to_lowercase())
            .then_with(|| a.cmp(b))
    });
    let parent = parent_of(&path);
    Ok(DirListing { path, parent, dirs })
}

fn parent_of(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rfind('/') {
        Some(0) => Some("/".to_owned()),
        Some(index) => Some(trimmed[..index].to_owned()),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[cfg(unix)]
    fn os(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn quoting() {
        assert_eq!(sh_quote("pi"), "pi");
        assert_eq!(sh_quote("/usr/local/bin/pi"), "/usr/local/bin/pi");
        assert_eq!(sh_quote("--mode"), "--mode");
        assert_eq!(sh_quote(""), "''");
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_quote("$HOME"), "'$HOME'");
        assert_eq!(sh_quote("~/x"), "'~/x'");
        assert_eq!(sh_quote("a=b"), "'a=b'");
        assert_eq!(sh_quote("`id`;\n"), "'`id`;\n'");
    }

    /// The words ssh hands to the login shell read the same in sh and (when installed) fish.
    #[test]
    fn remote_words_survive_sh_and_fish() {
        let words = [
            "",
            "a b",
            "it's",
            "$HOME",
            "\"q\"",
            "\\n",
            "a\\b",
            "back\\\\slash'q",
            "a\nb",
            "*",
        ];
        let fish = Command::new("fish").arg("--version").output().is_ok();
        for word in words {
            let line = format!("printf %s {}", remote_word(word));
            let out = Command::new("/bin/sh")
                .args(["-c", &line])
                .output()
                .unwrap();
            assert_eq!(String::from_utf8(out.stdout).unwrap(), word, "sh: {line}");
            if fish {
                let out = Command::new("fish").args(["-c", &line]).output().unwrap();
                assert_eq!(String::from_utf8(out.stdout).unwrap(), word, "fish: {line}");
            }
        }
    }

    #[test]
    fn quoting_survives_a_shell() {
        for word in [
            "", "a b", "it's", "$HOME", "\"q\"", "\\n", "a\nb", "ünï", "*",
        ] {
            let out = Command::new("/bin/sh")
                .args(["-c", &format!("printf %s {}", sh_quote(word))])
                .output()
                .unwrap();
            assert_eq!(String::from_utf8(out.stdout).unwrap(), word);
        }
    }

    #[cfg(unix)] // Unix shells, file modes or connection sharing
    #[test]
    fn base_args_for_alias_and_custom() {
        let alias = Ssh::with_control_dir(
            Target::Alias {
                alias: "spark".into(),
            },
            "/tmp/cm".into(),
        );
        let common = [
            "-o",
            "ControlMaster=auto",
            "-o",
            "ControlPath=/tmp/cm/%C",
            "-o",
            "ControlPersist=600",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
            "LogLevel=ERROR",
        ];
        let mut expected = os(&common);
        expected.push("spark".into());
        assert_eq!(alias.base_args(), expected);

        let custom = Ssh::with_control_dir(
            Target::Custom {
                host: "box.example.com".into(),
                user: Some("me".into()),
                port: Some(2222),
                identity_file: Some("/k/id key".into()),
                proxy_jump: Some("bastion".into()),
            },
            "/tmp/c%m".into(),
        );
        let mut expected = os(&common);
        expected[3] = "ControlPath=/tmp/c%%m/%C".into();
        expected.extend(os(&[
            "-p",
            "2222",
            "-i",
            "/k/id key",
            "-o",
            "IdentitiesOnly=yes",
            "-J",
            "bastion",
            "-l",
            "me",
            "box.example.com",
        ]));
        assert_eq!(custom.base_args(), expected);
    }

    #[cfg(unix)] // Unix shells, file modes or connection sharing
    #[test]
    fn a_closed_connection_runs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = Ssh::with_control_dir(
            Target::Alias {
                alias: "spark".into(),
            },
            dir.path().join("cm"),
        );
        assert_eq!(ssh.unshared_args(), os(&[]));
        // Clones are the same connection.
        ssh.clone().close();
        assert!(ssh.is_closed());
        let off = "The connection is turned off.";
        assert_eq!(ssh.exec("true").unwrap_err().to_string(), off);
        assert_eq!(ssh.connect(false).unwrap_err().to_string(), off);
        assert_eq!(
            ssh.connect_interactive(&[], false).unwrap_err().to_string(),
            off
        );
        let shell: &dyn workspace::RemoteShell = &ssh;
        assert_eq!(shell.run("true", None, None).unwrap_err().to_string(), off);
        // What the user starts anyway has a connection of its own.
        let (_, args) = ssh.command(None, &strings(&["pi"]));
        assert_eq!(
            &args[1..5],
            os(&["-o", "ControlMaster=no", "-o", "ControlPath=none"]).as_slice()
        );
        ssh.reopen();
        assert!(!ssh.is_closed());
        assert!(ssh.script_command("true").is_ok());
        assert_eq!(ssh.unshared_args(), os(&[]));
    }

    #[cfg(unix)] // Unix shells, file modes or connection sharing
    #[test]
    fn command_args() {
        let dir = tempfile::tempdir().unwrap();
        let ssh = Ssh::with_control_dir(
            Target::Alias {
                alias: "spark".into(),
            },
            dir.path().join("cm"),
        );
        let (program, args) = ssh.command(
            Some("/home/me/my project"),
            &strings(&["pi", "--mode", "rpc"]),
        );
        assert_eq!(program, PathBuf::from("ssh"));
        assert_eq!(args[0], "-T");
        assert_eq!(&args[1..args.len() - 2], ssh.base_args().as_slice());
        assert_eq!(args[args.len() - 2], "--");
        assert_eq!(
            args[args.len() - 1],
            OsString::from(format!(
                "exec /bin/sh -c {} pistachio '/home/me/my project' pi --mode rpc",
                remote_word(&outer_launch_script())
            ))
        );
        // The control directory is created privately on demand.
        let meta = fs::metadata(dir.path().join("cm")).unwrap();
        assert_eq!(meta.mode() & 0o777, 0o700);
    }

    /// Runs the remote command string the way sshd would (the user's shell, `-c`), locally
    /// with `SHELL=/bin/sh` and a scratch `HOME`.
    fn run_remote(home: &Path, cwd: Option<&str>, argv: &[String]) -> (String, String, bool) {
        run_remote_with_env(home, cwd, &[], argv)
    }

    fn run_remote_with_env(
        home: &Path,
        cwd: Option<&str>,
        env: &[(String, String)],
        argv: &[String],
    ) -> (String, String, bool) {
        let remote = remote_launch_command(cwd, env, argv);
        let out = Command::new("/bin/sh")
            .args(["-c", &remote])
            .env("SHELL", "/bin/sh")
            .env("HOME", home)
            .env_remove("ENV")
            .output()
            .unwrap();
        (
            String::from_utf8(out.stdout).unwrap(),
            String::from_utf8(out.stderr).unwrap(),
            out.status.success(),
        )
    }

    #[test]
    fn command_quoting_survives_both_shells() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        let cwd = home.join("it's a \"dir\" $HOME `x`");
        fs::create_dir(&cwd).unwrap();
        let argv = strings(&[
            "/bin/sh",
            "-c",
            r#"pwd -P; for a; do printf '[%s]\n' "$a"; done"#,
            "argv0",
            "a b",
            "it's",
            "$HOME",
            "",
            "`id`; rm -rf /nope",
            "*",
        ]);
        let (stdout, stderr, ok) = run_remote(&home, cwd.to_str(), &argv);
        assert!(ok, "{stderr}");
        assert_eq!(
            stdout,
            format!(
                "{}\n[a b]\n[it's]\n[$HOME]\n[]\n[`id`; rm -rf /nope]\n[*]\n",
                cwd.display()
            )
        );
    }

    #[test]
    fn command_sets_environment_variables() {
        let home = tempfile::tempdir().unwrap();
        let env = [
            ("PI_ONE".to_owned(), "it's $HOME `x`".to_owned()),
            ("PI_TWO".to_owned(), "a=b".to_owned()),
        ];
        let argv = strings(&[
            "/bin/sh",
            "-c",
            r#"printf '[%s]\n' "$PI_ONE" "$PI_TWO" "$1""#,
            "-",
            "PISTACHIO_ENV:X=y",
        ]);
        let (stdout, stderr, ok) = run_remote_with_env(home.path(), None, &env, &argv);
        assert!(ok, "{stderr}");
        // Only words before the program are taken as variables.
        assert_eq!(stdout, "[it's $HOME `x`]\n[a=b]\n[PISTACHIO_ENV:X=y]\n");
    }

    #[cfg(unix)]
    #[test]
    fn command_checks_who_answers_on_the_relay_port() {
        use crate::link::PROOF_FILE;

        let home = tempfile::tempdir().unwrap();
        let agent = home.path().join("agent dir");
        fs::create_dir(&agent).unwrap();
        // Answers every request with `answer`, or closes the connection.
        let serve = |answer: &'static str| {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            thread::spawn(move || {
                for mut stream in listener.incoming().flatten() {
                    let mut request = String::new();
                    let mut lines = io::BufReader::new(stream.try_clone().unwrap());
                    while io::BufRead::read_line(&mut lines, &mut request).unwrap_or(0) > 2 {}
                    if !answer.is_empty()
                        && request.starts_with("GET /_pistachio/prove/c0ffee HTTP/1.")
                    {
                        let _ = io::Write::write_all(
                            &mut stream,
                            format!("HTTP/1.0 200 OK\r\nconnection: close\r\n\r\n{answer}")
                                .as_bytes(),
                        );
                    }
                }
            });
            port
        };
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let launch = |port: u16, path: Option<&Path>| {
            fs::write(agent.join(PROOF_FILE), format!("{port} c0ffee a1b2c3\n")).unwrap();
            let mut env = vec![
                (RELAY_PORT_ENV.to_owned(), port.to_string()),
                (
                    "PI_CODING_AGENT_DIR".to_owned(),
                    agent.to_str().unwrap().to_owned(),
                ),
            ];
            if let Some(path) = path {
                env.push(("PATH".to_owned(), path.to_str().unwrap().to_owned()));
            }
            run_remote_with_env(home.path(), None, &env, &strings(&["/bin/echo", "started"]))
        };
        // A host with bash and no curl.
        let only_bash = home.path().join("only-bash");
        fs::create_dir(&only_bash).unwrap();
        std::os::unix::fs::symlink("/bin/bash", only_bash.join("bash")).unwrap();

        let (relay, other, silent) = (serve("a1b2c3"), serve("a1b2c4"), serve(""));
        for path in [None, Some(only_bash.as_path())] {
            let (stdout, stderr, ok) = launch(relay, path);
            assert!(ok, "{path:?}: {stderr}");
            assert_eq!(stdout, "started\n");
            // Someone else listens there, or nobody (the connection was made again without it).
            for port in [other, silent, closed] {
                let (stdout, stderr, ok) = launch(port, path);
                assert!(!ok, "{path:?}");
                assert_eq!(stdout, "");
                assert!(stderr.contains("Reconnect"), "{stderr}");
            }
        }
        // Nothing to check it with.
        let env = [(RELAY_PORT_ENV.to_owned(), relay.to_string())];
        let argv = strings(&["/bin/echo", "started"]);
        assert!(!run_remote_with_env(home.path(), None, &env, &argv).2);
    }

    #[cfg(unix)] // Unix shells, file modes or connection sharing
    #[test]
    fn command_expands_tilde_and_prepends_program_dir() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        fs::create_dir_all(home.join("proj")).unwrap();
        fs::create_dir_all(home.join("bin")).unwrap();
        let tool = home.join("bin/tool");
        fs::write(
            &tool,
            "#!/bin/sh\npwd -P\nprintf '%s\\n' \"$PATH\" \"$@\"\n",
        )
        .unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();

        let (stdout, stderr, ok) =
            run_remote(&home, Some("~/proj"), &strings(&["~/bin/tool", "x y"]));
        assert!(ok, "{stderr}");
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines[0], home.join("proj").to_str().unwrap());
        assert!(
            lines[1].starts_with(&format!("{}:", home.join("bin").display())),
            "{}",
            lines[1]
        );
        assert_eq!(lines[2], "x y");

        // No working folder: stays in $HOME (where sshd starts).
        let (_, stderr, ok) = run_remote(&home, None, &strings(&["true"]));
        assert!(ok, "{stderr}");
        // A missing folder fails before running anything.
        let (stdout, _, ok) = run_remote(&home, Some("/no/such/dir"), &strings(&["echo", "ran"]));
        assert!(!ok);
        assert_eq!(stdout, "");
    }

    #[test]
    fn list_script_runs_locally() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        for dir in ["b", "A", "c d", ".hidden", "it's"] {
            fs::create_dir(home.join(dir)).unwrap();
        }
        fs::write(home.join("file"), "").unwrap();
        let script = format!("set -- {}\n{LIST_SCRIPT}", sh_quote("~"));
        let out = Command::new("/bin/sh")
            .args(["-c", &script])
            .env("HOME", &home)
            .output()
            .unwrap();
        assert!(out.status.success());
        let listing = parse_listing(&out.stdout).unwrap();
        assert_eq!(listing.path, home.to_str().unwrap());
        assert_eq!(listing.dirs, strings(&["A", "b", "c d", "it's"]));

        let script = format!("set -- {} 1\n{LIST_SCRIPT}", sh_quote("~"));
        let out = Command::new("/bin/sh")
            .args(["-c", &script])
            .env("HOME", &home)
            .output()
            .unwrap();
        let listing = parse_listing(&out.stdout).unwrap();
        assert_eq!(listing.dirs, strings(&[".hidden", "A", "b", "c d", "it's"]));

        let script = format!(
            "set -- {} {}\n{MKDIR_SCRIPT}",
            sh_quote("~"),
            sh_quote("new dir/x")
        );
        let out = Command::new("/bin/sh")
            .args(["-c", &script])
            .env("HOME", &home)
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8(out.stdout).unwrap().trim(),
            home.join("new dir/x").to_str().unwrap()
        );

        let script = format!("set -- {}\n{LIST_SCRIPT}", sh_quote("/no/such"));
        let out = Command::new("/bin/sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&out.stderr),
            "No such folder: /no/such\n"
        );
    }

    #[cfg(unix)] // Unix shells, file modes or connection sharing
    #[test]
    fn probe_script_runs_locally() {
        let home = tempfile::tempdir().unwrap();
        let home = home.path().canonicalize().unwrap();
        fs::create_dir_all(home.join("bin")).unwrap();
        let tool = home.join("bin/fakepi");
        fs::write(&tool, "#!/bin/sh\necho 'fakepi 1.2.3'\n").unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();

        let run = |command: &str| {
            let script = format!("set -- {}\n{}", sh_quote(command), probe_script());
            let out = Command::new("/bin/sh")
                .args(["-c", &script])
                .env("HOME", &home)
                .env("SHELL", "/bin/sh")
                .env("PATH", "/usr/bin:/bin")
                .env_remove("ENV")
                .env_remove("PI_CODING_AGENT_DIR")
                .env_remove("PI_CODING_AGENT_SESSION_DIR")
                .output()
                .unwrap();
            assert!(out.status.success());
            parse_probe(&String::from_utf8(out.stdout).unwrap()).unwrap()
        };
        let probe = run("~/bin/fakepi");
        assert_eq!(probe.home, home.to_str().unwrap());
        assert_eq!(probe.pi_path.as_deref(), tool.to_str());
        assert_eq!(probe.pi_version.as_deref(), Some("fakepi 1.2.3"));
        assert_ne!(probe.system, "");

        let missing = run("definitely-not-a-command-pistachio");
        assert_eq!(missing.pi_path, None);
        assert_eq!(missing.pi_version, None);
        assert_eq!(missing.agent_dir, format!("{}/.pi/agent", home.display()));
        assert_eq!(
            missing.sessions_dir,
            format!("{}/.pi/agent/sessions", home.display())
        );

        // pistachio's own install is found when `pi` is not on PATH; settings move sessions.
        let managed = home.join(MANAGED_DIR).join("bin");
        fs::create_dir_all(&managed).unwrap();
        fs::copy(&tool, managed.join("pi")).unwrap();
        fs::create_dir_all(home.join(".pi/agent")).unwrap();
        fs::write(
            home.join(".pi/agent/settings.json"),
            "{\n  \"sessionDir\": \"/data/pi sessions\"\n}\n",
        )
        .unwrap();
        let found = run("pi");
        assert_eq!(found.pi_path.as_deref(), managed.join("pi").to_str());
        assert_eq!(found.sessions_dir, "/data/pi sessions");
    }

    #[test]
    fn parses_probe_output() {
        let stdout = "Welcome to Ubuntu!\nPISTACHIO_HOME=/home/me\nPISTACHIO_SYSTEM=Linux aarch64\n\
                      PISTACHIO_PI=/home/me/.nvm/versions/node/v22/bin/pi\nPISTACHIO_VERSION=0.45.3\n\
                      PISTACHIO_AGENT=/home/me/.pi/agent\nPISTACHIO_SESSIONS=/data/sessions\n";
        assert_eq!(
            parse_probe(stdout).unwrap(),
            Probe {
                home: "/home/me".into(),
                agent_dir: "/home/me/.pi/agent".into(),
                sessions_dir: "/data/sessions".into(),
                pi_path: Some("/home/me/.nvm/versions/node/v22/bin/pi".into()),
                pi_version: Some("0.45.3".into()),
                system: "Linux aarch64".into(),
            }
        );
        let no_pi =
            parse_probe("PISTACHIO_HOME=/Users/me\nPISTACHIO_SYSTEM=Darwin arm64\n").unwrap();
        assert_eq!(no_pi.pi_path, None);
        assert_eq!(no_pi.sessions_dir, "/Users/me/.pi/agent/sessions");
        assert_eq!(no_pi.pi_version, None);
        let empty_version =
            parse_probe("PISTACHIO_HOME=/h\nPISTACHIO_PI=/h/pi\nPISTACHIO_VERSION=\n").unwrap();
        assert_eq!(empty_version.pi_version, None);
        assert!(matches!(
            parse_probe("garbage"),
            Err(RemoteError::Failed(_))
        ));
    }

    #[test]
    fn parses_listing_output() {
        let listing = parse_listing(b"/home/me\0zeta\0Alpha\0beta\0alpha\0").unwrap();
        assert_eq!(
            listing,
            DirListing {
                path: "/home/me".into(),
                parent: Some("/home".into()),
                dirs: strings(&["Alpha", "alpha", "beta", "zeta"]),
            }
        );
        let root = parse_listing(b"/\0usr\0").unwrap();
        assert_eq!(root.parent, None);
        assert_eq!(parse_listing(b"/home\0").unwrap().parent, Some("/".into()));
        assert!(parse_listing(b"").is_err());
        assert!(parse_listing(b"relative\0").is_err());
    }

    #[test]
    fn default_control_dir_fits_a_socket_path() {
        let dir = Ssh::default_control_dir();
        assert!(dir.is_absolute());
        assert!(
            dir.as_os_str().len() <= MAX_CONTROL_DIR_LEN,
            "{}",
            dir.display()
        );
    }

    #[cfg(unix)] // Unix shells, file modes or connection sharing
    #[test]
    fn private_dir_is_created_and_tightened() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("a/cm");
        prepare_private_dir(&dir).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        prepare_private_dir(&dir).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);

        let file = root.path().join("file");
        fs::write(&file, "").unwrap();
        assert!(prepare_private_dir(&file).is_err());
    }
}
