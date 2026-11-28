# Changelog

All notable changes to PI-gui are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **A Windows installer.** `Pistachio-…-windows-x86_64-setup.exe` installs Pistachio for your
  user (no administrator rights) into `%LOCALAPPDATA%\Programs\Pistachio`, with the Pistachio
  logo throughout, a Start menu entry, an optional desktop shortcut, **Launch Pistachio** at the
  end and an uninstaller in Settings → Apps. The zip is still there, to run without installing.
- **Pistachio finds pi, or installs it.** At startup it looks for pi: on the `PATH` (also the
  login shell's), then where npm, nvm, fnm, Volta, asdf, mise, n, nvm-windows, Scoop, pnpm, Bun,
  Yarn and Homebrew put it, and uses what it finds. When pi is nowhere, a card offers **Install
  pi**, which installs it unattended, with its own Node.js if needed and without administrator
  rights, into `~/.pistachio/server` on macOS, Linux and Windows, and **Find pi…**, to choose the
  program (or its folder), which is checked before it is used.
- **A fourth suggested model: Tiny.** Deploy new model now suggests Tiny, Fast, Balanced and Most
  capable. Tiny is the most capable model of at most 3B parameters, in its lightest build, so
  there is always one that nearly any computer runs. The catalog adds MiniCPM5-2B (ahead of
  Qwen3.5-4B on tool calling, 1.6 GB) and MiniCPM5-1B (0.7 GB).
- **Downloads wait for the internet.** Where Pistachio downloads something (a suggested local
  model, a document parser, a speech-to-text or naming model, SearXNG, an update), it first checks
  that the internet can be reached. When it can't, the download button shows a crossed-out Wi-Fi
  icon and says **No internet**, and nothing starts; clicking it checks again.
- **A progress bar while a local model downloads.** **Download and run** turns into the step it is
  on, how much has arrived of how much, a progress bar and Cancel.

### Changed

- The names of the suggested models are centered on their cards.
- Windows: `pistachio.exe` has the Pistachio icon (in Explorer, the taskbar and the window) and
  says what it is in Task Manager.

### Fixed

- Windows: the window had no minimize, maximize and close buttons. They are at the right of the
  top bar (also on Linux when the app draws its own window frame).
- Windows: scanning the hardware, downloading and running a model, and other work opened
  console (PowerShell) windows. Programs Pistachio starts now run without a window.
- Windows: the hardware scan often found no processor or memory (PowerShell took too long to
  start), and never the free disk or a graphics card other than NVIDIA's. It now asks Windows
  itself: memory, processor, free disk and any graphics card with memory of its own (AMD and Intel
  too).
- The hardware scan's bar now fills from empty over two seconds (longer if the scan takes longer)
  and its loader turns, also when the system asks for reduced motion; before, it showed full at
  once.
- The tokens-per-second pill stayed away for good for local models (Ternary Bonsai with
  llama.cpp, for one) once a reply that only called a tool, like a web search, was taken for
  hidden thinking: a tool call costs tokens that never stream. Replies that call a tool are no
  longer judged, the latest judged reply decides, and a server that reports no token counts gets
  estimates (marked "~") instead of no pill.
- Cancel on a local model's download takes effect at once: the button comes back right away, also
  when the connection had stalled.

## [0.6.0] - 2026-11-28

### Added

- **Models suggested for this computer.** Add models… → Locally hosted model → This device now
  looks at the computer (memory, chip or graphics card, free disk) and suggests up to three models:
  Fast, Balanced and Most capable. Each comes with the engine that runs it best here, with its
  estimated speed and memory: a default that installs unattended (llama.cpp, Rapid-MLX, Splash,
  BigMoeOnEdge) and, where clearly faster, a specialized engine built on the computer
  (ik_llama.cpp, TurboFieldfare). Download says how much it is and asks first, listing exactly what
  is downloaded; every file is pinned and checked against its SHA-256. A model that runs is
  connected like any server, is started when a chat sends to it (the message waits until it
  answers) and stops when Pistachio quits.
- **Attach or deploy on this device.** Locally hosted model is now a series of pages: This device
  or Another device, then on this device **Attach to existing model** (Ollama, LM Studio, vLLM…
  and a server by its port) or **Deploy new model**. Deploying starts with **Scan local
  hardware**, which shows a progress bar through what it looks at and then lists the processor,
  memory, graphics, free disk and build tools found; **Suggest models** then shows Fast, Balanced
  and Most capable, each with its inference engine, quantization, memory, speed here and checkpoint.
  The suggestions start collapsed to their name and a centered **Download and run** (or Run,
  Connect) button; the full-width arrow at the bottom of each opens why it fits and how it runs. The engine and the checkpoint link to their pages, also in the download
  dialog, and the button says **Download and run**. Opened, a suggestion also shows its context,
  whether it takes images, and whether it thinks, with the levels pi offers for it here.
- **Thinking levels for local models.** A model Pistachio downloaded is added to `models.json`
  with `reasoning` and the `compat` (`chat-template` kwargs) and `thinkingLevelMap` its chat
  template needs, so the thinking levels in chat and code mode reach it: off or on for most, and
  their own levels for the four that take `reasoning_effort` (a template raises on a value it does
  not know). TurboFieldfare cannot pass thinking on, so a model it runs is added without it.
- **Connected models by where they run.** Settings → pi & models groups them into This device,
  Other devices and API connections, and each has a trash button that removes it after asking. A
  model Pistachio downloaded is also stopped and deleted from this computer, with its engine
  unless another local model uses it; for anything else only the connection is removed. A model
  Pistachio set up on this device also has a play / stop button that starts its engine (loading
  the model) or stops it, without deleting anything.
- **Tokens per second.** While a reply streams, a pill slides down from under the top bar with
  a line of the model's speed over the last seconds and its speed now ("~31 tok/s"); when the reply
  ends it turns gray with the reply's exact average ("avg. 30 tok/s") and slides back up after a
  few seconds. The live figure is an estimate from the streamed text, calibrated per model on its
  finished replies against the provider's exact count; hovering it says how far off it has been
  for that model on average. A provider that counts as it streams (Gemini) is shown exactly. A
  model that thinks without streaming it, or streams only a summary, gets no live estimate; nor
  does a model before one of its replies has checked out.
- **Python code as it is written.** Expanding a chat's "Computed" row while the model is still
  writing the call shows the code growing line by line, before it runs.
- **Thinking levels that do what they say on servers you add.** Adding a thinking model in
  Models → Locally hosted model now checks, in a few seconds, how its server takes thinking: which
  way of asking it reads, which levels change anything (levels that do the same show as one) and
  whether "off" really stops it. Before, "off" sent nothing to such servers, so most models went on
  thinking. A model that thinks a little at any effort, or can't stop, shows "lowest" instead of
  "off".

### Changed

- **A shorter, sharper Python tool text.** What the model is told about the python tool is down
  from about 1,750 to 700 characters: what it returns (errors included, to fix and retry), when to
  use it, said once, and the limits that would otherwise cost a failed call.

### Fixed

- A llama.cpp server is now added to `models.json` as `llama-cpp`, not `llama.cpp`: pi 1.5 has
  a built-in provider with that ID, which took the entry's place, so sending to it failed with
  "Provider is not configured: llama.cpp".

## [0.5.10] - 2026-11-27

### Added

- `request_user_help`, a tool for Code sessions on this computer: when the sandbox blocks a step
  the task needs and there is no way to do it inside the sandbox, the model can hand it to you. A
  pill above the message box says "Needs your help: …"; opened, it shows the command the sandbox
  blocked, its folder and its error apart from the model's reason and the command it suggests.
  **Copy command**, **Done** and **Dismiss**; the model gets your answer. It runs nothing and
  never asks for passwords or keys. It is offered while the sandbox is on, and has a switch in
  Settings → Tools → Code (on by default).

## [0.5.9] - 2026-11-27

### Fixed

- The gate no longer names GitHub accounts in what it prints back to the model: not in the
  remote it adds after creating a repository, nor when the repository changed while the user was
  asked. Fixes the code scanning alerts 9–12 (rust/cleartext-logging).

## [0.5.8] - 2026-11-27

### Changed

- The [documentation](https://seba2390.github.io/pistachio/) shows the app: every Settings page
  has screenshots of its sections, in light and dark to match the page, faded into it. Its home
  page says more plainly what Pistachio is, and the demo video stays in the README.

## [0.5.7] - 2026-11-27

### Added

- A documentation site, https://seba2390.github.io/pistachio/: each feature, and a page for each
  Settings page.

### Security

- The sandbox's git gate (pushing, pulling and `gh` with your logins) no longer runs git in the
  agent's repository at all. It keeps its own copy of each remote, per project, out of the
  sandbox's reach, and git with your keys works only there: fetches, pulls, clones, submodules
  and LFS downloads come in through that copy, and pushes go out from it (the commits are handed
  over as plain data). Settings the agent writes into its repository can no longer start
  programs with your keys, however they are set up, and the push question describes exactly what
  is pushed. Before, a folder set up as a bare repository, `git ls-remote <address>`, or changing
  `.git/config` at the right moment could get around the gate's checks.
- `gh` runs outside any repository, with the repository and branch named for it; body files are
  read inside the sandbox.
- A command runs only if it is still what you allowed: if the repository changed while you were
  asked, it stops and says so.

### Fixed

- The gate's error messages about which GitHub account to use no longer name the accounts (they
  go back to the model).
- A push through the gate uploads the commits' LFS files first, as git-lfs's own hook does (the
  gate runs git with hooks off, so they were left behind).
- The gate's `gh` works with older gh versions, such as Ubuntu's own package, which have no
  `gh auth status --json`: it said gh wasn't logged in.

### Changed

- `gh pr create` through the gate needs `--title` and `--body` (or `--body-file`); `--fill` isn't
  available. Pushes take branches and tags by name.

## [0.5.6] - 2026-11-27

### Added

- Settings → Tools: the tools the model gets in chats and in Code sessions. At the top of each,
  how much of the model's context the tools take with every message (about how many tokens, and
  each tool's share in a bar). Each tool has a one-line summary, where it comes from and its size;
  "What the model sees" opens exactly what pi tells the model (description, guidelines in the
  system prompt, parameters). pi itself is asked, so your extensions' tools are listed too.
- Switches on Settings → Tools: Python in chats (on by default), and pi's `grep`, `find`, `ls`
  and `codemode` in Code sessions on this computer (off by default). The sandbox checks `grep`,
  `find` and `ls` like `read`, and `codemode` scripts reach files only through those checked
  tools.
- The text Pistachio's own tools give the model (`python`, `web_search`, `web_fetch` and the
  sandboxed `bash`) can be edited on Settings → Tools (Edit, under "What the model sees"); "Use
  the original text" undoes it. It applies to conversations started afterwards.

