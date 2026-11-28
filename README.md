<p align="center">
  <img src="crates/app/assets/app-icon/icon-512.png" width="128" alt="Pistachio logo">
</p>

<h1 align="center">Pistachio</h1>

<p align="center">
  A fast, native desktop app for the <a href="https://pi.dev">pi coding agent</a> written in Rust.
</p>

<p align="center">
  <a href="https://github.com/seba2390/pistachio/actions/workflows/ci.yml"><img src="https://github.com/seba2390/pistachio/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/seba2390/pistachio/releases"><img src="https://img.shields.io/github/v/release/seba2390/pistachio?include_prereleases" alt="Release"></a>
  <a href="https://seba2390.github.io/pistachio/"><img src="https://img.shields.io/badge/docs-seba2390.github.io%2Fpistachio-00664A" alt="Documentation"></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="License"></a>
</p>

**Pistachio is an open-source, native desktop app for the [pi coding agent](https://pi.dev).**
It is written in Rust and drawn on the GPU with [GPUI](https://gpui.rs), so it starts quickly,
stays smooth in long sessions and uses about 90 MB of memory when idle. It runs on macOS, Linux
and (experimentally) Windows.

Pistachio runs pi itself (`pi --mode rpc`) and draws pi's own event stream, so a session behaves
exactly as it does in pi's terminal: the same sessions, models, skills, extensions and settings,
and you can switch between the app and the terminal at any time. Extra tools are opt-in: Code
sessions get pi's own tools, plus a way to ask you for help while the sandbox is on, until you
turn more on in [Settings → Tools](https://seba2390.github.io/pistachio/settings/tools.html).

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

The [documentation](https://seba2390.github.io/pistachio/) explains each feature and Settings
page. See [docs/FEATURES.md](docs/FEATURES.md) for the full list and keyboard shortcuts.

<!-- docs/media/demo.mp4, uploaded through GitHub (it only plays videos hosted there). After
re-recording it, upload the new file the same way and replace this link. -->
https://github.com/user-attachments/assets/45a10dcc-881a-451d-a78e-cefb881ec8d0


## Install

**1. Install pi** (needs Node.js 22.19+), then set it up as usual:

```bash
npm i -g @earendil-works/pi-coding-agent
```

Or skip this step: when Pistachio doesn't find pi, it offers **Install pi**, which installs it
(with its own Node.js if needed) into `~/.pistachio/server`, without administrator rights.

**2. Install Pistachio**

- **macOS (Apple Silicon)** with [Homebrew](https://brew.sh):

  ```bash
  brew install --cask seba2390/tap/pistachio
  ```

  Pistachio offers new versions itself and installs them with Update (or run
  `brew upgrade --cask pistachio`).
  <details>
  <summary>macOS without Homebrew</summary>

  Download `Pistachio-…-macos-arm64.zip` from
  [Releases](https://github.com/seba2390/pistachio/releases), unzip it and move `Pistachio.app` to
  Applications. The app isn't notarized yet, so macOS will block the first launch. Allow it once
  with:

  ```bash
  xattr -dr com.apple.quarantine /Applications/Pistachio.app
  ```

  </details>

- **Linux (x86_64 and ARM64):** paste the command for your system into a terminal. It picks the
  right download for your processor.
  - Debian, Ubuntu:

    ```bash
    curl -fLO https://github.com/seba2390/pistachio/releases/download/v0.6.1/pistachio_0.6.1-1_$(dpkg --print-architecture).deb && sudo apt install ./pistachio_0.6.1-1_$(dpkg --print-architecture).deb
    ```

  - Fedora and other RPM-based systems:

    ```bash
    sudo dnf install https://github.com/seba2390/pistachio/releases/download/v0.6.1/pistachio-0.6.1-1.$(uname -m).rpm
    ```

  - Any other distribution (installs for your user only, no root, and adds "Pistachio" to your
    app launcher):

    ```bash
    curl -fL https://github.com/seba2390/pistachio/releases/download/v0.6.1/pistachio-v0.6.1-linux-$(uname -m).tar.gz | tar xz && pistachio-v0.6.1-linux-$(uname -m)/install.sh
    ```

  Or download the files yourself from [Releases](https://github.com/seba2390/pistachio/releases).
- **Windows (x86_64, experimental):** download `Pistachio-…-windows-x86_64-setup.exe` from
  [Releases](https://github.com/seba2390/pistachio/releases) and run it. It installs for your
  user (no administrator rights), adds Pistachio to the Start menu and uninstalls from Settings →
  Apps. The `pistachio-…-windows-x86_64.zip` holds the same `pistachio.exe` to run without
  installing.
  - Windows may warn about an unknown publisher the first time (More info → Run anyway).
  - SSH remotes are untested on Windows.
- **From source** (any platform, needs [Rust](https://rustup.rs)):

  ```bash
  cargo install --locked --git https://github.com/seba2390/pistachio pistachio
  ```

  Linux also needs the GPUI system libraries listed in [CONTRIBUTING.md](CONTRIBUTING.md#setup).

Then launch **Pistachio** (or run `pistachio`). It looks for pi on your `PATH` and where npm
and Node.js version managers (nvm, fnm, Volta, …) put it; if it finds none, it offers **Install
pi** or **Find pi…**. `PISTACHIO_PI_BIN=/path/to/pi` chooses one yourself.

## Usage

Pick **Code** (⌘2) to work in a folder or **Chat** (⌘1) to just talk. The message box works
like pi's terminal:

| Key | Action |
|---|---|
| Enter / Shift+Enter | Send (steers while pi works) / new line |
| Alt+Enter | Queue a follow-up |
| Esc | Interrupt |
| `/` · `@` · `!` | Commands · mention a file · run a shell command |
| ⌘N · ⌘B · Ctrl+` | New session · sidebar · terminal |

## Contributing

Issues and pull requests are welcome. Start with [CONTRIBUTING.md](CONTRIBUTING.md), and see
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the code is organized. Please report
security issues privately ([SECURITY.md](SECURITY.md)).

## Acknowledgements

Pistachio is an independent project. It is not affiliated with or endorsed by the authors of pi.
It is built on:

- [pi](https://github.com/earendil-works/pi) by Mario Zechner (MIT), the agent this app is a
  window onto.
- [GPUI](https://gpui.rs) by Zed Industries (Apache-2.0) and
  [gpui-kit](https://github.com/longbridge/gpui-kit) by Longbridge (Apache-2.0).
- [alacritty_terminal](https://github.com/alacritty/alacritty) (Apache-2.0) for the
  integrated terminal, and [Lucide](https://lucide.dev) icons (ISC).

Release downloads include the full notices of every bundled dependency in
`THIRD-PARTY-LICENSES.txt`. You can generate the same file with
`script/third-party-licenses.py`.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
Unless you state otherwise, any contribution you submit is dual-licensed the same way, with no
extra terms.
