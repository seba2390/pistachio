//! Helpers for the operating system that several crates need.
//!
//! - [`NoWindow`]: child processes that open no console window. Pistachio is a GUI program
//!   without a console on Windows, so every console program it starts there (`pi.cmd`, `git`,
//!   PowerShell, an engine) would otherwise get a console window of its own, popping up on the
//!   screen while it runs. Everywhere else it does nothing.
//! - [`windows`]: what Windows itself says about the computer (memory, processor, graphics,
//!   free disk), read without starting another program.

use std::ffi::OsStr;
use std::process::Command;

#[cfg(windows)]
pub mod windows;

/// Windows' `CREATE_NO_WINDOW` process creation flag.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Starting console programs without a console window (Windows only; elsewhere a no-op).
pub trait NoWindow {
    /// The program runs without a console window: its output still reaches pipes and files,
    /// but nothing shows on the screen. Replaces any other creation flags.
    fn no_window(&mut self) -> &mut Self;
}

impl NoWindow for Command {
    fn no_window(&mut self) -> &mut Self {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

/// `Command::new(program)` that opens no console window on Windows.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.no_window();
    command
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn commands_still_run_and_pipe() {
        let output = command("/bin/sh")
            .args(["-c", "printf hi"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"hi");
    }
}
