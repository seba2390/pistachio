//! The hidden pi that runs `relay.ts`: starting it, talking to it and stopping it.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use remote::{Endpoint, Ssh};
use serde_json::{Value, json};

use super::{
    CONTROL_KEY_ENV, KEY_ENV, Launch, PORT_FILE_ENV, SOCKET_ENV, hex, hmac_sha256,
    install_extension, random, tokens,
};

const START_TIMEOUT: Duration = Duration::from_secs(30);
const ANSWER_TIMEOUT: Duration = Duration::from_secs(20);

struct Running {
    child: Child,
    /// pi exits when its input closes, so the relay ends with the app even if it crashes.
    _stdin: Option<ChildStdin>,
    endpoint: Endpoint,
    /// Signs the app's requests to this pi, and its answers.
    control_key: [u8; 32],
    /// The connections that forward to its port.
    forwards: Vec<Ssh>,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
static KEY: OnceLock<[u8; 32]> = OnceLock::new();

fn running() -> MutexGuard<'static, Option<Running>> {
    RUNNING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Signs the hosts' tokens, for as long as the app runs.
pub fn key() -> &'static [u8; 32] {
    KEY.get_or_init(random)
}

/// Where the relay listens and this computer's models, starting the relay if needed.
pub fn models(launch: &Launch) -> Result<(Endpoint, Vec<Value>), String> {
    let mut running = running();
    if let Some(relay) = running.as_mut()
        && matches!(relay.child.try_wait(), Ok(None))
        && let Ok(models) = fetch_models(&relay.endpoint, &relay.control_key)
    {
        return Ok((relay.endpoint.clone(), models));
    }
    if let Some(old) = running.take() {
        end(old);
    }
    let (relay, models) = start(launch)?;
    let endpoint = relay.endpoint.clone();
    *running = Some(relay);
    Ok((endpoint, models))
}

/// Tells the relay which tokens to accept.
pub fn allow_tokens() -> Result<(), String> {
    let running = running();
    let relay = running.as_ref().ok_or("the model relay has stopped")?;
    send_tokens(&relay.endpoint, &relay.control_key)
}

fn send_tokens(endpoint: &Endpoint, control_key: &[u8; 32]) -> Result<(), String> {
    let body = json!({ "nonces": tokens::all() }).to_string();
    let path = "/_pistachio/tokens";
    request(endpoint, control_key, "POST", path, body.as_bytes()).map(drop)
}

/// Remembers that `ssh` forwards to the relay, to close that when the relay ends: another
/// program could take over its port.
pub fn forwarded(ssh: &Ssh, endpoint: &Endpoint) {
    if !matches!(endpoint, Endpoint::Port(_)) {
        return;
    }
    if let Some(relay) = running().as_mut()
        && relay.endpoint == *endpoint
    {
        relay
            .forwards
            .retain(|known| known.target() != ssh.target());
        relay.forwards.push(ssh.clone());
    } else {
        ssh.close_forward(endpoint);
    }
}

pub fn stop() {
    if let Some(mut relay) = running().take() {
        // The app closes its connections itself.
        relay.forwards.clear();
        end(relay);
    }
}

#[cfg(all(test, unix))]
pub fn kill_for_tests() {
    if let Some(relay) = running().as_mut() {
        let _ = relay.child.kill();
    }
}

fn end(mut relay: Running) {
    // On Windows pi may be a script: ending that leaves pi itself, which its input ends.
    drop(relay._stdin.take());
    let _ = relay.child.kill();
    let _ = relay.child.wait();
    if let Endpoint::Socket(socket) = &relay.endpoint {
        let _ = std::fs::remove_file(socket);
    }
    close_forwards(&relay.endpoint, &relay.forwards);
}

fn close_forwards(endpoint: &Endpoint, forwards: &[Ssh]) {
    for ssh in forwards {
        ssh.close_forward(endpoint);
    }
}

