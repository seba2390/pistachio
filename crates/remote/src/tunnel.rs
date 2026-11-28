//! A host's connection where OpenSSH cannot share one (Windows): a background `ssh` that stays
//! for as long as the host is connected and holds the port forward to the app's model relay.
//! Commands open connections of their own.
//!
//! It runs `cat` on the host, which ends when its input does. So the tunnel ends with the app,
//! even one that crashes: the system then closes the other end of that pipe.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::link::{Endpoint, FORWARD_REFUSED, forward_spec};
use crate::ssh::Ssh;
use crate::{RemoteError, classify};

const FORWARD_TIMEOUT: Duration = Duration::from_secs(45);

/// Printed once the connection stands; the forward was asked for before.
const READY: &str = "PISTACHIO_TUNNEL";

/// In `sh`, whatever the login shell is.
const REMOTE_COMMAND: &str = "sh -c 'echo PISTACHIO_TUNNEL; exec cat >/dev/null'";

#[derive(Debug)]
pub(crate) struct Tunnel {
    child: Child,
    /// The forward: the host's port and what it leads to here.
    forward: Option<(u16, Endpoint)>,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

enum Line {
    Stdout(String),
    Stderr(String),
}

impl Ssh {
    fn tunnel(&self) -> MutexGuard<'_, Option<Tunnel>> {
        self.tunnel_slot()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// The running tunnel's process id.
    pub(crate) fn tunnel_pid(&self) -> Option<u32> {
        let mut tunnel = self.tunnel();
        let child = &mut tunnel.as_mut()?.child;
        matches!(child.try_wait(), Ok(None)).then(|| child.id())
    }

    pub(crate) fn close_tunnel(&self) {
        let tunnel = self.tunnel().take();
        drop(tunnel);
    }

    /// Connects: opens a tunnel without a forward. `options` go before the usual ones.
    pub(crate) fn open_tunnel(
        &self,
        options: &[OsString],
        env: &[(OsString, OsString)],
        timeout: Duration,
    ) -> Result<(), RemoteError> {
        let tunnel = self.start_tunnel(options, env, None, timeout)?;
        *self.tunnel() = Some(tunnel);
        Ok(())
    }

    /// See [`Ssh::forward_to`].
    pub(crate) fn tunnel_to(&self, port: u16, endpoint: &Endpoint) -> Result<u16, RemoteError> {
        let _changing = self.start_connecting()?;
        {
            // The host can't listen on that port again while the old tunnel holds it.
            let mut tunnel = self.tunnel();
            let holds = |tunnel: &Tunnel| tunnel.forward.as_ref().is_some_and(|f| f.0 == port);
            if tunnel.as_ref().is_some_and(holds) {
                *tunnel = None;
            }
        }
        let tunnel = self.start_tunnel(&[], &[], Some((port, endpoint)), FORWARD_TIMEOUT)?;
        let port = tunnel.forward.as_ref().map_or(port, |forward| forward.0);
        *self.tunnel() = Some(tunnel);
        Ok(port)
    }

    /// See [`Ssh::close_forward`].
    pub(crate) fn close_tunnel_to(&self, endpoint: &Endpoint) {
        {
            let mut tunnel = self.tunnel();
            let leads = |tunnel: &Tunnel| tunnel.forward.as_ref().is_some_and(|f| f.1 == *endpoint);
            if !tunnel.as_ref().is_some_and(leads) {
                return;
            }
            *tunnel = None;
        }
        // Stays connected, without the forward.
        let Ok(_changing) = self.start_connecting() else {
            return;
        };
        if self.tunnel_pid().is_none()
            && let Err(error) = self.open_tunnel(&[], &[], FORWARD_TIMEOUT)
        {
            log::warn!("{}: {error}", self.target().display());
        }
    }

