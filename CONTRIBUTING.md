# Contributing to Pistachio

Thanks for helping! Bug reports, ideas and pull requests are all welcome.

## Ground rules

- **Pistachio is a view over pi.** Agent behaviour belongs in pi, not here: the app renders pi's
  RPC events and forwards your input. If a change re-implements something pi does, it
  probably belongs upstream.
- **Keep the UI crate thin.** Logic goes into the UI-agnostic crates (`transcript`,
  `workspace`, `remote`, …) with unit tests; `crates/app` wires it to GPUI. See
  [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
- **Stay lean.** Memory and idle CPU matter: no polling loops, no work for hidden views.
- Open an issue first for anything large, so we can agree on the approach.

## Setup

You need stable Rust (see `rust-toolchain.toml`) and [pi](https://pi.dev) on your `PATH`.

- **macOS:** Xcode command line tools (`xcode-select --install`).
- **Linux (Debian/Ubuntu):**

  ```bash
  sudo apt-get install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan-dev libx11-xcb-dev libxcb1-dev libfontconfig-dev libfreetype-dev libzstd-dev libasound2-dev
  ```

Then:

```bash
cargo run            # debug build
cargo run --release  # what users get
```

## Before you open a pull request

CI runs exactly these; please run them locally:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- Add or update tests for logic changes (UI-agnostic crates make this easy).
- Keep commits focused, with messages that say *why*.
- Update [docs/FEATURES.md](docs/FEATURES.md) and [CHANGELOG.md](CHANGELOG.md) for
  user-visible changes, and the documentation site's page for it (`docs/general.md` or
  `docs/settings/`).

**The documentation site** (https://seba2390.github.io/pistachio/) is built with Sphinx, Furo and
MyST from `docs/`, and `docs.yml` publishes it when `docs/` changes on `main`. To build it
locally:

```bash
uv venv --python 3.14 /tmp/pistachio-docs && VIRTUAL_ENV=/tmp/pistachio-docs uv pip install --require-hashes -r docs/requirements.txt
/tmp/pistachio-docs/bin/sphinx-build -M html docs docs/_build -W --keep-going -n   # open docs/_build/html/index.html
```

## Releasing

For maintainers. Describe the changes under `## [Unreleased]` in [CHANGELOG.md](CHANGELOG.md) (it
becomes the release notes), then review the documentation as below, commit everything, and run
this on the branch with the changes:

```bash
script/release.sh 0.5.0 --docs-review /tmp/docs-review.md   # --dry-run checks everything and changes nothing
```

**Review the documentation before every release**, patch and internal-only ones included, even
when no documentation file changed:

1. **Find out what is being released, from summaries.** Read the commit messages since the last
   released tag (`git log --oneline $(git describe --tags --abbrev=0)..HEAD`) and the changelog's
   Unreleased section, and pull requests' titles and descriptions where they help. Don't read all
   the diffs: look at a specific change only when the summaries leave a concrete documentation
   question open.
2. **Go through all the documentation**, not only pages changed in that range: `README.md` and
   `docs/index.md` (overview, installation, prerequisites, platforms); `docs/FEATURES.md`,
   `docs/general.md` and every page in `docs/settings/` (features, workflows, defaults, setting
   names, shortcuts, limitations); `docs/ARCHITECTURE.md`, `CONTRIBUTING.md`, `AGENTS.md` and the
   other guides where the changes touch what they say; and screenshots and examples where changed
   UI or behaviour could make them misleading.
3. **Fix what is missing, changed or obsolete** before releasing, and check that the README, the
   feature list, the documentation site and the release notes agree. Settle unclear claims with
   targeted checks, not guesses. Update screenshots only where needed.
4. **Validate.** When the site's content changed, build it as above
   (`sphinx-build … -W --keep-going -n`), fix warnings and errors, and check the affected links,
   examples and pages. A passing build doesn't replace the review.
5. **Record it** in a file outside the repository, for `--docs-review`; it goes into the release
   pull request:

   ```markdown
   - Range: v0.5.8..HEAD (commit messages, changelog, PRs #56 and #57)
   - Checked: README, docs/index.md, FEATURES, general, every settings page, ARCHITECTURE,
     CONTRIBUTING, AGENTS, screenshots
   - Updated: docs/settings/tools.md (new switch) — or "Reviewed; no updates needed"
   - Validation: sphinx-build -M html docs docs/_build -W --keep-going -n: passed (or "not run:
     the site didn't change")
   ```

   An unresolved question about whether the documentation is right, or validation that fails,
   stops the release until it is fixed.

The script refuses to release without `--docs-review`. It runs CI's checks, sets the version, dates the changelog section, pushes, opens a pull request
into `main` and turns on auto-merge. Then GitHub does the rest: it merges once the required checks
pass, `tag-release.yml` tags the merge and starts `release.yml`, and that builds the downloads,
publishes the release and updates the Homebrew cask. If a required check fails, nothing is
released: push a fix to the branch. If `release.yml` fails, run it again on the tag (Actions →
Release → Run workflow).

## CI and release builds

- **Caches.** `rust-cache` keeps compiled dependencies. A run can restore caches saved on its own
  branch or on `main`, and a release runs for a tag, so `release-cache.yml` builds the release
  binaries on `main` (on each push except documentation, and weekly) and saves them under
  `release-<platform>`; `release.yml` restores those and saves nothing. CI saves from `main` only.
  The repository has 10 GB for caches; caches unused for a week are deleted.
- **Build logs.** Release builds run through `script/ci-build.sh`: `cargo build --timings` with the
  machine's memory, swap (Windows: commit charge), disk, CPU and largest processes printed every
  30 s between cargo's lines. Each build job's summary says how it went, how many crates were
  compiled (nearly all of them: the cache wasn't used), the peak memory and, after a failure, the
  end of the log. The log, the samples and cargo's timings report (`cargo-timing.html`) are in the
  job's `build-log-<platform>` artifact, also when the build failed or ran out of time. A build
  stops after 60 minutes (a normal one takes 15–25), so one that stalls fails with its logs.
- If a release build fails, read its summary and artifact first; run it again only once the
  cause is known.

## Handy tools

```bash
script/bundle-macos.sh [debug|release]   # wrap the binary in Pistachio.app (macOS)
script/shot.sh <folder> out.png terminal,diff   # relaunch the debug app and screenshot it
script/measure-memory.sh                 # memory of a running instance and its pi children
script/perf.sh [release|debug] [runs] [baseline.json]   # time startup, navigation and quitting
script/check-linux-displays.sh <app> <folder>  # that the app opens on X11 and on Wayland (Linux)
script/make-icon.sh                      # regenerate the .icns / PNG from icon.svg
script/set-version.sh 0.5.0              # bump the version (Cargo.toml, Cargo.lock, README install links)
script/update-searxng.sh                 # move the pins of the SearXNG the app can download
script/update-parsers.sh                 # move the pins of the document parsers the app can download
script/update-local-models.sh <radar>    # copy the local-model catalog from a local-llm-radar checkout
script/release.sh 0.5.0 --docs-review <file>   # release a version (see Releasing)
script/ci-build.sh --release --locked    # CI: a build with resource samples and timings (see CI and release builds)
```

`script/shot.sh` stops any running `Pistachio.app` first.

**Opening views at startup.** `PISTACHIO_OPEN` takes a comma-separated list, which makes layout
checks scriptable:

- Mode and panels: `chat`, `code`, `terminal`, `diff`, `files`, `no-sidebar`;
  `diff-expand-all` opens the Changes tab with every file expanded, and `expand-tools` expands
  every tool call in the chat (like Ctrl+O); `view-file:<path>` opens a file (relative to the
  folder) in the Files tab's viewer.
- Docked above the composer: `question` and `question-plain` (the sandbox gate's question, with
  and without a warning), `help-request` and `help-request-open` (a `request_user_help` pill,
  folded and opened).
- Sessions and popovers: `latest`, `new-chat` (like ⌘N), `attach`, `context`; `find:<text>` opens the find bar
  searching for `<text>`; `attach:<path>` attaches a file and `send:<text>` sends `<text>` as a
  message, once what is attached is read; `image-viewer:<n>` opens the picture viewer on the session's
  `<n>`th picture; `side-chat` opens a side chat of it 2 s after start; `report-check` opens how the session's last deep search report was checked.
- Dialogs: `add-host`, `manage-hosts`, `pi-missing` (the card that installs or finds pi), `models`, `models-provider:<id>`, `models-server:<address>`,
  `edit-models`.
- Skills page: `skills`, `new-skill`, `open-skill:<path>`; the Settings page: `settings`
  (`settings-web`, `settings-speech`, `settings-names`, `settings-documents`, `settings-security` and `settings-tools` open those pages, `settings-git` its Git and GitHub section, `settings-at:<page>:<section>` any page scrolled to a section, 0 being General, as for the documentation's screenshots; after `settings-web`,
  `web-advanced` opens its advanced sections,
  `searxng-setup` asks whether to download SearXNG and `searxng-get` downloads and runs it); `type-name:<text>` plays the
  sidebar's typing animation for the open session 2.5 s after start (nothing is saved).
- SSH: `ssh:<alias>`, `remote:<alias>:<folder>` and `remote-latest:<alias>` open a remote
  session once connected. Panels listed after them apply to that session.

**Response times.** `script/perf.sh` builds the app and runs a fixed navigation script
(`PISTACHIO_BENCH=1`, see `crates/app/src/perf/bench.rs`) against generated sessions in a
private pi folder, then prints the median time from each input to the frame that shows its
response on screen, plus startup phases, the time to quit and render times per view. Pass the
`summary.json` of an earlier run to compare. The window takes focus for about 20 s per run, and
each run moves one generated session to the Trash. `PISTACHIO_PERF=<file.jsonl>` records the
same measurements in a normal session.

**Update prompt.** In debug builds, `PISTACHIO_PRETEND_VERSION=0.0.1` makes "Check for
Updates…" treat the latest release as new.

**Separate state.** Setting `PI_CODING_AGENT_DIR` to a scratch folder runs the app with its
own state, so it won't touch your normal pi sessions or settings.

**The README demo.** `script/demo/record-demo.sh` re-records `docs/media/demo.mp4` after UI
changes. It needs [OpenScreen](https://getopenscreen.com) (`brew install --cask openscreen`) and
`ffmpeg` (`gifski` too for `post.sh --gif`, a short looping GIF), plus the model named in
`script/demo/readme-demo.toml`. It sets up an isolated agent dir and a demo project in `/tmp`,
plays the storyboard in the app's demo mode (`PISTACHIO_DEMO=<storyboard.toml>`, see
`crates/app/src/demo`) while OpenScreen records the window, then edits the take
(`script/demo/post.py`). Leave the mouse and keyboard alone while it records. The steps also run
on their own: `setup.sh`, `record.sh` and `post.sh` in `script/demo/`. GitHub only plays videos
uploaded to it, so after re-recording, drag the new `demo.mp4` into a comment box on github.com
(without posting) and put the link it creates at the top of the README.

## Tests against real hosts

These tests are all `#[ignore]`d. Run them with `PISTACHIO_SSH_TEST_HOST=<alias>`:

- `cargo test -p remote --test live -- --ignored`: connection, probe, quoting, remote git,
  files, sessions and trash in a scratch folder.
  - `live_install` installs pi on the host, so it writes to the host.
- `cargo test -p pistachio -- --ignored`: RPC over SSH with a scripted fake pi.
  - `real_pi_over_ssh` starts real pi the way a remote session does.
  - `askpass_prompt_reaches_the_app` also needs `PISTACHIO_SSH_PASSWORD_HOST`, a host that
    offers password login.

`PISTACHIO_PREVIEW_CHECK=<folder of session copies> cargo test -p pistachio preview_matches_pi --
--ignored` runs real pi on each session file and checks that the history Pistachio previews
from the file matches pi's (use copies: pi may migrate files it opens).

`script/test-sandbox.py` runs the sandbox's extension in real pi with a scripted model, in a
made-up home folder: commands and file tools that should be allowed or blocked (`cargo build`
first).

`script/test-deepsearch.py` runs deep search's extension in real pi, with a scripted model, search
service and pages on this computer (`cargo build` first). It covers what no unit test can: sending
the model back, revisions, a model that does not answer, Esc and more.

`LOCAL_MODELS_LIVE=lfm3-1.7b/gguf-q4_k_m/llama-cpp cargo test -p local-models --test live --
--ignored --nocapture` really downloads an engine and a model (about 1 GB), starts it and asks it
something; any `<model>/<build>/<engine>` that runs on the computer works.
`cargo test -p local-models typical_computers -- --ignored --nocapture` prints what typical
computers would be offered, to look over after moving the catalog.

`cargo test -p documents --test live -- --ignored --test-threads 1` really downloads the document
parsers (2 GB, into a temporary folder) and reads a PDF with each; add `pymupdf4llm`, `docling` or
`mineru` for one of them.

`TITLES_PI_MODEL=<provider/model> cargo test -p titles real_pi -- --ignored --nocapture` names
a few sample first messages with real pi and that model (the conversation-model namer).

`crates/transcript/tests/fixtures` holds sanitized recordings of real `pi --mode rpc`
sessions. The replay tests check that streaming and history render the same way.

## Reporting security issues

Please don't open public issues for vulnerabilities; see [SECURITY.md](SECURITY.md).

## License

By contributing you agree that your work is licensed under the project's dual
[MIT](LICENSE-MIT) / [Apache-2.0](LICENSE-APACHE) license.