fn start(launch: &Launch) -> Result<(Running, Vec<Value>), String> {
    let fail = |error: std::io::Error| format!("could not start pi for the model relay: {error}");
    let extension = install_extension(&launch.dir).map_err(fail)?;
    let id = std::process::id();
    // In the private folder of the shared SSH connections.
    let socket = Ssh::default_control_dir().join(format!("relay-{id}.sock"));
    let port_file = launch.dir.join(format!("relay-{id}.port"));
    let (variable, place) = if launch.tcp {
        (PORT_FILE_ENV, &port_file)
    } else {
        (SOCKET_ENV, &socket)
    };
    let _ = std::fs::remove_file(place);
    let log_path = launch.dir.join("relay.log");
    let log = std::fs::File::create(&log_path).map_err(fail)?;
    let control_key = random();
    let mut command = ::platform::command(&launch.program);
    command
        .args([
            "--mode",
            "rpc",
            "--no-session",
            "--no-extensions",
            "--no-skills",
            "--no-prompt-templates",
            "--no-context-files",
            "--no-tools",
            "--extension",
        ])
        .arg(&extension)
        .current_dir(&launch.dir)
        .envs(launch.env.vars().iter().map(|(name, value)| (name, value)))
        .env(variable, place)
        .env(KEY_ENV, hex(key()))
        .env(CONTROL_KEY_ENV, hex(&control_key))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(log);
    remote::no_console(&mut command);
    let mut child = command.spawn().map_err(fail)?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let started = Instant::now();
    loop {
        let endpoint = if launch.tcp {
            read_port(&port_file).map(Endpoint::Port)
        } else {
            Some(Endpoint::Socket(socket.clone()))
        };
        if let Some(endpoint) = endpoint
            && let Ok(models) = fetch_models(&endpoint, &control_key)
            // One that was started again: hosts still use the tokens they have.
            && send_tokens(&endpoint, &control_key).is_ok()
        {
            log::info!(
                "model relay started (pid {}) in {:?}",
                child.id(),
                started.elapsed()
            );
            let _ = std::fs::remove_file(&port_file);
            watch(stdout, endpoint.clone());
            let relay = Running {
                child,
                _stdin: stdin,
                endpoint,
                control_key,
                forwards: Vec::new(),
            };
            return Ok((relay, models));
        }
        let exited = child.try_wait().ok().flatten();
        if exited.is_some() || started.elapsed() > START_TIMEOUT {
            drop(stdin);
            let _ = child.kill();
            let _ = child.wait();
            let log = std::fs::read_to_string(&log_path).unwrap_or_default();
            let tail: Vec<&str> = log.lines().rev().take(5).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            return Err(format!(
                "pi for the model relay did not start{}",
                if tail.is_empty() {
                    String::new()
                } else {
                    format!(": {}", tail.join("\n"))
                }
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Reads what pi prints (nothing of use) until it ends, then closes the forwards to its port.
fn watch(stdout: Option<impl Read + Send + 'static>, endpoint: Endpoint) {
    let Some(mut stdout) = stdout else { return };
    let spawned = std::thread::Builder::new()
        .name("model-relay".into())
        .spawn(move || {
            let _ = std::io::copy(&mut stdout, &mut std::io::sink());
            let forwards = match running().as_mut() {
                Some(relay) if relay.endpoint == endpoint => std::mem::take(&mut relay.forwards),
                _ => return,
            };
            close_forwards(&endpoint, &forwards);
        });
    if let Err(error) = spawned {
        log::warn!("the model relay is not watched: {error}");
    }
}

fn read_port(file: &Path) -> Option<u16> {
    std::fs::read_to_string(file).ok()?.trim().parse().ok()
}

/// This computer's available models, from the relay (which reloads pi's settings first).
fn fetch_models(endpoint: &Endpoint, control_key: &[u8; 32]) -> Result<Vec<Value>, String> {
    let body = request(endpoint, control_key, "GET", "/_pistachio/models", b"")?;
    let body: Value = serde_json::from_slice(&body).map_err(|error| error.to_string())?;
    Ok(body["models"].as_array().cloned().unwrap_or_default())
}

trait Connection: Read + Write {}
impl<T: Read + Write> Connection for T {}

fn connect(endpoint: &Endpoint) -> std::io::Result<Box<dyn Connection>> {
    match endpoint {
        Endpoint::Port(port) => {
            let stream = TcpStream::connect(("127.0.0.1", *port))?;
            stream.set_read_timeout(Some(ANSWER_TIMEOUT))?;
            Ok(Box::new(stream))
        }
        Endpoint::Socket(path) => connect_socket(path),
    }
}

#[cfg(unix)]
fn connect_socket(path: &Path) -> std::io::Result<Box<dyn Connection>> {
    let stream = std::os::unix::net::UnixStream::connect(path)?;
    stream.set_read_timeout(Some(ANSWER_TIMEOUT))?;
    Ok(Box::new(stream))
}

#[cfg(not(unix))]
fn connect_socket(_path: &Path) -> std::io::Result<Box<dyn Connection>> {
    Err(std::io::ErrorKind::Unsupported.into())
}

/// One request to the relay, signed (a program that took over its port must not get a secret)
/// and with the answer's signature checked.
fn request(
    endpoint: &Endpoint,
    control_key: &[u8; 32],
    method: &str,
    path: &str,
    body: &[u8],
) -> Result<Vec<u8>, String> {
    use sha2::{Digest, Sha256};
    let fail = |error: std::io::Error| error.to_string();
    let nonce = hex(&random::<16>());
    let sign = |text: String| hex(&hmac_sha256(control_key, text.as_bytes()));
    let auth = sign(format!(
        "request {nonce} {method} {path} {}",
        hex(&Sha256::digest(body))
    ));
    let mut stream = connect(endpoint).map_err(fail)?;
    let mut head = format!(
        "{method} {path} HTTP/1.0\r\nHost: relay\r\nx-pistachio-relay-auth: {nonce}-{auth}\r\n\
         content-length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    head.extend_from_slice(body);
    stream.write_all(&head).map_err(fail)?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).map_err(fail)?;
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("no response from the model relay")?;
    let head = String::from_utf8_lossy(&response[..split]);
    if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
        return Err(format!(
            "the model relay answered {}",
            head.lines().next().unwrap_or_default()
        ));
    }
    let body = &response[split + 4..];
    let proof = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("x-pistachio-relay-proof"))
        .map(|(_, value)| value.trim());
    let expected = sign(format!("answer {nonce} {}", hex(&Sha256::digest(body))));
    if proof != Some(expected.as_str()) {
        return Err("something else answers in place of the model relay".to_owned());
    }
    Ok(body.to_vec())
}