### Changed

- Opening a conversation is faster: the model, thinking level, web search button and context
  wheel in the message box show at once instead of after pi has started (about a third of a
  second), from what pi said about the conversation last time or what its file selected.
- Chats that used web search (and any conversation with a compaction, extension messages or
  context edits) show their history at once too, read from the session file by pi's rules,
  instead of waiting for pi.

### Fixed

- Switching to another conversation no longer shows an empty "How can I help?" for a frame
  before its history.

- Chats use Python for more than numbers: the model is told to run a short program for any small
  job a program does more reliably than reasoning, such as counting letters or words, sorting or
  reformatting text, regular expressions, JSON, dates and weekdays, number bases, or trying a
  small algorithm on examples.

## [0.5.5] - 2026-11-26

### Added

- Chats compute numbers with Python: a `python` tool runs the model's code in Monty, a sandboxed
  Python interpreter written in Rust (no files, network or environment; limits on time, memory and
  recursion), so a sum, logarithm or series is worked out instead of estimated. Settings → General
  → Chat mode turns it off, also in open chats.
- A sandbox for the agent's commands in Code sessions on this computer, on by default (macOS and
  Linux). Commands run under Seatbelt on macOS and bubblewrap on Linux (Landlock where bubblewrap
  can't run). Of your home folder they see only the project, folders you allow and the folders of
  common tools; keys and logins (`~/.ssh`, `~/.aws`, pi's `auth.json`, …) never, and API keys in
  the environment are removed. They can change only the project, temporary folders, tool caches
  and folders you allow to change, and the network can be turned off. pi's file tools keep to the
  same rules. pi itself and its model requests run as before, so keys never enter the sandbox.
  Windows is planned (#47).
- Settings → Sandboxing & Security: the sandbox's state and settings, the tool folders and allowed
  folders (each to read, or read and write), the allowed websites (None, the default: each site
  is asked about; All; or Custom: the websites listed, checked as you type them, and others asked
  about), and which project files pi loads (its project trust default and saved decisions).
- The sandbox's gate: what needs your keys is asked about and done outside the sandbox. In Code
  sessions `git push`, `pull`, `fetch`, `clone`, `ls-remote`, submodules and git-lfs, GitHub's `gh`
  (pull requests, issues, runs, creating a repository) and signing commits with your GPG or SSH
  key now work: a question above the message box says exactly what will happen (the commits, the
  branch, the remote's real address, force pushes and the default branch called out) and offers
  Allow, Always for this project or Don't allow. Your keys never enter the sandbox. Repositories whose git settings
  could start a program are refused, and hooks are off. With several GitHub accounts, you pick
  which one each command acts as (a company repository only as a member of that company), and
  "Always" remembers it.
- Websites are asked about: with None (the default), the first time a conversation's commands
  reach a site (curl, pip, npm, cargo, git over HTTPS, …), a question asks Allow (for that
  conversation), Always for this project, or Don't allow; connections made at once share one
  question. Custom lets its listed sites through and asks about the others. Sites allowed always
  are listed under Internet, each removable. The model is told how this works, so it runs the
  command and waits instead of looking for a way around.
- Extensions' questions with a few answers (the gate's and the websites' Allow / Always / Don't
  allow, and other extensions' choices and confirmations) are a see-through pill of one line above
  the message box instead of a dialog in the middle of the window: the question, then ✕ (don't
  allow, Esc), ✓✓ (always) and ✓ Allow. A small button on its top edge shows the details (the
  commits, addresses and warnings); a question with something that needs care opens with them
  shown. ←/→ choose, Enter answers.
- In the sandbox, git always reads its own settings (`~/.gitconfig`, `~/.config/git`), whatever
  the tool folders are, so commits are made as you instead of failing or under a made-up author.
  Sandboxed sessions tell the model up front how git and GitHub work there (run the command; you
  are asked), `gh auth status` is answered by the gate (the accounts and git identity, never a
  token), and a command that ran into the sandbox gets its note even when a chain hid the failure.
- The gate runs `gh repo delete` too: asked every time (never "always"), with the repository named
  in full and a warning that it can't be undone. GitHub needs the `delete_repo` permission for it:
  only logins that have it are offered, and without one the model is told how you can add it
  (`gh auth refresh -h github.com -s delete_repo`) before you are asked for nothing.
- Settings → Sandboxing & Security → Git and GitHub: what the agent may do with your logins
  without asking, each removable.
- Dots at the left edge of the conversation, one per prompt, in Chat and Code. Hovering one shows
  that prompt and the start of its answer; clicking it scrolls there.
- A shield next to the folder in Code sessions shows whether commands are sandboxed; its menu turns
  the sandbox off or on for the project. A command the sandbox blocked says so, with a button to
  allow reading or changing that folder.

### Changed

- A new Code session asks which folder to open (the welcome screen's recent folders, another folder
  or an SSH host) instead of taking the current or most recent one, which could be the home folder.
- Settings: section headings are centred, with a thin line between sections, and a section
  picked in the sidebar has its heading glow for a moment (also with reduced motion). Descriptions next to a switch run up to it instead of wrapping halfway.
- Settings section headings are a size larger.
- The side chat tab on the chat's right edge is an L-shaped arrow; hovering it says what it does.
- Lists on the Sandboxing & Security page scroll smoothly and their border lights up under the
  pointer.

### Fixed

- The models relay no longer sends error details to the other computer: errors could carry a
  stack trace and this computer's paths (code scanning alert 6). A failed connection says only its
  code (ECONNREFUSED, …); the rest goes to the relay's log here.
- A message you send is shown exactly as typed: "log(2.6778)" was drawn as a formula, since the
  escapes that kept it literal looked like math delimiters.

## [0.5.4] - 2026-11-25

### Changed

- Adding a locally hosted model also shows llama.cpp, MLX, vLLM, Jan and Docker Model Runner, with how
  to start them, next to Ollama and LM Studio. SGLang, Lemonade and KoboldCpp are found when they
  run.
- Downloading a speech-to-text model, a model for conversation names or a document parser asks first
  and shows what exactly is downloaded, like SearXNG does. All their buttons say how much it is.
- Settings → pi & models says "Startup model" instead of "Startup model (this computer)".

### Fixed

- The startup model can be any model pi has on this computer. Without an open session only the
  current one could be chosen, and with a session on an SSH host that has models of its own, those
  of the host.

## [0.5.3] - 2026-11-25

### Added

- Settings → pi & models lists the servers and API keys that are connected, and whether the servers
  answer. Clicking one changes or removes it.
- Models can be tested with a short message before they are added.
- Side chats: the "Side chat" tab on the chat's right edge (or `/side`, or ⌘⇧N) opens a second
  conversation next to the one you are in, with everything it holds at that moment: messages,
  thinking, tool calls and their results, also while the agent is still working. A reply still
  being written comes along up to where it was, marked as cut off, and a tool still running
  comes along with its output so far. The original keeps working undisturbed. The side chat
  keeps the model and thinking level, works in Chat and Code (on the same folder) and over SSH,
  and is listed in the sidebar under the conversation it came from. What it took over is folded
  into one card (its first prompt), which opens on a click. (#43)

### Changed

- Adding models is new. It starts with two choices: a locally hosted model, on this device or
  another one (Ollama, LM Studio and other servers are looked for on the device that is chosen, and
  for any other server its address is enough), or a model of an API provider (Anthropic, OpenAI,
  Google, OpenRouter and 19 others need only an API key, which is checked where the provider allows
  it). A server's models are ticked in a list, several at once, with their context and abilities
  filled in as far as the server says. What is connected is listed below the two choices and can be
  changed and removed the same way, and what else is in `models.json` stays.
- The model menu scrolls when it has many models.

### Fixed

- Slider knobs (deep search's effort, the web search settings) have a thin edge and a soft
  shadow, so they no longer disappear against the white background of the light theme.
- Long names in the model menu fade out at the menu's edge, also in the menus for adding models.
  Before, they were drawn past it.

## [0.5.2] - 2026-11-25

### Added

- SSH sessions can use the models of ChatGPT/Codex and GitHub Copilot logins, Google Vertex and
  Amazon Bedrock that this computer has, as they could others. The host needs pi 1.5 or newer.
  (#25)
- Windows: SSH sessions can use this computer's models, and a connected host stays connected. Not
  yet tested on a real Windows computer. (#31)
- Linux: X11 desktops. Both downloads (x86_64 and ARM64) now work on X11 as well as on Wayland,
  which is used where there is one.

### Changed

- SSH sessions: before pi starts on a host, it is checked that this computer is what answers where
  it will send its model requests. On a shared host, someone else could have listened there after
  the connection was lost. Each connection also gets a token of its own, and the last one's no
  longer works.
- The model menu of an SSH session leaves out models that only pi on the host knows about, and that
  this computer could not provide.

### Fixed

- Linux: the window is called Pistachio in title bars and task lists, no longer "Unnamed Window".

## [0.5.1] - 2026-11-25

### Added

- PDF files open in a viewer in the app, as pictures do: click the page above a message or the
  picture of what you attach. It opens at once and draws only the pages you look at. Links work, on
  the web and within the file (with a way back). Pages are light or dark, with photos left as they
  are. The keys scroll and zoom, and so do the trackpad and the buttons.
- Text files that are attached open in a viewer in the app as well, from the page above a message or
  the picture on a card: with line numbers and syntax colors, to select, copy and search (⌘F).
- Chats read many more files, without a download: Word, PowerPoint and Excel (also `.xls` and
  `.xlsb`), OpenDocument (`.odt`, `.odp`, `.ods`) and EPUB, with headings, lists and tables; plain
  text that is not UTF-8 (UTF-16, Windows-1252, Shift JIS and others); and BMP, TIFF and SVG
  pictures. Docling is still used for the Office files it reads, if it is chosen.
- Text files of more than 256 KB are no longer left out of chats: they are shortened, like long
  documents, to their start and the parts that match your message. Their card says so. Settings →
  Document parsing has a slider for how much text goes whole, from 32 KB to all that fits. What the
  model has no room left for is always shortened.

### Changed

- The buttons that download models and document parsers have the download symbol.
- PDF and other files attached to a chat show as small pages above the message, like pictures,
  instead of their text in the message. Clicking one shows or opens it. Conversations are named after
  the message, not the text of the file.
- Pictures attached to a message say what kind of file they are in a corner, like the pages of files.
- What you attach shows in the message box as cards before it is sent: a small picture of the file
  with its extension on it, and its name. While a file is read, a spinner turns on its picture. They
  stay in the order they were attached in.

### Fixed

- The model is no longer told how an attached file was read, which it could take for a tool it does
  not have. pi's notes about pictures it made smaller no longer show in messages.
- The update prompt's progress bar fills as the new version downloads, also when Homebrew downloads
  it. Before, a Homebrew update showed a full, pulsing bar the whole time.

## [0.5.0] - 2026-11-25

### Added

- Deep search has an effort slider next to it in the message box: Light, Standard, Thorough or Max.
  More effort is more searches, pages and results per search, and a stricter check of the report.
  Each chat remembers its own; Settings → Web search has the one new chats start with. Effort is
  also a floor: the question is split into sub-questions, and a model that wants to report before
  it has searched and read enough for them, on enough different sites, is sent back to its research.
- Deep search checks its reports: whether the links work and were read, what the sources say about
  the claims (the key ones in Standard, all of them from Thorough on), and which claims, also in
  tables and bullets, have no source that was read. What it finds goes back to the model once or
  more. A symbol below the report opens how it did: what was searched and read, how many claims are
  backed, and what is still wrong, with the reasons and sources. A check that fails or is stopped
  says so.
- Only deep search's final report is written into the chat as the answer. Until then the chat shows
  the work: what was planned, searched, read and checked, and drafts as rows that open. If it is
  stopped, the latest complete draft is the answer, marked as not checked.
- Deep search reads PDF files.
- Settings → Document parsing chooses what reads PDF files. The built-in reader stays the default;
  PyMuPDF4LLM, Docling and MinerU can be downloaded for better tables, columns, formulas and scans.
  Each shows what it is good at, its size and its license before, is checked against checksums, runs
  without network, and can be removed. If it is missing, fails or takes too long, the built-in
  reader is used and the result says so.
- PDF files can be attached to chats. The chip says “Reading…” until it is done, and a long document
  is shortened to its outline and what matches the message. With Docling, Word, PowerPoint and Excel
  files can be attached too.
- pi reads PDF files as Markdown in Code sessions on this computer.
- PDF files that chats find on the web are read by the chosen parser.
- Long research no longer fills up the context: older pages are cut down to their key passages, and
  nothing is shown to the model twice. The “Working…” line says what it is doing.
- SearXNG can run on this computer, with one click and without Docker (macOS and Linux). Choosing
  it in Settings → Web search checks whether one answers at the address. If not, it shows what is
  missing and asks: Download and run gets SearXNG, a Python of its own and the packages it needs
  (about 80 MB) and starts it, Cancel keeps the service from before. A light shows whether it runs,
  and its “⋮” menu starts, stops, updates and removes it or changes the address. Only this computer
  can reach it. It starts when a chat searches and stops with Pistachio. It does not come with the
  app.

### Changed

- The thinking menu lists only the levels that differ on the model. Ones that its
  `thinkingLevelMap` in `models.json` sends as another level are left out: with minimal sent as
  low and high as xhigh, it has off, low, medium and xhigh. Shift+Tab steps through the same ones.
- Settings → Web search shows only what the chosen service needs: the SearXNG address, the Brave or
  the Tavily API key. The others keep what was entered. Searching the settings for one finds it.
- Settings → Web search has two sections that open with a click on their title: Advanced search
  options (the limits per message, and the text per page, now with sliders) and Advanced deep search
  settings (what each effort level may and has to do, and how its report is checked).

## [0.4.9] - 2026-11-24

### Changed

- The Skills page is an overview of all your skills, also without a session open: the ones that come
  with Pistachio, your own library, and each project's, beside each other, with "+" below them to
  add one. Skills are cards with an icon and color of their own; ones pi only loads when you call
  them say so. Opening or adding one expands beside the list.
- A new skill is for all projects or for one, chosen from a menu of your projects (also without a
  session open). Its instructions start with a template.
- An open skill has Move to trash, Discard and Save centered below it. Save is there when something
  was changed, and opening another skill no longer drops changes that are not saved.
- Text fields are as tall as the search field everywhere, and the focused one has a thin white
  outline (dark in the light theme) instead of a green glow.
- Switches in Settings have an outline and a light knob, and a brighter green when on. In the dark
  theme one that was off looked like it could not be clicked.
- The drifting π signs are behind every Settings page too.
- Settings → pi & models has Add model… and Edit models.json… centered below the text, as tall as
  the fields and with icons. Add connection in the SSH dialog is centered too, with an icon and
  the brighter green.
- A folder name too long for the message box's toolbar fades out instead of being cut off, then
  the machine's does. Pointing at the folder shows its path.

### Fixed

- A skill opened while another window was active came up empty, and Save would have emptied it.
- Clicking a file in a conversation while the Files tab is open shows the file. It closed the
  panel.
- The Skills page lists skills also when no session is open. It said "No skills yet".

## [0.4.8] - 2026-11-24

### Fixed

- Clicking a switch in Settings no longer leaves a green outline on it. The outline shows when the
  keyboard is used.
- Check for Updates shows one notification: the result takes the place of "Checking for updates…".
- macOS: the first connection to an SSH host on the local network goes through once you allow
  local network access. It failed while macOS asked, and had to be turned off and on again. If the
  host still can't be reached, the error says where the setting is, and macOS' question says what
  the access is for.

## [0.4.7] - 2026-11-24

### Added

- The `/` list works from the keyboard: ↑ / ↓ move through it, Tab or Enter takes the highlighted
  command and Esc closes it. Every match is listed (it scrolls), and matching looks inside names
  (`/add` finds `skill:add-skill`). Enter on a half-typed command completes it instead of sending
  it to the model.
- `/login` and `/logout` (optionally with a provider) open pi's sign-in in a terminal, `/reload`
  restarts pi on the conversation, and `/settings` opens Settings.

### Changed

- Settings → SSH connections: each saved connection has a switch that connects it and turns it
  off again, and a "⋮" menu with Rename, Change and Remove. Change edits what "Add SSH
  connection" asks for (host, user, port, key file, jump host) and the command that starts pi.
  A host from `~/.ssh/config` shows the config's values; what you change is saved in Pistachio
  and the file is left as it is. "+" below the list adds one. Turning a connection off ends everything running over it (it asks
  first when pi is working there), also stops one that is still connecting, and nothing in the
  background connects again until you turn it on or reconnect.
- The welcome screen's New session and New chat buttons, and New skill on the Skills page, are a
  quiet outline with a green icon instead of a solid green block; New chat shows its shortcut.
- macOS builds are for Apple Silicon only: the download is now `Pistachio-…-macos-arm64.zip`,
  half the size of the universal one. Intel Macs stay on 0.4.6, the last universal release:
  Homebrew won't install newer versions on them, and Update in 0.4.6 and earlier finds no
  universal download, so it says so and keeps the version they have. On Apple Silicon,
  Homebrew installs update as before; a copy installed from the zip updates to 0.4.7 once by
  hand (Update in 0.4.6 looks for the universal zip), and with Update from then on.

### Fixed

- API keys, cloud settings, proxies and other variables exported in your shell's startup files
  reach pi when Pistachio is started from the Dock or a desktop; only `PATH` and a few others did.
  A slow shell no longer freezes the window, and one that leaves a background process running no
  longer hangs it. If the shell can't be read, pi is still looked for in the usual folders.
- The SSH password prompt is ready to type in, and Enter sends the password; it cancelled instead.
  Enter in Rename, Add model, the SSH dialogs and an extension's text question does what the
  button does, instead of closing the dialog and losing what you typed. Extension choices, /resume
  and /fork work with the arrow keys and Enter.
- An extension's yes/no question is no longer answered "yes" by an Enter typed for the message box:
  it takes a click, or Enter on the button itself.
- A message you have not sent (and its attachments) is kept when you open another session, start
  a new one, switch between Chat and Code, restart pi, reconnect, or save a model or skill, until
  the app quits. A new session nothing was sent in comes back too, and choosing another folder or
  machine takes the message along.
- A message pi refuses (no API key, say) goes back to the message box with its images, followed by
  the messages waiting behind it. It was lost.
- `!commands` show their output as it comes, can be stopped with Esc or the stop button, and keep
  running when you switch chats.
- `/compact` shows that it is working, can be cancelled with Esc or the stop button, keeps going
  when you switch chats, and holds messages sent meanwhile.
- Esc and Alt+↑ give queued messages back with their images.
- `/model` takes an exact name over one that merely contains it and lists the candidates when
  several match; `/thinking` accepts only the levels the model offers.
- Renaming a session no longer opens it.
- One value in `settings.json` that can't be read no longer resets every setting: the others are
  kept, and so is a copy of the file.
- Reconnect starts pi again in an SSH session whose connection ended; it connected to the host
  but left the stopped session as it was.

## [0.4.6] - 2026-11-23

### Added

- A built-in skill, `add-skill`, that teaches pi how to add skills: where each kind goes, the
  front matter rules, writing a description pi can route on, supporting files, and checking
  that pi loads it. Ask pi to "make a skill for …" or call `/skill:add-skill`. Code sessions
  on this computer load it alongside your own skills (it is written to
  `~/.pi/agent/pistachio/skills` and passed with `--skill`; pi's own skill folders are left
  alone). The Skills page lists it under "Comes with Pistachio", read-only.
- Formulas in answers are drawn as math: `$…$` and `\(…\)` inline, `$$…$$` and `\[…\]`
  blocks centered on their own lines. KaTeX lays them out (a Rust port with KaTeX's fonts,
  so they look as they do on the web), sharp at any size and in the text color of either
  theme. A formula KaTeX can't read shows as written, and copying a formula gives its LaTeX.

### Fixed

- Naming a chat that starts with a question no longer answers it: Qwen2.5 0.5B named "what is
  current market cap of nvidia" "Nvidia's current market cap is $175 billion". The local
  model now sees a question among its examples (it names it "Nvidia current market cap"), the
  instructions say a question gets its topic as the title, and a title that states a number
  the message never mentions is rejected as an answer, so the chat keeps its first message.
- Dollar amounts no longer break bold text in answers: "**$4.0 trillion** — about **$4.02T**"
  rendered as `$4.0 trillion** — about **$4.02T`, because the text between two dollar signs
  was read as a formula. Dollar signs now follow pandoc's rules: a formula's closing `$` is
  never followed by a digit, so prices ("$5 and $10") stay text, and `$` in code is left
  alone.

## [0.4.5] - 2026-11-23

### Added

- Update from the app: when a new version is out, Update in the prompt installs it the way
  Pistachio was installed and opens the new version, with no download to unpack. On macOS
  that is `brew upgrade --cask pistachio` for a Homebrew install, otherwise the new app
  replaces the old one; on Linux the .deb or .rpm (the system asks for your password) or the
  binary the tarball installed; on Windows `pistachio.exe` is replaced. The download is
  checked against GitHub's checksum, and if anything fails the old version stays and the
  prompt says what happened. The prompt now sits in the middle of the window, over the window
  blurred (a dimmed veil on Linux and Windows).
- What's new: after an update, Pistachio shows what changed since the version you had, from
  this changelog.

### Fixed

- This computer's models on SSH hosts (#26):
  - The relay sends a request to the address in this computer's model settings, never to
    one taken from the request (only the path comes from it). The address was already
    checked against those settings; this also clears a code-scanning alert.
  - An answer that is not a stream (an error, a list of models) no longer carries this
    computer's key or provider headers back to the host, even from a server that writes the
    credentials it got into its answer.
  - When the SSH connection is made again in the moment before a session starts, pi on the
    host no longer starts without a way to reach this computer's models: the session stops
    with a message and offers to reconnect.
  - Relative paths in the host's own pi settings (`"extensions": ["./my-ext"]`, local
    packages) still point into the host's agent folder.

## [0.4.4] - 2026-11-23

### Added

- SSH sessions use this computer's models on any host, with no setup: the same model list,
  default model and thinking level. Model requests come back over the SSH connection and go
  out from this computer, so models on `localhost`, the LAN or a VPN work from any host, and
  keys and logins never leave this computer (each host gets a token that works only through
  its own connection). pi on the host uses a settings folder Pistachio manages,
  `~/.pistachio/agent`; the host's own `~/.pi` is left alone and its extensions and skills
  still load. ChatGPT/Codex, GitHub Copilot and Gemini CLI logins, Vertex and Bedrock are not
  relayed yet. The "Copy model settings" offer in the connect dialog and Manage SSH
  connections is gone.
- File names in answers are links (Code sessions): a file of the session's folder that an
  answer mentions (`fib.rs`, src/main.rs:12) opens in the Files tab, at its line, and so does
  the file of a Read, Edited or Wrote row. Only the chat's display changes: what pi and the
  model see, copying and the session file keep the answer as it was. A name is linked only
  when it points at exactly one file.

### Changed

- The default text size is larger (the old Large); the picker gains a Larger step and your
  current size is kept.
- Faster: the window appears sooner (about 235 ms instead of 450 ms in the benchmark; it no
  longer waits for the login shell's environment), and long conversations no longer lay out
  every message in one frame when opened or when the chat column changes width, so opening a
  conversation and toggling the sidebar or side panel take about half as long.
- A window that is not focused redraws at up to 60 fps instead of 30, so hovering it reacts
  sooner.

### Fixed

- The "Working…" label no longer shifts sideways while its shimmer passes over it.
- Hover highlights, clicks and scrolling show up sooner on macOS: about 35 ms from moving the
  mouse to the change on screen, down from about 63 ms on a 75 Hz display. GPUI waited for the
  next display refresh before drawing pointer input and then queued the frame three refreshes
  ahead; Pistachio now carries a patched copy of GPUI's macOS backend (see vendor/README.md).
- Deleting a session no longer freezes the window while Finder moves the file to the Trash:
  the row goes at once.
- Settings… and Check for Updates… in the app menu (and shortcuts like ⌘N) work right after
  launch instead of staying greyed out until something in the window was clicked.
- A turn's copy and branch buttons show while the pointer is anywhere in its answer, not only
  over the "Worked for" row.
- Hovering the open session in the sidebar no longer shows its rename and trash buttons on a
  darker patch: they take the hovered row's colour.
- Dictation keeps everything you said: the greyed-out live text no longer disappears when a
  re-decode comes back empty, and a phrase whose last decode is empty keeps the text that was
  shown instead of dropping it. Soft speech (a laptop's microphone at arm's length) is no
  longer cut into fragments, lost between phrases or ended by the silence timeout while you
  talk: pauses are found by a voice detector built into the app (Silero VAD) instead of by
  loudness.

## [0.4.3] - 2026-11-23

### Added

- Automatic conversation names (Settings → Conversation names, off by default): a new
  conversation is named after its first message in a few words, instead of the message's
  first line, and the sidebar types the name out behind a caret. The name is made either by
  the conversation's own model, through a separate `pi --print --no-session` call that leaves
  the conversation's session and context untouched (on the SSH host for remote
  conversations), or by a small model on this computer (Qwen2.5 0.5B, SmolLM2 360M or
  Qwen2.5 1.5B, downloaded on request, checked against pinned SHA-256 digests and run with
  candle). The first message is sent right away; names are saved through pi, so the
  terminal shows them too, and names you set yourself are never replaced.
- Pictures in the chat: when the agent reads an image (a screenshot, say), a small thumbnail
  shows to the right of the call, and an "Explored" group shows the first two with "+N".
  Pictures attached to a prompt show above it. Clicking one opens a full-window viewer; ← / →
  or the arrows browse all the pictures of that call, group or prompt, and Esc closes it.

### Changed

- Switching to a session is near-instant: its history is read straight from the session
  file (in about 60 ms for a long session) and shown while pi starts, instead of after pi
  has started and sent it (around a second). pi's own history replaces it only if it
  differs; sessions with compactions or context edits, and sessions on SSH hosts, still wait
  for pi.
- The working indicator is a little green π with dots orbiting it, in the chat and in the
  sidebar, where it now sits left of the conversation's name. Settings → Appearance →
  Working indicator picks the style, with a live preview: by default two dots chase round the
  π at a spinner's uneven speed while it glows and a shimmer crosses "Working…"; the others
  are a comet tail, a spinning π, a single dot, an atom ring, half-turn hops and a pulse. "Working… 12s · esc to interrupt" is centred, and "Worked for" has a
  small π.
- Tool calls start with a small icon for what they did (a document for reads, a pencil for
  changes, a terminal for commands, a magnifier for searches, a folder for listings, a globe
  for the web), instead of a green dot. It is muted when the call is done, red when it failed,
  breathing in the accent colour while it runs and faded when interrupted, so green is left
  for the connection and running work.
- The model's text stands apart from its activity: runs of tool calls and thinking sit on a
  thin rail on the left, in a slightly smaller, muted style, with more room where they meet
  the text. An image read says "Read an image" instead of "Read 1 line".

- Tool calls in the chat read in the past tense like the rest: "Wrote", "Edited", "Listed" and
  "Searched" instead of "Write", "Edit", "List" and "Search".

### Fixed

- A conversation name is never a code block's language ("rust"): when a small naming model
  answers with code instead of a title, the conversation keeps its first-line title.
- Opening a long conversation, the scrollbar now spans all of it from the start, instead of
  starting too big and shrinking and jumping as you scroll up.
- Clicking the conversation already behind the Settings or Skills page (or the Chat / Code
  mode already in use) did nothing; it now closes the page and shows the conversation.

## [0.4.2] - 2026-11-22

### Added

- Speech to text: dictate with the microphone button in the message box. The text appears
  greyed out while you speak and becomes normal text when you stop. A model runs on your
  computer (Parakeet v3, Whisper large-v3 turbo, Whisper small or Moonshine v2); none ship
  with the app. The first press offers to download one, with its size, speed and languages;
  it downloads in the background with a progress bar at the top of the chat, and is checked
  against its SHA-256. Settings → Speech to text manages the models, when a silence ends
  dictation and the language for Whisper.
- Dictation shortcut: ⌘⇧Space (Ctrl+Shift+Space on Linux/Windows) in the message box. Hold
  it to talk and let go to keep the text, or tap it to start and tap again to stop. Without a
  model it offers the download, like the microphone button.
- Find in chat (⌘F, also Edit → Find in Chat): highlights every match in the conversation,
  with Enter / Shift+Enter (or ⌘G / ⌘⇧G) for the next / previous one and a "3 of 12" count;
  Esc closes it. Thinking, tool output and edits are searched too, and a collapsed row opens
  when it holds the current match.
- File viewer: clicking a file in the Files tab opens it read-only in the panel, with syntax
  colors and line numbers, also for folders on an SSH host. "Mention" in the viewer, the +
  button on a row, or ⌥-click in the tree still put an `@path` mention in the message box.
  Binary files show a notice; files over 512 KB open with their first 512 KB.
- Settings → Appearance → Code colors: pick the syntax colors for code (Pistachio, One,
  GitHub, Solarized, Gruvbox or Catppuccin), each with a light and a dark variant. A Python
  snippet below the picker shows the chosen palette right away. Only code colors change; the
  rest of the app keeps its theme.

### Changed

- Clicking a file in the Files tab opens it instead of mentioning it (⌥-click mentions it).
- The models.json editor colors JSON: keys, strings, numbers, `true`/`false` and `null` each
  get their own color.
- The skill editor colors Markdown: headings, bold and italic, code, links and list markers.
- Code blocks in chat are colored for Python, Rust, JavaScript, TypeScript (and TSX), C, C++,
  C#, Go, Java, HTML, CSS, Bash, YAML, TOML, JSON, Markdown and diffs. Other languages
  still get the labeled box with a copy button, in one color.
- The agent's file edits in the chat, the files it reads, and the diffs in the Changes panel
  are colored the same way (by the file's extension), on top of the red and green
  added/removed backgrounds. Each side of a diff is colored as a whole, so a string or comment
  spanning several lines stays colored. They follow the Code colors setting.

### Fixed

- New session / New chat (and ⌘N) did nothing while the Skills or Settings page was open over a
  conversation that was still empty; it now closes the page and shows that conversation.
- Esc in the message box now interrupts the agent: the text box kept the key to itself.
- Dragging the right panel wide no longer squeezes the chat until its header buttons and the
  message box controls overlap: the chat keeps at least 400 px, the title truncates, and in a
  narrow column the left side of the message box toolbar is clipped while the model picker,
  thinking level and send / stop button stay visible.

## [0.4.1] - 2026-11-22

### Added

- Linux builds for ARM64 (aarch64), and `.deb` and `.rpm` packages for x86_64 and ARM64.
- Install on macOS with Homebrew: `brew install --cask seba2390/tap/pistachio`.

### Fixed

- "Check for Updates…" on Linux offers the download for your architecture.

## [0.4.0] - 2026-11-22

### Added

- New chats and Code sessions show the π field behind their empty screen; it fades out when
  the first message is sent.
- A demo video in the README, recorded with the new scripted demo mode
  (`PISTACHIO_DEMO=<storyboard.toml>`; re-record with `script/demo/record-demo.sh`).

### Fixed

- Debug builds no longer panic when a run of exploration steps ("Explored · N steps") appears
  in the chat.

## [0.3.0] - 2026-11-22

### Added

- **Settings page** (⌘, or the button at the bottom of the sidebar): theme, text size, start
  mode, reopening the last session, background notifications, which `pi` to run, pi's startup
  model and thinking level, web search service, keys and limits, and SSH connections.
- **Check for Updates…** in the Pistachio menu and in Settings, plus an optional check when the
  app opens. A newer release is offered (what's new, download in the browser, skip this
  version); nothing is downloaded without a yes.

### Changed

- The last session is reopened at start only with the new "Reopen the last session" setting
  (before, an SSH session came back on its own).
- Saved SSH connections connect at start only with the new "Connect saved connections at
  start" setting (off by default; before, they always did).
- The sidebar's search field matches the Settings page's (search icon, regular size), and
  switches in Settings show a hover effect.
- Web search's `config.json` is written atomically and readable only by you.

### Fixed

- The Windows release packaging (license notices with non-ASCII names), so 0.2.0's changes
  ship in this release.

## [0.2.0] - 2026-11-22

Tagged but not published as downloads (the Windows packaging failed); its changes are in
0.3.0.

### Changed

- **PI-gui is now Pistachio.** The app, binary (`pistachio`), macOS bundle (`Pistachio.app`,
  `dev.pistachio.app`), Linux launcher and repository (`seba2390/pistachio`) are renamed.
  - Settings move on first start from `~/.pi/agent/pi-gui/` to `~/.pi/agent/pistachio/`.
    Earlier chats stay where they are and still show up under Chat.
  - On SSH hosts pi is now installed into `~/.pistachio/server`; an existing
    `~/.pi-gui/server` is still used until then.
  - Environment variables are now `PISTACHIO_*` (for example `PISTACHIO_PI_BIN`,
    `PISTACHIO_OPEN`).

### Added

- Windows build (experimental): a `pi-gui.exe` zip in each release. pi installed with npm
  (`pi.cmd`) is found on `PATH`. CI builds and lints Windows; SSH remotes are untested there.

### Changed

- CodeQL runs from a workflow on `main` and weekly instead of on pull requests.

## [0.1.0] - 2026-11-22

The first public release: chat and code sessions over pi's RPC mode, with streaming
transcripts, git changes and file panels, an integrated terminal, SSH remotes, web search for
chats, skills and model management. Changes since the last private build:

### Added

- Labelled bash calls show only their label; click to see the command and output.
- Nested tool activity (subagents, code mode) shows inside its parent tool call.
- Composer: ↑ / ↓ prompt history, inline `@file` completion, and a "Waiting to send" list with
  cancel buttons.
- **Restart pi** for local sessions whose pi stopped.
- Accessibility: names and roles for icon buttons, sidebar rows, the Chat | Code switch,
  expandable transcript rows and the message box.
- Window menu (Minimize, Zoom), ⌘W and ⌘M. Closing the window hides the app (macOS), and the
  Dock icon brings it back.
- Terminal: each folder keeps its own shell. IME composition is drawn. On macOS, Option-arrows
  move by word and ⌘-arrows / ⌘-Backspace edit the line.

### Fixed

**Restarts and background chats**

- Restarts after saving `models.json` or a skill now apply to the chat that asked for them.
  They also wait for prompts that are still being sent.
- Closing the Skills page no longer keeps its pi process running.
- Prompts typed while pi wasn't ready are no longer lost silently.
- Esc and Alt+↑ merge restored messages into your draft instead of replacing it.
- Retry and Edit keep the prompt's images.
- A background chat whose pi crashed is reported as stopped, not "done".
- Questions held for a background chat are dropped once pi has timed them out.
- `/clone` waits for the current response.
- Startup notices survive resuming a session.
- A pi that hangs on exit is stopped together with the processes its tools started.

**SSH and security**

- The host key you confirm is exactly the key that gets saved: the connection is pinned to it.
  "Trust" is no longer offered when the key couldn't be fetched.
- The SSH connection and password-prompt folders are refused unless they are private to you.
- Saving `models.json` keeps its file permissions.
- The remote probe can no longer mistake login-shell output for pi's folders.
- Host names starting with `-` are rejected.

**Performance and saved state**

- Per-file diffs are capped at 2 MB, and line counts of untracked files at 500 files.
- Saves of UI state and SSH hosts can no longer land out of order.
- A corrupt `hosts.json` is kept as a backup.
- A remembered terminal height can no longer squeeze out the chat.

[Unreleased]: https://github.com/seba2390/pistachio/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/seba2390/pistachio/compare/v0.5.10...v0.6.0
[0.5.10]: https://github.com/seba2390/pistachio/compare/v0.5.9...v0.5.10
[0.5.9]: https://github.com/seba2390/pistachio/compare/v0.5.8...v0.5.9
[0.5.8]: https://github.com/seba2390/pistachio/compare/v0.5.7...v0.5.8
[0.5.7]: https://github.com/seba2390/pistachio/compare/v0.5.6...v0.5.7
[0.5.6]: https://github.com/seba2390/pistachio/compare/v0.5.5...v0.5.6
[0.5.5]: https://github.com/seba2390/pistachio/compare/v0.5.4...v0.5.5
[0.5.4]: https://github.com/seba2390/pistachio/compare/v0.5.3...v0.5.4
[0.5.3]: https://github.com/seba2390/pistachio/compare/v0.5.2...v0.5.3
[0.5.2]: https://github.com/seba2390/pistachio/compare/v0.5.1...v0.5.2
[0.5.1]: https://github.com/seba2390/pistachio/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/seba2390/pistachio/compare/v0.4.9...v0.5.0
[0.4.9]: https://github.com/seba2390/pistachio/compare/v0.4.8...v0.4.9
[0.4.8]: https://github.com/seba2390/pistachio/compare/v0.4.7...v0.4.8
[0.4.7]: https://github.com/seba2390/pistachio/compare/v0.4.6...v0.4.7
[0.4.6]: https://github.com/seba2390/pistachio/compare/v0.4.5...v0.4.6
[0.4.5]: https://github.com/seba2390/pistachio/compare/v0.4.4...v0.4.5
[0.4.4]: https://github.com/seba2390/pistachio/compare/v0.4.3...v0.4.4
[0.4.3]: https://github.com/seba2390/pistachio/compare/v0.4.2...v0.4.3
[0.4.2]: https://github.com/seba2390/pistachio/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/seba2390/pistachio/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/seba2390/pistachio/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/seba2390/pistachio/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/seba2390/pistachio/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/seba2390/pistachio/releases/tag/v0.1.0