    fn start_tunnel(
        &self,
        options: &[OsString],
        env: &[(OsString, OsString)],
        forward: Option<(u16, &Endpoint)>,
        timeout: Duration,
    ) -> Result<Tunnel, RemoteError> {
        let mut command = Command::new(Ssh::program());
        command.arg("-T").args(options);
        if let Some((port, endpoint)) = forward {
            // ssh only says so whether the host accepted a forward.
            command.args(["-o", "LogLevel=DEBUG1", "-R"]);
            command.arg(forward_spec(port, endpoint));
        }
        command
            .args(self.base_args())
            .args(["--", REMOTE_COMMAND])
            .envs(env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        platform::NoWindow::no_window(&mut command);
        // Ended by `Drop` when this fails.
        let mut tunnel = Tunnel {
            child: command.spawn()?,
            forward: None,
        };
        let (lines, received) = mpsc::channel();
        read_lines(tunnel.child.stdout.take(), lines.clone(), Line::Stdout);
        read_lines(tunnel.child.stderr.take(), lines, Line::Stderr);

        let deadline = Instant::now() + timeout;
        let mut ready = false;
        let mut listening = None;
        let mut said = String::new();
        while !ready || (forward.is_some() && listening.is_none()) {
            let left = deadline.saturating_duration_since(Instant::now());
            match received.recv_timeout(left) {
                // After what a login shell may print.
                Ok(Line::Stdout(line)) => ready |= line.trim_end().ends_with(READY),
                Ok(Line::Stderr(line)) => {
                    if let Some((port, endpoint)) = forward {
                        match forwarded(&line, port, endpoint) {
                            Some(Forwarded::On(port)) => listening = Some(port),
                            Some(Forwarded::Refused) => {
                                return Err(RemoteError::Failed(FORWARD_REFUSED.to_owned()));
                            }
                            None => {}
                        }
                    }
                    if !line.starts_with("debug1:") {
                        said.push_str(&line);
                        said.push('\n');
                    }
                }
                // Both ends closed: ssh ended.
                Err(RecvTimeoutError::Disconnected) => return Err(classify(&said)),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(RemoteError::Unreachable(format!(
                        "no response after {} seconds",
                        timeout.as_secs()
                    )));
                }
            }
        }
        tunnel.forward = forward
            .zip(listening)
            .map(|((_, endpoint), port)| (port, endpoint.clone()));
        Ok(tunnel)
    }
}

/// What became of a forward the host was asked for.
#[derive(Debug, PartialEq, Eq)]
enum Forwarded {
    On(u16),
    Refused,
}

/// Whether a line of ssh's log (`LogLevel=DEBUG1`) says that the host listens on `port` for
/// `endpoint` (0: on which port it chose), or that it does not. Lines about forwards in the
/// user's own ssh configuration say nothing about this one.
fn forwarded(line: &str, port: u16, endpoint: &Endpoint) -> Option<Forwarded> {
    if line
        .split_once("remote port forwarding failed for listen port ")
        .is_some_and(|(_, refused)| refused.trim() == port.to_string())
    {
        return Some(Forwarded::Refused);
    }
    let target = match endpoint {
        Endpoint::Port(local) => format!("127.0.0.1:{local}"),
        Endpoint::Socket(path) => path.display().to_string(),
    };
    if port == 0 {
        let rest = line.strip_prefix("Allocated port ")?;
        let (chosen, rest) = rest.split_once(" for remote forward to ")?;
        return (rest == target)
            .then(|| chosen.parse().ok())?
            .map(Forwarded::On);
    }
    let rest = line.strip_prefix("debug1: remote forward success for: listen ")?;
    let (listen, rest) = rest.split_once(", connect ")?;
    let listens = listen == format!("127.0.0.1:{port}");
    (listens && rest.starts_with(&target)).then_some(Forwarded::On(port))
}

/// Sends the lines of a pipe until it ends; those nobody waits for any more are dropped, so ssh
/// never waits either.
fn read_lines(
    pipe: Option<impl Read + Send + 'static>,
    lines: Sender<Line>,
    kind: fn(String) -> Line,
) {
    let Some(pipe) = pipe else { return };
    thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut bytes = Vec::new();
        while reader.read_until(b'\n', &mut bytes).is_ok_and(|n| n > 0) {
            let line = String::from_utf8_lossy(&bytes);
            let _ = lines.send(kind(line.trim_end_matches(['\r', '\n']).to_owned()));
            bytes.clear();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_became_of_the_forward() {
        let endpoint = Endpoint::Port(5000);
        let read = |line: &str, port: u16| forwarded(line, port, &endpoint);
        let chosen = "Allocated port 40123 for remote forward to 127.0.0.1:5000";
        assert_eq!(read(chosen, 0), Some(Forwarded::On(40123)));
        let asked =
            "debug1: remote forward success for: listen 127.0.0.1:4321, connect 127.0.0.1:5000";
        assert_eq!(read(asked, 4321), Some(Forwarded::On(4321)));
        for port in [0, 4321] {
            let refused = format!("Warning: remote port forwarding failed for listen port {port}");
            assert_eq!(read(&refused, port), Some(Forwarded::Refused));
            let refused = format!("Error: remote port forwarding failed for listen port {port}");
            assert_eq!(read(&refused, port), Some(Forwarded::Refused));
        }
        // Forwards of the user's own, and other lines.
        for line in [
            "Warning: remote port forwarding failed for listen port 8080",
            "Allocated port 40124 for remote forward to localhost:22",
            "debug1: remote forward success for: listen 127.0.0.1:4322, connect 127.0.0.1:5000",
            "debug1: Remote connections from 127.0.0.1:4321 forwarded to local address 127.0.0.1:5000",
            "debug1: pledge: network",
            "",
        ] {
            assert_eq!(read(line, 0), None, "{line}");
            assert_eq!(read(line, 4321), None, "{line}");
        }
    }
}
