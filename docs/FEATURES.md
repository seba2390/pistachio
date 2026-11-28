# Features

Everything Pistachio does, in detail. For installation see the [README](../README.md).

- Chat in the centre: streaming markdown, collapsible thinking (“Thought for 4s”), tool calls with
  results and inline edit diffs, and “Worked for 1m 3s” after every turn.
- Bash calls read as what they do (“Read Cargo.toml, src/main.rs”, “Searched “fn main” in src”,
  “Ran cargo test”), worked out by the app from the command itself; the model writes nothing
  extra. Runs of calls that only look around (reading, listing, searching, checking) fold into
  one “Explored · N steps” row; expanding it shows every call. Anything not fully understood
  (loops, `$(…)`, `xargs`, unknown programs, flags that could change files such as
  `find -delete` or `sed -i`) is shown as the raw command, so a label never hides a change. A
  labelled call shows only its label; click it to see the command and its output. To check
  the labeller against real
  transcripts: `cargo run --release -p transcript --example label_corpus -- <session.jsonl…>`.
- Tools that run other tools (subagents, code mode) show that work inside their own row: a
  live step count while running, and every nested call and reply when expanded.
- Composer: attachments (cards with a small picture of the file, its extension and its name; a
  spinner turns on those still being read), working-folder picker (a long name fades out), model and
  thinking pickers (only the thinking levels that differ on the model), context-window wheel, `/`
  command completion, inline `@file` completion (paths with spaces are quoted like pi does), ↑ / ↓
  through earlier prompts, and `!cmd` shell commands (with live output; Esc stops them). Messages
  typed while pi starts, compacts or is busy sending are listed above the input as “Waiting to
  send”, each with a cancel button. A message you have not sent stays with its session while the app
  runs, also when you open another one.
- Opening a conversation shows it at once, without waiting for pi to start (a few hundred
  milliseconds): its history is read from the session file following pi's rules (compactions,
  extension messages such as web search's, context edits), and the composer shows the model,
  thinking level, web search mode and context use pi had for it last time (remembered in
  `~/.pi/agent/pistachio/recall.json`) or, for one pi never opened here, what its file
  selected. pi's own answer replaces anything that differs.
- If pi stops (a crash or a failed start), the chat offers **Restart pi** on the same session;
  anything not yet sent, or refused by pi, goes back into the message box.
- pi gets your login shell's environment (API keys, proxies, cloud settings), also when the app is
  started from the Dock or a desktop. Programs can tell by `PISTACHIO_RESOLVING_ENVIRONMENT=1`
  that the shell is only asked for it.
