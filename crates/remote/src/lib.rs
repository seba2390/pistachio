//! UI-agnostic SSH support for Pistachio: pi runs *on* the remote machine and the GUI talks to it
//! over `ssh host … pi --mode rpc`.
//!
//! * [`config`] — host aliases discovered from the user's OpenSSH config, and `ssh -G`
//!   resolution of a target to its effective hostname, user and port.
//! * [`host`] — saved connections (`<agent dir>/pistachio/hosts.json`).
//! * [`ssh`] — talking to one host: a shared (`ControlMaster`) connection, remote shell scripts,
//!   probing for pi, listing remote folders, and the command line that starts pi remotely.
//!   Also: installing pi automatically (`Ssh::install_pi`), interactive connections with
//!   in-app prompts ([`askpass`]), remote terminals, uploads and changed host keys, and the
//!   [`workspace::RemoteShell`] the diff / files panels run their scripts through.
//! * [`sessions`] — pi's session files on a host, listed incrementally, and remote trash.
//! * [`link`] — the host side of using this computer's models there: a port forward to the
//!   app's model relay and the pi settings folder Pistachio manages on the host.
//! * `tunnel` — a host's connection where OpenSSH cannot share one (Windows).
//!
//! Everything goes through the system `ssh` binary so the user's `~/.ssh/config`, keys,
//! `ssh-agent`, `known_hosts` and `ProxyJump` all behave exactly as in a terminal. All functions
//! are synchronous and may block on process I/O; callers are expected to run them on a
//! background executor.

pub mod askpass;
pub mod config;
mod error;
mod extras;
pub mod host;
mod install;
pub mod link;
mod local_network;
mod process;
pub mod sessions;
pub mod ssh;
mod tunnel;

pub use askpass::{AskpassServer, Prompt};
pub use config::{
    ConfigHost, ConfigValues, Resolved, config_values, discover_hosts, parse_config, resolve,
};
pub use error::{RemoteError, classify};
pub use host::{HostId, HostStore, SavedHost, Target};
pub use install::install_script;
pub use link::{Endpoint, MANAGED_AGENT_DIR, RelayProof};
pub use sessions::SessionIndex;
pub use ssh::{
    DirListing, LEGACY_MANAGED_DIR, MANAGED_DIR, Probe, RELAY_PORT_ENV, Ssh, remote_word, sh_quote,
};
