# General

What each part of Pistachio does and how to use it. The settings of a feature are described on
its page under Settings in the menu.

## Chat and Code

The switch at the top of the sidebar picks the mode (⌘1 / ⌘2).

Code
: The coding agent in a project folder, with pi's tools (read, write, edit, bash). Sessions are
  grouped under their folder in the sidebar, including ones started from pi in the terminal. A
  new session (⌘N) asks for the folder: a recent one, another folder, or one on an
  [SSH host](settings/ssh-connections.md). It never picks one by itself.

Chat
: Plain conversation with **no file or shell tools**: the model can't read, write or run
  anything on your computer. Its only tools are [web search](#web-search) (while it is on) and
  [Python](#python-in-chats). Attached files go along as text, and `!` commands are off.

Both modes list their conversations in the sidebar, where you can search, rename and move them
to the trash. Opening one shows its history at once, read from pi's session file, without
waiting for pi to start. If pi stops (a crash or a failed start), the chat offers
**Restart pi** on the same session, and anything not yet sent goes back into the message box.

pi gets your login shell's environment (API keys, proxies, cloud settings), also when
Pistachio is started from the Dock or a desktop launcher.

## The conversation

- Answers stream in as Markdown. Thinking is collapsed ("Thought for 4s"), and each turn ends
  with how long it took.
- Tool calls sit on a thin rail on the left, in a quieter style than the model's text. Shell
  commands are labelled by what they do ("Read Cargo.toml", "Ran cargo test"); click one to see
  the command and its output. A command that isn't fully understood (loops, `$(…)`, flags that
  could change files) is shown as written, so a label never hides a change.
- Runs of calls that only look around (reading, listing, searching) fold into one
  "Explored · N steps" row.
- Edits show as inline diffs with syntax colors.
- LaTeX math is drawn: `$…$` and `\(…\)` inline, `$$…$$` and `\[…\]` as blocks. Prices like
  "$5 and $10" stay text.
- Pictures the agent reads show as thumbnails; click one for a full-window viewer (← / → to
  browse, Esc to close).
- In Code mode, a file of the project that an answer names (`main.rs`, `src/main.rs:12`) is a
  link that opens it in the [Files panel](#side-panels-and-terminal), at the line if one is
  given.
- From two prompts on, dots at the left edge mark each prompt. Hover one to preview it, click
  it to jump there.
- **Find in chat** (⌘F) searches prompts, replies, thinking and tool output, collapsed rows
  included. Enter / Shift+Enter (or ⌘G / ⌘⇧G) go to the next / previous match, Esc closes it.
- Ctrl+T expands all thinking, Ctrl+O all tool output.
- **Tokens per second.** While a reply streams, a pill slides down under the top bar: a line of
  the model's speed over the last seconds and its speed now ("~31 tok/s"). When the reply ends it
  turns gray with the reply's exact average ("avg. 30 tok/s") and slides away after a few seconds.
  The live figure is an estimate from the streamed text; Pistachio calibrates it for each model
  against the exact token counts of its finished replies, and hovering the pill says how far off
  it has been for that model on average (usually a few percent). A model that thinks without
  streaming its thinking, or streams only a summary, gets no live figure, since the tokens it
  writes can't be seen; a new model shows only the exact average until one of its replies has
  been checked. Replies that call a tool (a web search, say) aren't used for that check, since a
  tool call costs tokens that never stream. A local server that reports no token counts at all
  gets estimates throughout, also for the average ("avg. ~30 tok/s").

## The message box

The message box works like pi's terminal.

- **Enter** sends; while the agent works it steers the current turn. **Alt+Enter** queues a
  follow-up instead. **Esc** interrupts (queued messages come back to the box). Shift+Enter
  is a new line.
- `/` completes commands, `@` mentions a file (paths with spaces are quoted), `!command` runs a
  shell command with live output (Esc stops it). ↑ / ↓ in an empty box go through earlier
  prompts.
- The paperclip attaches files. Each shows as a card with a small picture of it.
  [Documents](settings/document-parsing.md) (PDF, Word, Excel, …) are read as Markdown in
  chats.
- Below the text: the working folder and machine (Code), the model and thinking level
  (Shift+Tab cycles thinking), and a wheel that shows how much of the context window is used.
  In chats, the globe sets [web search](#web-search). In Code sessions on this computer, a
  shield shows the [sandbox](#sandbox-and-the-git-gate).
- Messages typed while pi is starting or busy are listed above the box as "Waiting to send",
  each with a cancel button. A draft you haven't sent stays with its session.
- The microphone button [dictates](settings/speech-to-text.md).

pi's own commands work: `/new`, `/compact`, `/name`, `/model`, `/thinking`, `/session`,
`/copy`, `/settings`, `/reload` (restarts pi on the conversation), `/login` and `/logout`
(pi's sign-in, in a terminal), `/quit`. Extension commands, prompt templates and
`/skill:…` go to pi unchanged.

## Branches and side chats

- Hover a prompt for **Copy** and **Edit**. Hover the end of a turn for **Copy reply**,
  **Retry** (newest turn) and **Branch from here**. Edit, Retry and branching continue in a new
  session, like pi's `/fork`; the original stays in the sidebar.
- `/fork` picks a message to branch from, `/clone` copies the session, `/resume` opens an
  earlier one, `/export [path]` writes HTML, and `/share` makes a secret GitHub gist (with
  `gh`).
- **Side chat** (⌘⇧N, `/side`, or the tab on the chat column's right edge) opens a second
  conversation in a column to the right, with everything the conversation has so far, also
  while its agent is working. The original keeps going. The side chat is a session of its own,
  with the same model, folder and machine, listed under its conversation in the sidebar. Its
  top bar moves it to the main column or closes it.

## Background sessions

Switching away from a conversation that is working keeps it running. Its sidebar row shows a
spinner, then a green dot when it finishes, and a notification says so. Questions from
extensions in a background conversation wait until you open it (amber dot), so they are never
answered in the wrong one. Notifications and the Dock bounce can be turned off under
[Settings → General](#settings-general).

## Side panels and terminal

Right panel (⌘⌥B)
: **Diff** (⌘⇧D) shows the working folder's git changes with syntax colors. **Files** (⌘⇧E)
  is a file tree. Click a file to read it in the panel, with line numbers (← goes back).
  **Mention**, the **+** on a row, or ⌥-click puts an `@path` mention in the message box.

Terminal (Ctrl+`)
: An integrated terminal at the bottom, in the session's folder. Drag its top edge to resize
  it. Each folder keeps its own shell, so a dev server keeps running when you switch sessions.

For sessions on an SSH host, both panels and the terminal work on the host.

## SSH remotes

In Code mode, the device button next to the folder ("This computer") runs pi on another
machine. **Add SSH connection…** lists the hosts in your `~/.ssh/config` or takes a custom one.
pi runs on the host, through your system `ssh` (your config, keys and agent apply), and is
installed there automatically if it is missing. The models you have on this computer work on
every host, and your keys never leave this computer. See
[SSH connections](settings/ssh-connections.md).

## Web search

In chats, the globe next to the paperclip chooses how the model uses the web, per chat
(`/websearch off|search|deep` works too):

- **Search**: a few searches and pages, then a concise answer with links.
- **Deep search**: a researched, sourced report. The model splits the question, searches from
  several angles, reads many pages, and its report is checked against the sources before it is
  shown. A slider sets the effort, from Light to Max.

The search service and limits are on [Web search](settings/web-search.md).

## Python in chats

Chats work numbers out in Python instead of estimating them (sums, logarithms, series,
statistics), and use it for small jobs a program does more reliably than reasoning: counting
words, sorting or reformatting text, regular expressions, JSON, dates. The code runs in
[Monty](https://github.com/pydantic/monty), a Python interpreter with no files, network,
environment or programs, stopped after 5 seconds or 256 MB. The chat shows the call as
"Computed" with its result; expand it for the code, which shows as the model writes it.

Turn it off under [Settings → General](#settings-general) → Chat mode or on
[Tools](settings/tools.md).

## Sandbox and the git gate

In Code sessions on this computer (macOS and Linux), the agent's commands run in a sandbox
(Seatbelt on macOS, bubblewrap or Landlock on Linux), and pi's file tools keep to the same
rules. Commands see the project and the folders you allow, never your keys, and reach only the
websites you allow.

When the agent needs your logins (`git push` and `pull`, `gh`, signing a commit), a card above
the message box says exactly what will happen and asks: **Allow**, **Always for this
project** or **Don't allow**. Allowed commands run outside the sandbox, and the agent gets only
their output.

The shield next to the folder shows the session's sandbox; its menu runs the project without
one (or with one again). Everything is set on
[Sandboxing & Security](settings/sandboxing-and-security.md).

## Skills

The **Skills** button under the Chat | Code switch lists every skill: the ones that come with
Pistachio, your own (`~/.pi/agent/skills`, `~/.agents/skills`) and your projects'
(`.pi/skills`, `.agents/skills`), on this computer or the SSH host.

- **+** writes a new `SKILL.md` into your skills or a project's `.pi/skills`. Optionally it is
  used only when you call it.
- Click a skill to edit it, turn it off or move it to the trash.
- pi loads a project's skills only if the project is trusted. The page shows whether it is and
  can trust it (saved in pi's `trust.json`, like `/trust`).

pi reads skills when it starts, so a change restarts it on the same session.

## Models

**Add or change models…** in the model menu (or [pi & models](settings/pi-and-models.md))
connects a locally hosted model (Ollama, LM Studio, llama.cpp, vLLM and others, on this device
or another) or an API provider. On this device it can also deploy a new model: it scans this
computer's hardware, suggests models that run well on it, and downloads and runs one on request. The startup model and thinking level are set there too.

## Settings → General

```{image} /_static/screenshots/general-appearance-light.webp
:class: shot only-light
:alt: Settings → General: Appearance
```

```{image} /_static/screenshots/general-appearance-dark.webp
:class: shot only-dark
:alt: Settings → General: Appearance
```

Settings opens with ⌘, or the button at the bottom of the sidebar. Every change is saved at
once.

Appearance
: **Theme** (follow the system, light or dark), **Code colors** (the syntax palette, with a
  preview), **Working indicator** (how a working agent is shown) and **Text size** (scales
  the whole interface).

Start
: **Open in** Chat, Code, or the mode used last. **Reopen the last session** instead of the
  welcome screen, also for sessions on SSH hosts.

Chat mode
: **Compute with Python** turns [Python in chats](#python-in-chats) on or off. Applies from the
  next message, also in open chats.

Background chats
: **Notifications** when a chat working in the background finishes or asks something.
  **Bounce the Dock icon** (macOS) or **Flash the window** when that happens while Pistachio is
  not in front.

Updates
: **Check for updates when Pistachio opens**, and **Check now**.

## Finding pi

When Pistachio opens, it checks that it can start pi: the program set in
[Settings → pi & models](settings/pi-and-models.md), or `pi` on your `PATH` (the login shell's
too). If pi is elsewhere, in Pistachio's own install or where npm, nvm, fnm, Volta, asdf, mise,
n, nvm-windows, Scoop, pnpm, Bun, Yarn or Homebrew put it, Pistachio uses that one and sets it as
the pi program. If pi is nowhere, a card says so:

- **Install pi** installs it from npm into `~/.pistachio/server` (`%USERPROFILE%\.pistachio\server`
  on Windows), with Node.js 22 from nodejs.org (checked against its checksum) when the computer
  has no recent one. No administrator rights are needed, nothing outside that folder changes, and
  deleting the folder undoes it. It shows each step; if it fails, it says why.
- **Find pi…** asks for the `pi` program (or the folder it is in), checks that it runs and uses it.
- **Not now** closes the card; sessions can't start until pi is found.

## Updates

When Pistachio opens, it asks GitHub for the latest release (no personal data is sent). If
there is a newer one, a prompt shows what's new:

- **Update** installs it the way Pistachio was installed (Homebrew, the app bundle, the
  `.deb` or `.rpm`, the tarball's binary, or `pistachio.exe` on Windows), checked against GitHub's checksum, and opens the
  new version. If anything fails, the old version stays.
- **Download instead** opens the download in your browser. A version can also be skipped.

Nothing is downloaded without a yes. After an update, Pistachio shows what changed.

Every download in Pistachio (updates, local models, document parsers, speech and naming models,
SearXNG) first checks that the internet can be reached. When it can't, its button shows a
crossed-out Wi-Fi icon and says **No internet**; clicking it checks again.

## Keyboard shortcuts

| Key | Action |
|---|---|
| Enter / Shift+Enter | Send (steers while the agent works) / new line |
| Alt+Enter | Queue a follow-up |
| Esc | Interrupt, or stop a `!command` or `/compact` |
| Alt+↑ | Put queued messages back in the message box |
| Shift+Tab | Cycle thinking level |
| Tab / Enter | Complete the highlighted `/` command or `@file` |
| ↑ / ↓ | Earlier / later prompts (in an empty message box) |
| ⌘⇧Space | Dictate: hold to talk, or tap to start and stop |
| Ctrl+T / Ctrl+O | Expand all thinking / tool output |
| ⌘N | New chat or session |
| ⌘⇧N | Side chat |
| ⌘1 / ⌘2 | Chat / Code |
| ⌘B / ⌘⌥B | Sidebar / right panel |
| ⌘⇧D / ⌘⇧E | Diff / Files panel |
| Ctrl+` | Terminal |
| ⌘F | Find in the conversation |
| ⌘W / ⌘M | Close the window (the app keeps running) / minimize |
| ⌘, | Settings |

Use Ctrl instead of ⌘ on Linux and Windows.

## Files and environment

Pistachio keeps its own files in `~/.pi/agent/pistachio/`:

| Path | What |
|---|---|
| `settings.json` | The app's settings |
| `hosts.json` | Saved SSH connections |
| `chat/` | The empty folder chats run pi in |
| `deepsearch/` | Web search settings and prompts |
| `sandbox/settings.json` | Sandbox settings |
| `documents/` | Downloaded document parsers |
| `speech/`, `naming/` | Downloaded speech and naming models |
| `tools/texts.json` | Your edits to Pistachio's tool texts |

`PISTACHIO_PI_BIN=/path/to/pi` chooses the pi to run, ahead of the setting.
