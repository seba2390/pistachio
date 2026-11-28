# Pistachio

**Pistachio is an open-source, native desktop app for the [pi coding agent](https://pi.dev).**
It is written in Rust and drawn on the GPU with [GPUI](https://gpui.rs), so it starts quickly,
stays smooth in long sessions and uses about 90 MB of memory when idle. It runs on macOS, Linux
and (experimentally) Windows.

Pistachio runs pi itself (`pi --mode rpc`) and draws pi's own event stream, so a session behaves
exactly as it does in pi's terminal: the same sessions, models, skills, extensions and settings,
and you can switch between the app and the terminal at any time. Extra tools are opt-in: Code
sessions get pi's own tools, plus a way to ask you for help while the sandbox is on, until you
turn more on in [Settings → Tools](settings/tools.md).

- **Easy to use and polished.** A clean, intuitive interface built to the standard of the
  Claude Code and Codex desktop apps: streaming markdown, inline diffs and readable tool calls.
- **Chat and Code.** Plain chats that can't touch your files, or the agent working in a project
  folder, with a terminal, git changes and a file tree beside it.
- **Local models in one click.** Pistachio scans your computer's hardware and suggests models
  that run well on it (Tiny, Fast, Balanced, Most capable), each with the inference engine that runs it
  best there. One click downloads, verifies and runs it, ready for your chats.
- **SSH remotes.** Run pi on another machine. Pistachio installs pi there if needed, and your
  models work there with the keys kept on your computer.
- **Speech to text.** Dictate your messages. A model that runs on your computer transcribes them,
  so nothing you say leaves it.
- **Document parsing.** Attach PDFs, Word, PowerPoint, Excel and EPUB files, with optional parsers
  for complex layouts, scans and formulas.
- **Web search and deep research** in chats.
- **Sandboxing and security.** The agent's commands see only the project and the folders you
  allow, never your keys. Pushing, GitHub and signing go through a gate that asks you first,
  and a step the sandbox blocks can be handed to you to do yourself.
- **Background sessions, branches and side chats**, so several things can run at once.

## Install

**1. Install pi** (needs Node.js 22.19 or newer), then set it up as usual:

```bash
npm i -g @earendil-works/pi-coding-agent
```

Or skip this step: when Pistachio doesn't find pi, it offers **Install pi** (see
[Finding pi](general.md#finding-pi)).

**2. Install Pistachio.**

macOS (Apple Silicon)
: With [Homebrew](https://brew.sh):

  ```bash
  brew install --cask seba2390/tap/pistachio
  ```

  Without Homebrew, download `Pistachio-…-macos-arm64.zip` from the
  [latest release](https://github.com/seba2390/pistachio/releases/latest), unzip it and move
  `Pistachio.app` to Applications. The app isn't notarized yet, so allow the first launch with:

  ```bash
  xattr -dr com.apple.quarantine /Applications/Pistachio.app
  ```

Linux (x86_64 and ARM64)
: Download the package for your system from the
  [latest release](https://github.com/seba2390/pistachio/releases/latest) and install it:

  ```bash
  sudo apt install ./pistachio_*.deb      # Debian, Ubuntu
  sudo dnf install ./pistachio-*.rpm      # Fedora and other RPM-based systems
  ```

  On any other distribution, unpack `pistachio-v…-linux-<arch>.tar.gz` and run the
  `install.sh` inside it. It installs for your user only (no root) and adds Pistachio to your
  app launcher. The [README](https://github.com/seba2390/pistachio#install) has one-line
  commands for each.

Windows (x86_64, experimental)
: Download `Pistachio-…-windows-x86_64-setup.exe` from the
  [latest release](https://github.com/seba2390/pistachio/releases/latest) and run it. It
  installs for your user (no administrator rights) into `%LOCALAPPDATA%\Programs\Pistachio`,
  adds Pistachio to the Start menu (and, if you like, the desktop) and uninstalls from Settings →
  Apps. To run it without installing, unzip `pistachio-…-windows-x86_64.zip` and run
  `pistachio.exe`. Windows may warn about an unknown publisher the first time
  (More info → Run anyway). SSH remotes are untested on Windows, and there is no sandbox yet.

From source
: Needs [Rust](https://rustup.rs). Linux also needs the GPUI system libraries listed in
  [CONTRIBUTING.md](https://github.com/seba2390/pistachio/blob/main/CONTRIBUTING.md#setup).

  ```bash
  cargo install --locked --git https://github.com/seba2390/pistachio pistachio
  ```

Then launch **Pistachio** (or run `pistachio`). It looks for pi and, if it finds none, offers to
install it or to find it ([Finding pi](general.md#finding-pi)). You can also set it in
[Settings → pi & models](settings/pi-and-models.md), or start the app with
`PISTACHIO_PI_BIN=/path/to/pi`.

Pistachio offers new versions itself: when one is out, **Update** installs it the way you
installed the app (see [Updates](general.md#updates)).

## First steps

1. Pick **Chat** (⌘1) to talk, or **Code** (⌘2) to have the agent work in a project folder.
2. Press ⌘N for a new conversation. In Code mode you choose the folder, on this computer or an
   SSH host.
3. Type and press Enter. The message box works like pi's terminal: `/` for commands, `@` to
   mention a file, `!` to run a shell command.

[General](general.md) walks through each part of the app. The Settings pages (⌘,) each have a
page here.

On Linux and Windows, use Ctrl where these pages say ⌘.

```{toctree}
:hidden:

Home <self>
general
```

```{toctree}
:caption: Settings
:hidden:

settings/pi-and-models
settings/web-search
settings/ssh-connections
settings/speech-to-text
settings/conversation-names
settings/document-parsing
settings/sandboxing-and-security
settings/tools
```

```{toctree}
:caption: Project
:hidden:

GitHub <https://github.com/seba2390/pistachio>
Releases <https://github.com/seba2390/pistachio/releases>
Changelog <https://github.com/seba2390/pistachio/blob/main/CHANGELOG.md>
Contributing <https://github.com/seba2390/pistachio/blob/main/CONTRIBUTING.md>
```