- Left sidebar with a **Chat | Code** switch (⌘1 / ⌘2):
  - **Code**: the coding agent in a project folder; sessions are grouped under their folder
    (collapsible, "+" for a new session there), including ones started from the terminal. A new
    session (⌘N) asks which folder (recent folders, another folder or an SSH host); it never picks
    one by itself.
  - **Chat**: plain conversation with **no file or shell tools**: the model cannot read, write
    or run anything on your computer. Chats run pi in a dedicated empty folder
    (`~/.pi/agent/pistachio/chat`) with only deep search's two web tools, which stay off until
    you turn web search on, and `python` (`--tools web_search,web_fetch,python --no-mcp`). The
    text of attached files goes along with the message instead of a path (of PDF files as Markdown,
    see [Documents](#documents)), and shows as a small page above it; `!` shell commands are
    disabled.
  - **Python** (Chat): the model works numbers out in Python instead of estimating them (a sum,
    a logarithm, a series, statistics), and uses it for other small jobs a program does more
    reliably than reasoning: counting letters or words, sorting and reformatting text, regular
    expressions, JSON, dates and weekdays, number bases, or trying a small algorithm on examples. The code runs in [Monty](https://github.com/pydantic/monty),
    a Python interpreter written in Rust for code a model writes, started fresh for each call
    (in milliseconds): it has no files, network, environment or programs, only the computation,
    with `math`, `decimal`, `statistics` and more of the standard library, and stops after 5
    seconds, at one allocation over 256 MB or at deep recursion. The chat shows the call as
    "Computed" with the result; expanded, the code. Settings → General → Chat mode turns it off
    (also in open chats, from their next message). By hand: `pistachio --python-tool 'code'`.
  - **Web search** (Chat): the globe button next to the paperclip chooses how the model uses
    the web, per chat (remembered in the session; `/websearch off|search|deep` works too):
    - **Search**: a quick lookup when needed (a few searches and pages), then a concise answer
      with links.
    - **Deep search**: a thorough investigation: the model plans sub-questions, searches from
      several angles, reads many pages (also PDFs), cross-checks key facts and reports back with a
      structured, sourced report. A slider next to it sets the effort (`/websearch effort
      light|standard|thorough|max` too): from 10 searches and pages to 160, with more results per
      search. Effort is a soft floor too. The question is split into sub-questions (a simple one
      is only one), and for each of them the effort level asks for a number of searches and pages,
      and for different sites. A model that wants to report before that, or leaves a sub-question
      open, is sent back to its research with what is missing: until it is done, the budget is used
      up, nothing new turns up, the search service no longer answers, or it was sent back a set number of times (once in Light, up to 8
      times in Max). With APIs like OpenAI's it has to call a tool while its research is short, so
      it writes no reports that would only be sent back. Settings → Web search → Advanced deep
      search settings has all of these numbers.

      Then the report is checked: links are tried, and from Standard on the model judges, from the
      sources' own passages and without the conversation, whether they back its claims (the key
      ones; all of them from Thorough on; one by one in Max). Quotes it gives as proof are looked up
      on the page. Table rows and bullets are claims too, whether they cite with links, page names in
      brackets, numbers or a line of sources below them. Ones without a source are problems, and a
      page that was not read is no source. The report goes back to be corrected with what was found.
      These questions to the model have time and token limits (`check` in `config.json`); a check
      that fails or is stopped ends the run with a note that says so.

      Until the report is final it is not shown as an answer. The chat shows the work instead:
      searches, pages, the plan, the checks, and drafts as rows that open. The “Working…” line says
      what is going on (“Checking the report: 12 of 40 claims”). Then the final report is written
      into the chat once. A symbol at the end of the turn opens how it was checked. If the check fails or you press Esc,
      the latest complete draft is the answer, marked as not checked. The session keeps every
      draft, so other programs (pi's terminal) may show them.

      Long research stays within the context: older pages are cut down to the passages that
      mattered, replaced drafts are left out, and results and passages the model has seen are not
      shown again.

    The model searches (`web_search`) and reads pages (`web_fetch`); the work is done in Rust
    by the app itself, and a small embedded pi extension only connects the tools to pi. Pages
    are read without JavaScript, reduced to their main text, and long pages are cut down to
    the passages that match what the model is looking for. Only public addresses are fetched
    (no localhost or private networks), and each message has a limit on searches and page
    reads. Settings live in `~/.pi/agent/pistachio/deepsearch/` and apply to the next message,
    no restart:
    - `config.json`: `backend` (`duckduckgo` by default, no key; `searxng` with
      `searxng.url`, e.g. a self-hosted instance with `json` enabled in its `settings.yml`, or with
      `searxng.local` the one Pistachio runs on this computer;
      `brave` or `tavily` with an API key or `api_key_env`), `default_mode` for new chats,
      `default_effort`, `modes.search` limits (`max_searches`, `max_fetches` per message), `deep`
      (what each effort level may and has to do: searches, pages, results, checked claims, revisions,
      sub-questions, sites),
      results per search, page text per read (`fetch.max_chars`; lower it for small context
      windows), `fetch.allow_private_networks` and timeouts. Settings added in later versions
      are filled in automatically.
    - `prompt-search.md` and `prompt-deep.md`: each mode's instructions (`{{date}}`,
      `{{max_searches}}`, `{{max_fetches}}` and the effort's `{{depth}}` are filled in). Delete a file to get the
      default back.
    - **SearXNG on this computer** (macOS and Linux, no Docker): choosing SearXNG in Settings
      checks whether one answers at the address. If not, it shows what is missing and asks whether
      to download and run it; Cancel keeps the service from before. Its row has a light (green
      running, amber busy, red stopped or not usable) and a “⋮” menu: start or stop, update, change
      the address, remove. It does not come with the app: on request SearXNG, a Python of its own
      and the packages it needs (about 80 MB, 190 MB on disk), each pinned and checked against its
      checksum, go to the `searxng` folder here; your own Python is not touched, and Remove deletes
      all of it. Only this computer can reach it, on a free port. It starts with the first message
      of a chat that searches and stops when the app quits (also if it crashes) or another service
      is chosen. Its settings are `searxng/settings.yml`, yours to edit.
    - Try settings without the GUI: `pistachio --deepsearch-tool search <query>` and
      `pistachio --deepsearch-tool fetch <url> [what to look for]` print what the model would
      see.
  - Search, rename and move to trash in both modes.
  - **Background work**: switching away from a chat that is working (or still has messages
    waiting to be sent) keeps it running. Its row shows a spinner; when it finishes it gets a
    green dot until you open it, a toast says so, and the Dock icon bounces if the app is not
    in front. Questions from extensions (confirm, select, input) in a background chat wait
    until you open that chat, marked with an amber dot, so they are never answered for the
    wrong chat; its toasts name the chat.
- **Side chats**: the tab with an L-shaped arrow on the chat column's right edge (also `/side` and ⌘⇧N)
  opens a second conversation in a column to the right, holding everything the conversation
  has at that moment, also while its agent works: finished messages and tool results, the reply
  being written (up to where it was, marked as cut off), and tools still running (with their
  output so far). The original keeps going undisturbed. The side chat is a session of its own
  (its file is written next to the original's, with pi's `parentSession` pointing at it), with
  the same model, thinking level, folder and machine; in Code mode both agents work on the same
  folder. What it took over is folded into one card showing the first prompt and how many
  replies there are; a click shows it all (and folds it again). In the sidebar it is listed
  under its conversation, indented, with a line curving down from it. Its top bar moves it to the main column or closes it (it stays in the sidebar, and
  keeps working in the background if it is busy). `/side` in a side chat opens a side chat of
  that one in its place.
- **Branches**: hovering a prompt offers Copy and Edit; hovering the end of a turn offers
  Copy reply, Retry (newest turn) and Branch from here. Edit, Retry and branching continue in
  a new session (the original stays in the sidebar), like pi's `/fork`. Slash commands:
  `/fork` (pick a message), `/clone`, `/side` (a side chat, above), `/resume` (pick an earlier
  session here), `/export [path]` (HTML) and `/share` (a secret GitHub gist via `gh`, like pi's
  terminal).
- **SSH** (Code mode): the device button next to the folder ("This computer", with a status
  light) runs pi on another machine. "Add SSH connection…" lists the hosts in your
  `~/.ssh/config` (following `Include`), or takes a custom host, user, port, key and jump host;
  connections are saved in `~/.pi/agent/pistachio/hosts.json` and "Manage SSH connections…"
  renames them, changes the command that starts pi and removes them. In Settings → SSH
  connections a switch connects each one or turns it off, and its "⋮" also changes the host,
  user, port, key file and jump host (kept in Pistachio; `~/.ssh/config` is not written). pi runs **on the
  remote** (its tools, `AGENTS.md`, skills and sessions all live there)
  through the system `ssh`, so your config, keys, ssh-agent and `known_hosts` apply.
  - One shared connection per host (ControlMaster) makes later commands ~30 ms. With
    "Connect saved connections at start" (Settings → SSH connections, off by default) saved
    hosts connect quietly in the background when the app opens. The dot shows green (ready),
    amber (working or needs you), red (failed) or grey (not connected).
  - **pi is installed automatically** when a host does not have it, like an editor's remote
    server: into `~/.pistachio/server` on the host, with no root access, using the host's
    Node.js 22.19+ or a downloaded, checksum-verified standalone Node.js. Remove that folder
    to undo it.
  - **This computer's models work on every host**, with nothing to set up: the model list,
    default model and thinking level are the ones you have here, and model requests come back
    over the SSH connection and go out from this computer. So models on `localhost`, your LAN
    or a VPN work from any host, and keys and logins never leave this computer. A hidden pi
    here (started on first use) adds them to each request; the host reaches it through a
    port forward on its own loopback, and each request needs a token that only works for
    this connection to that host. Before pi starts there, the hidden pi proves that it is the
    one that answers. Logins and cloud credentials that take more than a key (ChatGPT/Codex,
    GitHub Copilot, Vertex, Bedrock) work too, with pi 1.5 or newer on the host: the hidden pi
    sends those conversations itself. (Custom thinking budgets and sampling parameters are not
    passed on for these.) pi on the host reads a settings folder Pistachio writes there,
    `~/.pistachio/agent` (written again when your models, logins or settings here change);
    the host's own `~/.pi` is not changed, and its extensions, skills, prompts and themes are
    linked in. If a host does not allow port forwarding, pi there uses its own models and
    the model menu says why. Adding models in a remote session edits this computer's models.
  - Passwords, key passphrases and one-time codes are asked for in the app (ssh's
    `SSH_ASKPASS`); nothing is stored. Unknown host keys are shown for confirmation; a changed
    host key is explained, and you can remove the old key and confirm the new one.
  - macOS asks once whether Pistachio may reach devices on your local network, and blocks the
    connection meanwhile. It is tried again for about 15 s, so it goes through once you allow it;
    otherwise the error says where the setting is.
  - The folder browser takes a typed or pasted path, creates folders and can show hidden ones.
  - Remote sessions behave like local ones: they are listed in the sidebar under the host
    (then folder), can be opened, renamed and moved to the host's trash, and with "Reopen the
    last session" the last one comes back after a restart. The Diff and Files tabs read git and the folder over SSH
    (refreshed every 10 s while shown and after each turn), the terminal is your shell on the
    host in the session's folder, and the attach picker indexes the remote folder.
  - When the connection drops, the session offers to reconnect and resumes its session file.
  - Limits: csh/tcsh login shells are not supported (bash, zsh, dash, ksh and fish are; fish
    is untested on a real host). Windows clients are untested: Windows' OpenSSH cannot share
    connections, so every command opens its own connection there (keys or an agent are needed,
    as a password would be asked for each time), and a tunnel in the background tells whether
    the host is connected and forwards to the hidden pi, which listens on a loopback port.
- **Skills** (Code mode): a Skills button under the Chat | Code switch shows all skills: the
  ones that come with Pistachio, your own library (`~/.pi/agent/skills` and `~/.agents/skills`)
  and those of your projects (`.pi/skills` and `.agents/skills`), here or on the SSH host. With a
  session open, what pi itself reports for it counts.
  "+" below them writes a `SKILL.md` (name checked against pi's rules, description, instructions
  from a template, optionally "only when I call it" via `disable-model-invocation`) into your
  personal skills or the `.pi/skills` of a project you choose; existing skills open in an editor,
  with the toggle and move to trash. pi loads project resources only for trusted folders, and in RPC mode it cannot ask, so
  the page shows whether the project is trusted and can trust it, saved in pi's own
  `trust.json` like the terminal's `/trust`. pi reads skills when it starts, so changes restart
  it on the same session.
- **Formulas**: answers draw LaTeX math with KaTeX's layout and fonts: `$…$` and `\(…\)`
  inline, `$$…$$` and `\[…\]` as centered blocks (scrolling sideways when wider than the
  chat). Dollar signs follow pandoc's rules, so prices such as "$5 and $10" stay text, and
  code keeps its `$`. A formula that doesn't parse shows as written; copying gives its LaTeX.
- **Built-in skills**: Code sessions on this computer also load the skills that come with
  Pistachio, written to `~/.pi/agent/pistachio/skills` and passed with `--skill` (pi's own
  skill folders are not touched). They show read-only on the Skills page under "Comes with
  Pistachio", also before a session is open. So far there is one, `add-skill`, which walks pi
  through adding a new skill.
- **Adding models**: "Add or change models…" in the model menu (or Settings → pi & models) starts
  with two choices. **Locally hosted model** asks whether it runs on this device or another one;
  nothing is assumed to run anywhere. On the chosen device Ollama, LM Studio, llama.cpp, MLX, vLLM,
  Jan, Docker Model Runner, SGLang, Lemonade and KoboldCpp are looked for on their usual ports
  (another device by its address, like `192.168.1.89`), and
  any server is connected by its port or address (`192.168.1.89:8888`): which API it speaks
  (OpenAI's, Anthropic's or Gemini's) is found out. On this device (pi running here) it asks
  first whether to **attach to an existing model** (the servers above) or **deploy a new model**:
  **Scan local hardware** lists the processor, memory, graphics, free disk and build tools it
  found, and **Suggest models** then offers up to four for them (**Tiny**, the most capable model of
  at most 3B parameters in its lightest build, then **Fast**, **Balanced**, **Most capable**),
  each showing its inference engine, checkpoint and quantization, memory and speed
  here: a default engine that installs unattended and,
  where faster, a specialized engine built on the computer. Download asks first and lists exactly
  what is downloaded, then shows its progress with Cancel; everything is pinned and checked, runs for this computer only, starts when a
  chat sends to it and stops when the app quits. The catalog of models and engines comes from
  local-llm-radar (`crates/local-models/catalog.json`). **API provider model** lists pi's own
  providers, which need only an API key (checked with the provider first, where it allows that) or
  a sign-in; then all their models are in the menu. What is connected already is listed below the
  two choices. Settings → pi & models groups what is connected into **This device**, **Other
  devices** and **API connections**, each with a trash button that removes it after asking; a
  model Pistachio downloaded is also stopped and deleted, with its engine unless another local
  model uses it. A server is asked which models it has, and they are ticked in a list; their
  context, longest answer and whether they think or see pictures are filled in as far as the
  server says. Models can be tested with a short message before they are added, and added by their
  ID when a server has no list. Adding a thinking model checks how its server takes thinking levels (the
  prompt vLLM builds for each way of asking, the efforts llama.cpp's template knows, and one short
  reply with thinking off), so each level of the message box does what it says; a model that
  thinks anyway shows **lowest** instead of **off**. The same screens change and remove what is connected; what they do
  not know of a provider or model in `models.json` stays as it is. The file can also be edited as
  text, with validation. It is the one the session's pi reads (on the SSH host for remote
  sessions); the previous version is kept as `models.json.bak-pistachio`. pi reads the file only
  when it starts, so after saving it restarts on the same session (after the current turn, if one
  is running) and selects the new model.
- **Pictures**: when the agent reads an image, a thumbnail shows next to the call (the first two of
  an "Explored" group, with "+N"); pictures attached to a prompt show above it, with their file
  extension in a corner. Clicking one opens a full-window viewer: ← / → or the arrows browse all of
  that call's, group's or prompt's pictures, Esc closes it. pi's notes to the model about pictures
  it made smaller are not shown.
- **Working indicator**: a little green π with orbiting dots shows a working agent in the
  chat ("Working… 12s · esc to interrupt", centred) and before its name in the sidebar. Settings
  → Appearance → Working indicator picks the style, with a live preview.
- **Tokens per second**: while a reply streams, a pill under the top bar shows a line of the
  model's speed over the last seconds and its speed now ("~31 tok/s"), then the reply's exact
  average in gray ("avg. 30 tok/s") before it slides away. The live figure is estimated from the
  streamed text and calibrated per model against the provider's exact counts (hover it for how far
  off it has been); models whose thinking doesn't stream get none. Replies that call a tool aren't
  judged (a tool call's markup never streams), and a server that counts no tokens at all is
  estimated throughout ("avg. ~30 tok/s").
- **Transcript layout**: the model's text is the main voice. Tool calls and thinking sit on a
  thin rail on the left in a quieter style, with more room where they meet the text.
- **Turn rail**: dots at the left edge of the conversation, one per prompt (Chat and Code, from two
  prompts on). The turn being read has a longer dot in the accent color. Hovering a dot shows a
  card with that prompt and the start of its answer; clicking it scrolls there. With many turns
  the dots move closer together. A side chat's taken-over history has none.
- **Find in chat** (⌘F): a search bar over the conversation. Every match is highlighted;
  Enter / Shift+Enter (or ⌘G / ⌘⇧G) go to the next / previous one with a "3 of 12" count, and
  Esc closes the bar. It searches prompts and replies as shown, thinking, tool output and the
  agent's edits, including collapsed rows: the row with the current match opens and the
  transcript scrolls to it. Case is ignored.
- Right panel: git changes and a file tree of the working folder. Clicking a file opens it
  read-only in the panel, with syntax colors and line numbers (← goes back to the tree);
  **Mention** in its header, the **+** on a row, or ⌥-click in the tree puts an `@path`
  mention in the message box instead. Binary files show a short notice, and files over
  512 KB open with their first 512 KB. Files on an SSH host are read over the connection.
- **File links** (Code mode): a file of the session's folder that an answer names, in
  backticks (`fib.rs`) or as a path in the text (src/main.rs:12), is a link that opens it in
  the panel, at the line when one is given; the path of a Read, Edited or Wrote row does the
  same. A bare name is linked only when exactly one file in the folder has it. Links are
  added when an answer is finished (the folder is listed again when a turn ends, so files
  the turn made are found) and only in what the chat draws: pi, the session file and
  copying keep the answer as the model wrote it.
- Syntax colors for code by file extension: chat code blocks, the agent's edits (its diffs
  in the chat), the files it reads, and the Changes panel's diffs. Diffs keep their red and
  green backgrounds with the code colors on top; the old and new text are rebuilt from each
  diff and colored as a whole, so multi-line strings and comments come out right.
- Bottom panel: an integrated terminal in the working folder; drag its top edge to resize it
  (the height is remembered).
- macOS and Linux (Wayland and X11), plus an experimental Windows build. GPU-rendered with [GPUI](https://gpui.rs) via
  [gpui-kit](https://gpui-kit.com).
- Windows: an installer (`Pistachio-…-windows-x86_64-setup.exe`: per user, no administrator
  rights, Start menu entry, optional desktop shortcut, uninstaller in Settings → Apps), the
  Pistachio icon on the program and in the taskbar, minimize / maximize / close at the right of
  the top bar (also on Linux when the app draws its own window frame), and no console windows
  for the programs it starts (pi, git, model servers). The hardware scan reads Windows' own
  APIs: memory, processor, free disk and any graphics card with memory of its own.
- **Finding pi** at startup: the configured program or `pi` on the `PATH`, else where npm,
  Node.js version managers (nvm, fnm, Volta, asdf, mise, n, nvm-windows, Scoop) and pnpm, Bun,
  Yarn or Homebrew put it, which then becomes the pi program. When there is none, a card offers
  **Install pi** (into `~/.pistachio/server`, with Node.js if needed, no administrator rights; on
  macOS, Linux and Windows) and **Find pi…** (choose the program or its folder; it is checked
  before it is used).

## Sandbox

In Code sessions on this computer, the agent's commands run in a sandbox (on by default, macOS and
Linux; Windows is planned, #47).

- **What is sandboxed:** every `bash` command runs through the app's helper
  (`pistachio --sandbox-tool run`), which starts it under **Seatbelt** on macOS, **bubblewrap** on
  Linux, or **Landlock** where bubblewrap is missing or not allowed to run (Ubuntu's AppArmor,
  containers). pi's file tools (`read`, `write`, `edit`, `grep`, `find`, `ls`) keep to the same
  rules, checked by a small pi extension. pi itself, its logins and model requests, MCP servers,
  other extensions and your own `!` commands are not sandboxed.
- **Files:** the home folder is private. Commands and file tools see only the session's folder
  (and, for a subfolder of a repository, the rest of it), pi's skills, the **tool folders** and the
  **allowed folders**. Both are lists where each folder has Read and Write to tick (unticking
  Read unticks Write; ticking Write ticks Read), a red trash can, and + to add. The tool
  folders start as what this computer has: caches to write (`~/.cargo`, `~/.npm`, `~/.cache`, …),
  programs and settings to read (`~/.nvm`, `~/.local`, `~/.gitconfig`, …) and `$PATH` folders in
  the home folder; switched off, none of them. Outside the home folder, programs and system files
  stay readable, since commands need them. Changes are allowed only in the session's folder,
  temporary folders, the git folder its commits need (also from a subfolder or a worktree) and
  what is writable in the lists. On macOS the folders on the way to the project (`~`, `~/code`)
  can be listed, since programs find their working folder that way, but nothing in them read.
- **Always hidden**, also inside an allowed folder: `~/.ssh`, `~/.aws`, `~/.azure`,
  `~/.config/gcloud`, `~/.gnupg`, `~/.config/gh`, `~/.docker/config.json`, `~/.kube`, `~/.netrc`,
  `~/.git-credentials`, `~/.npmrc`, `~/.pypirc`, password stores and keychains, and pi's
  `auth.json`. Environment variables that hold keys and tokens (names ending in `_API_KEY`,
  `_TOKEN`, `_SECRET`, …) are removed from commands. pi keeps them for its model requests, so keys
  never reach the sandbox.
- **The gate: what needs your keys.** In the sandbox, `git`, `gh`, `gpg` and `ssh-keygen` are small
  wrappers. Local git (commit, branches, merge, rebase, log, …) runs in the sandbox as usual. What
  needs your logins goes to a socket the sandbox extension listens on, outside the sandbox:
  - git's `clone`, `fetch`, `pull`, `push` (tags, deletes and force pushes are pointed out),
    `ls-remote`, `submodule update` (each submodule's address is listed) and git-lfs's `fetch`,
    `pull` and `push`;
  - `gh` for a repository: reading pull requests, issues, runs and releases; opening, commenting
    on, closing or merging pull requests and issues; creating a repository; deleting one (asked
    every time, with a warning that it can't be undone; GitHub needs the `delete_repo`
    permission). `gh api`, logins and secrets are not run. With several GitHub accounts (a personal one and a company one), the
    dialog offers each: the command runs with that account's token only, and your active account
    stays as it is. A repository for an owner (`gh repo create acme/tool`) is offered only as
    accounts that are that owner or members of it, else refused. "Always" remembers the account;

  - signing a commit or tag with your GPG or SSH key (git's own signing call, in git's namespace).

  The gate reads the request itself: only these commands and a list of their options, only in the
  session's folder, only from remote servers (no local folders or helper programs as addresses), and
  only in repositories whose `.git/config` starts no program (`core.sshCommand`,
  `credential.helper`, filters, `url.*.insteadOf`, …; those it names and refuses). It runs git with
  hooks off. You are asked in a see-through card that slides up above the message box (one line; a
  button on its edge shows the details, open at first when something needs care) and says exactly
  what will happen (the commits, the branch, the remote's real address, a force push or the default
  branch): **Allow**, **Always for this project** (that project, that remote, that kind of thing:
  fetch, push, push to the default branch, force push, sign, read or change on GitHub) or **Don't
  allow**. Your keys never enter the sandbox; the command's output comes back to the agent. What you
  allowed is listed in Settings → Sandboxing & Security → Git and GitHub, each removable. git's own
  settings (`~/.gitconfig`, `~/.config/git`) are always readable in the sandbox, so commits are made
  as you. Sandboxed sessions tell the model how this works before it starts, and `gh auth status` is
  answered by the gate: the accounts logged in and the git identity, never a token.
- **Allowed websites:** None (the default), All, or Custom. With None a command reaches a website
  only once you allow it: the first time a conversation's commands use a site, a question like the
  git gate's asks **Allow** (for that conversation), **Always for this project** (listed under
  "Allowed when asked", each removable) or **Don't allow** (the command gets 403). Connections made
  at once share one question. Custom lets the websites listed through without asking (others are
  asked about), which start as where code and packages come from (GitHub, npm, PyPI, crates.io,
  Go, RubyGems, Maven). A site is allowed as a whole (HTTPS doesn't show pages): `github.com`, or `github.com/*` with its subdomains. The field
  checks an address as you type it (`httttp://`, `wwww.`, missing endings) and looks the name up,
  and a page like `https://www.google.com/search` is taken as its site. Custom works through a
  proxy in Pistachio (None too): the address commands get names their conversation, so the proxy
  asks in the right one through its gate. Commands may connect only to it (on Linux with bubblewrap, a bridge in the
  sandbox passes their connections to its Unix socket), it lets allowed sites through and answers
  403 to others, and reads the list for each connection, so changes apply at once. Commands find
  it through `HTTPS_PROXY` and the like (curl, git, npm, pip, cargo; Node with
  `NODE_USE_ENV_PROXY`); tools that ignore those get no network. On Linux, Landlock needs Linux
  6.7 to limit the network.
- **In a session:** a shield next to the folder in the message box ("Some sites", "No network" or "No sandbox"
  when that applies); its menu runs the project without a sandbox (or with one again) and opens
  Settings → Sandboxing & Security. A call the sandbox blocked says so under it, with a button that allows
  reading (or changing) the folder it tried to use.
- **Asking you for help:** when the sandbox blocks a step the task needs and nothing inside the
  sandbox would do, the model can call `request_user_help` with a short title, its reason and
  maybe a command to suggest. A pill above the message box says "Needs your help: …"; opened, it
  shows the command the sandbox blocked last, its folder and its error (recorded by the
  extension) apart from the model's reason and suggestion. **Copy command**, **Done** and
  **Dismiss** (Esc); the model gets the answer. It runs nothing and asks for no credentials. It is
  offered only while the sandbox is on for the folder and its switch in Settings → Tools → Code is
  on (the default), and a pill goes when the agent stops.
- Settings are `~/.pi/agent/pistachio/sandbox/settings.json`, read by every command, so changes
  apply from the next one, also in running sessions. `script/test-sandbox.py` checks it end to end
  with real pi and a scripted model.

## Documents

Documents are read as Markdown: the ones you attach to a chat, PDF files that chats find on the web,
and those pi reads in Code sessions on this computer. Settings → Document parsing chooses what reads
them. The built-in reader is the default and needs no download; the others are downloaded on request
and can be removed again (macOS and Linux):

| Parser | Good at | Download (on disk) | License |
|---|---|---|---|
| Built-in ([pdf-extract](https://crates.io/crates/pdf-extract), [calamine](https://crates.io/crates/calamine) and our own) | plain text of PDFs, page by page; Word, PowerPoint, Excel, OpenDocument and EPUB files | none | MIT |
| [PyMuPDF4LLM](https://github.com/pymupdf/pymupdf) | fast; headings, columns and tables of PDFs made on a computer | 143 MB (320 MB) | AGPL-3.0 |
| [Docling](https://github.com/docling-project/docling) | complex layouts, tables, scans; also Word, PowerPoint and Excel files | 788 MB (1.8 GB); Linux 3.7 GB (8.5 GB) | MIT |
| [MinerU](https://github.com/opendatalab/MinerU) | academic papers: formulas, dense columns, long tables, scans | 1.0 GB (1.3 GB) | MinerU Open Source License (Apache-2.0 with added terms) |

Sizes are of a Mac; Linux differs little, except that Docling comes with NVIDIA's libraries there
(that is how PyTorch is published for Linux). MinerU runs its small models ("basic"), not the
language model that needs a strong GPU. Macs with an Intel processor only have PyMuPDF4LLM (an older
version), because the others are not published for them any more; parsers need macOS 14 or newer,
and Windows has only the built-in reader. Settings says what is not available.

They do not come with Pistachio. The licenses fit because each is an optional program that you
download and that runs as a separate process; nothing of it is part of Pistachio. Downloading asks
first and shows its parts with their sizes, and the license. uv, a private Python, the packages and the models come from
GitHub, PyPI and Hugging Face, each pinned and checked against its checksum, and go to
`~/.pi/agent/pistachio/documents/`; your own Python is not touched. A new installation must read a
small test document before it replaces an old one. Parsers run one process per document, without
network, and for at most 10 minutes (2 for a PDF from the web). If the chosen one is missing, fails
or takes too long, the built-in reader is used and the result says so.

- **Chats:** the card of an attached PDF shows its first page with a spinner and “Reading…” until it
  is done; it can be removed meanwhile, and the message is not sent before. Once sent, its first
  page shows above the message, with “.pdf” on it (macOS, and Linux with poppler's `pdftoppm`;
  otherwise a page with its name); clicking either shows the file in the viewer (below). The text
  goes to the model with the message but is not shown, and Edit and Retry keep it. The same goes for
  Word, PowerPoint, Excel (`.docx`, `.pptx`, `.xlsx` and their kin, `.xls`, `.xlsb`), OpenDocument
  (`.odt`, `.odp`, `.ods`) and EPUB files, whose headings, lists, tables, notes and footnotes are
  kept: Docling reads the Office ones if it is chosen, the built-in reader all others. Plain text
  can be in any common encoding and up to 16 MB. Pictures in BMP, TIFF and SVG are converted to PNG
  (in Code sessions, SVG stays an `@path`). Files on this computer attached to a session on an SSH
  host are read too.
- **Long texts:** text of more than 256 KB, of a document or a plain file, is shortened to a quarter
  of that: its outline, its start and the parts that match your message. The card says “Shortened”,
  and the model is told. A slider in Settings → Document parsing changes the limit, from 32 KB to
  all that fits. What does not fit in what is left of the model's context (a fifth of which is kept
  for the answer) is always shortened. Logs, tables and source code are cut between lines, and their
  start (the names of columns) is always there.
- **Viewer:** PDF files are shown in the app, drawn by [hayro](https://github.com/LaurenzV/hayro) in
  a helper process that ends with the viewer. Only the pages you look at and their neighbors are
  drawn. The sun or moon chooses light or dark pages (like the app until you choose; remembered):
  dark ones keep the hue of colors, and leave photos and pages that are dark anyway as they are.
  Links open web pages and mail, or lead to a place in the file; ← at the top, or ⌘[ , goes back.
  Zoom with the buttons, ⌘+ / ⌘- / ⌘0 (fill the width) or a pinch; scroll with ↑ / ↓, Space, Page Up
  / Down, Home / End, and ← / → for pages. Another button opens the file in your usual app, to
  print, search or copy from it. Files protected with a password are not shown.
- **Text viewer:** attached text files (`.txt`, `.tex`, `.csv`, source code and any others that are
  plain text) open in the app too, with line numbers and the colors of the Code setting. Text can be
  selected, copied and searched (⌘F), and not changed; a button wraps long lines, which prose has
  from the start, and another shows the file in its folder. Of a file of more than 2 MB the start is
  shown, and above 512 KB there are no colors.
- **Web search:** PDFs found on the web are read with the same parser; the model gets the passages
  that match, with tables kept whole.
- **Code:** an attached PDF stays an `@path`. When pi reads a PDF, a small embedded extension returns
  its Markdown; `offset` and `limit` are lines of that. Other files are pi's own business, and so are
  all files in sessions on an SSH host.
- `config.json` in that folder has the parser and the limits: `timeout_secs`, `web_timeout_secs`,
  `max_file_bytes` (100 MB) and `whole_text_bytes` (256 KB; `null` for all that fits). The text of
  the last documents is kept in `cache` there (24 at most), because long ones are read in parts;
  `attached` has where attached files are and the small pictures of PDFs.
- Try it without the GUI: `pistachio --documents-tool parse paper.pdf` prints the Markdown.

## Speech to text

The microphone button at the right of the message box dictates instead of typing, and so does
⌘⇧Space (Ctrl+Shift+Space on Linux/Windows) in the message box: hold it to talk and let go to
keep the text, or tap it to start and tap again to stop. Speech is
transcribed on this computer by a model downloaded once (none ship with the app); the first
press offers the choice:

| Model | Download | Languages | Speed |
|---|---|---|---|
| Parakeet v3 (NVIDIA, recommended) | 671 MB | 25 European languages, incl. English and Danish | very fast |
| Whisper large-v3 turbo (OpenAI) | 1.0 GB | about 99 languages | slower |
| Whisper small (OpenAI) | 375 MB | about 99 languages | fast |
| Moonshine v2 (Moonshine AI) | 141 MB | English only | fastest |

In Settings a download asks first and shows the files, what each is for and its size. It runs in
the background with a progress bar at the top of the chat, from a pinned Hugging Face revision, and every file is checked against its SHA-256 before the model is used.
While you speak, the text appears greyed out and keeps improving; when you stop talking (after
the silence set in Settings, 2.5 s by default), press Enter or press stop, it becomes normal
text in the message box, ready to edit or send. Esc discards it. Pauses and the end of speech
are found by a small voice detector built into the app (Silero VAD), which goes by how speech
sounds rather than how loud it is, so soft speech counts and a fan or typing does not. Models
live in `~/.pi/agent/pistachio/speech/`.

## Conversation names

Off by default (Settings → Conversation names). When on, a new conversation gets a short name
(about 3–6 words) made from its first message instead of the message's first line, and the
sidebar types it out letter by letter behind a caret. The message is sent to pi right away;
naming runs beside it and never holds the conversation up. The name is saved through pi
(`set_session_name`), so the terminal `pi` shows it too, and a name you gave the conversation
yourself (Rename, or `/name`) is never replaced. Very short first messages ("Fix the login
bug") are their own name, and slash commands are not named.

The name is made by either:

- **The conversation's own model**: one short request through a separate
  `pi --print --no-session` with the same provider and model (no tools, skills, context files
  or extensions, its own short instructions, thinking off). The conversation's pi, its
  session file and the model's context never see it, so the session stays exactly what the
  terminal shows. For a conversation on an SSH host, that pi runs on the host the same way.
- **A small model on this computer**, downloaded once (none ship with the app) from a pinned
  Hugging Face revision and checked against its SHA-256. It runs on the CPU (with
  [candle](https://github.com/huggingface/candle)) once per conversation, in the background,
  and is unloaded right after; nothing is sent anywhere.

| Model | Download | Languages | |
|---|---|---|---|
| Qwen2.5 0.5B (Qwen, recommended) | 498 MB | many | about a second per name |
| SmolLM2 360M (Hugging Face) | 389 MB | English | smallest |
| Qwen2.5 1.5B (Qwen) | 1.1 GB | many | the best names, a few seconds each |

All three are Apache 2.0 licensed. Downloading one asks first and shows its files. Models live in `~/.pi/agent/pistachio/naming/`.

## Settings and updates

Settings opens from the Pistachio menu (⌘,) or the button at the bottom of the sidebar. Every
change is saved right away. Small π signs drift behind its pages and move out of the way of the
mouse. Picking a section in the sidebar scrolls to it, and its heading glows for a moment. Lists on
the Sandboxing & Security page light up under the pointer and scroll by themselves.

- **General:** theme (follow the system, light or dark), code colors (the syntax palette for
  code in chat, diffs, skills and `models.json`, with a live preview), the working indicator's
  style (with a live preview), text size (scales the interface),
  whether to open in Chat, Code or the mode used last, reopening the last session, whether chats
  compute with Python, and notifications for chats working in the background.
- **pi & models:** which `pi` to run (`PISTACHIO_PI_BIN` still wins), the model and thinking
  level pi starts with (saved in pi's own `settings.json`, like `/settings` in the terminal),
  and what is connected (with whether its server answers) to add to, change or remove.
- **Web search:** the search service (DuckDuckGo, SearXNG, Brave or Tavily) and the address or
  API key the chosen one needs (keys are stored only on this computer) or a SearXNG on this
  computer, whether new chats search and with how much effort, limits
  per message, and local network access.
- **SSH connections:** whether saved connections connect when the app opens (off by
  default), and the saved hosts with their status, connecting, adding and managing them.
- **Speech to text:** the dictation models (download, use, remove), how long a silence ends
  dictation, and the language for Whisper models.
- **Conversation names:** automatic names for new conversations (off by default), whether the
  conversation's own model or a small model on this computer makes them, and the local models
  (download, use, remove).
- **Document parsing:** what reads PDF files, and the parsers that can be downloaded (size, license,
  download, use, update, remove).
- **Sandboxing & Security:** the sandbox (see [Sandbox](#sandbox)): which one this computer has (with the
  command that installs bubblewrap where it is missing), on or off, the tools' folders, the
  allowed folders (read-only or to change), the network (None, All, or Custom with the allowed
  sites), and what the agent may do with your git and GitHub logins without asking (Git and
  GitHub). Also which project files pi loads: pi's `defaultProjectTrust` and the saved
  decisions in its `trust.json`, which can be removed.
- **Tools:** the tools the model gets in chats and in Code sessions on this computer. Each mode
  starts with how much of the model's context its tools take with every message: about how many
  tokens (the characters of each tool's description, parameter schema and system-prompt
  guidelines, at about 4 characters a token; in chats also the total with web search on), and a
  bar with each tool's share. Each tool has a one-line summary, where it comes from (pi, Pistachio
  or an extension) and its size; "What the model sees" opens, word for word, what pi tells the
  model. In chats it also says when web search's tools are offered (while web search is on).
  Switches turn Python on or off in chats (on by default; the same as under General) and pi's
  `grep`, `find`, `ls` and `codemode` in Code sessions on this computer (off by default; passed
  to pi as `--tools +grep,…`, so the terminal `pi` is not affected). The sandbox checks `grep`,
  `find` and `ls` like `read` (what they may see, and no searching into hidden folders), and
  `codemode`'s scripts reach files and commands only through the session's tools, which the
  same checks cover. The text Pistachio's own tools give the model (`python`, `web_search`,
  `web_fetch`, the sandboxed `bash`) can be edited there: **Edit** turns the description and
  guidelines into fields, saved as you type to `~/.pi/agent/pistachio/tools/texts.json`, which
  the tools read as pi starts (`PISTACHIO_TOOL_TEXTS`); **Use the original text** undoes it.
  Changes apply to conversations started afterwards. pi is asked for them when the page is shown: a pi that keeps
  no session, started like a conversation of each mode, reports them through a small extension
  (`~/.pi/agent/pistachio/tools/tools-probe.ts`) and quits.

The app's own settings live in `~/.pi/agent/pistachio/settings.json`.

**Updates.** When Pistachio opens (and from **Check for Updates…** in the Pistachio menu or
Settings) it asks GitHub for the latest release. If there is a newer one, a prompt in the middle
of the window shows what's new. **Update** installs it the way Pistachio was installed and opens
the new version: `brew upgrade --cask pistachio` for a Homebrew install on macOS (otherwise the
new app replaces the old one), the .deb or .rpm on Linux (the system asks for your password) or
the binary the tarball installed, and `pistachio.exe` on Windows (also when the installer put it
there). The download is checked against
GitHub's checksum; if anything fails, the old version stays and the prompt says what happened.
A progress bar shows how much is downloaded, also when Homebrew downloads it.
**Download instead** opens the download in your browser, and a version can be skipped. Nothing is
downloaded or installed without a yes, and the automatic check can be turned off in Settings.

**No internet.** Every download (updates, local models, document parsers, speech and naming
models, SearXNG) first checks that the internet can be reached. When it can't, its button shows a
crossed-out Wi-Fi icon and says **No internet**, and nothing starts; clicking it checks again.
Nothing watches the network: it is asked when downloads are shown and when one is asked for.

**What's new.** After an update, Pistachio shows what changed since the version you had.

## Keyboard shortcuts

Composer behaviour mirrors pi's terminal.

| Key | Action |
|---|---|
| Enter / Shift+Enter | Send (steers while the agent works) / newline |
| Alt+Enter | Queue a follow-up message |
| Esc | Interrupt (queued messages return to the editor), or stop `!cmd` / `/compact` |
| Alt+↑ | Restore queued messages to the editor |
| Shift+Tab | Cycle thinking level |
| Tab / Enter | Complete the highlighted `/` command or `@file` (↑ / ↓ choose) |
| ↑ / ↓ | Earlier / later prompts (in an empty message box) |
| ⌘⇧Space | Dictate: hold to talk, tap to start / stop |
| Ctrl+T / Ctrl+O | Expand all thinking / tool output |
| ⌘N / ⌘B / ⌘⌥B | New chat or session / toggle sidebar / toggle right panel |
| ⌘⇧N | Side chat of the conversation you are in |
| ⌘1 / ⌘2 | Chat mode / Code mode |
| ⌘F | Find in the conversation |
| Enter / Shift+Enter, ⌘G / ⌘⇧G | Next / previous match (in the find bar) |
| ⌘⇧D / ⌘⇧E | Changes / files panel |
| Ctrl+` | Toggle terminal |
| ⌘W / ⌘M | Close (the app keeps running) / minimize the window |
| ⌘, | Settings |

(Use Ctrl instead of ⌘ on Linux/Windows.)

In the terminal on macOS, Option-← / Option-→ move by word and ⌘-← / ⌘-→ / ⌘-Backspace go to
the start / end of the line or delete to its start, as in Terminal.app and iTerm. Each folder
keeps its own shell, so a dev server keeps running when you switch sessions.

Built-in pi commands that the TUI handles itself are mapped onto RPC: `/new`, `/compact`, `/name`,
`/model`, `/thinking`, `/session`, `/copy`, `/settings`, `/quit`, `/reload` (restarts pi on the
conversation) and `/login` / `/logout` (pi's sign-in in a terminal). Extension commands, prompt
templates and `/skill:…` go to pi unchanged.
