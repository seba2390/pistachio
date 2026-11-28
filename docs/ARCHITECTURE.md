# Architecture

```
crates/
  pi-rpc/       typed protocol (serde mirror of pi's rpc-types), strict JSONL framing, client, process
  transcript/   pure reducer: pi events / stored messages → renderable transcript (no UI, no I/O)
                and the tokens-per-second meter, calibrated per model on its finished replies
  pi-sessions/  index of pi's session files for the sidebar; writes the file a side chat starts from
  pi-config/    editing pi's config: models.json, SKILL.md front matter, the trust store; what a
                model server offers and how it takes thinking levels (the requests, and reading the answers)
  workspace/    git status/diff (git CLI) + gitignore-aware file listing + fs watcher
  remote/       SSH: ~/.ssh/config discovery, saved hosts, shared connections, remote probes,
                pi auto-install, askpass prompts, remote sessions, the RemoteShell for workspace
  term/         terminal backend: alacritty_terminal + PTY, UI-agnostic snapshots
  titles/       automatic conversation names: title clean-up, the one-shot `pi --print` call, and
                small local models (quantized GGUF via candle, pure Rust)
  deepsearch/   chat web search (search / deep): search backends, public-only fetching, page text extraction,
                passage ranking, checking reports, settings, and the pi extension that runs them via the app
  documents/    PDF and Office files as Markdown: the built-in reader, the parsers that can be downloaded
                (pinned and checked), running one per document, and the pi extension for `read` in Code
  sandbox/      the sandbox for the agent's tools in local Code sessions: settings, rules per project,
                Seatbelt (macOS), bubblewrap and Landlock (Linux), the helper and its pi extension
  searxng/      a private SearXNG for web search: downloading it on request (pinned and checked), running it,
                checking one at an address
  python-env/   what both download with: uv, a private Python and locked packages
  local-models/ models suggested for this computer: hardware scan, the pinned catalog (from
                local-llm-radar), suggestions (tiny, fast, balanced, most capable), downloading
                engines and models, running them
  pi-install/   pi on this computer: finding it (PATH, npm and Node.js version managers' folders),
                checking it, installing it into ~/.pistachio/server (with Node.js if needed)
  platform/     the OS: child processes without console windows on Windows (`no_window()`, for
                every program the app starts there) and Windows' own hardware APIs
  app/          the GPUI application (binary `pistachio`)
```

Only `app` depends on the UI toolkit; everything else is plain Rust with its own tests. The app
keeps a pi process alive only for the visible chat and for chats whose agent is still working.

## Documents

A document is always read by a helper process, the app's own executable run as `--documents-tool`
(or, for a PDF a chat finds on the web, the `--deepsearch-tool` helper that fetched it): it cannot
take the app down, and ends when nobody waits for it. A downloaded parser is one more process per
document, with its own Python, packages and models, no network, a time limit, and its own process
group. It watches its standard input, so it also ends when the helper is killed. Parsers that load
models take turns, across processes, because several would fill the memory.

- The PDF viewer (`documents::viewer`) keeps one helper for as long as it shows a file: it asks for
  one page at a time and gets its pixels and links. A page that breaks the helper, or takes too
  long, is marked, and another helper draws the next one. Pages are laid out without drawing them
  (`viewer::layout`), so only what is looked at is drawn and kept.
- `web_fetch` turns the Markdown into blocks (`deepsearch::markdown`) and ranks passages as for any
  page.
- Office, OpenDocument and EPUB files are read by `documents::office` in the same helper: XML in zip
  archives, with limits on what is unpacked and no entities, and spreadsheets by calamine.
- Attachments in chats keep the text until the message is sent; one that is too long is cut to its
  outline and what matches the message (`deepsearch::excerpt`). How long is decided then, from the
  settings and what is left of the model's context (`Room` in `chat/document.rs`).
- In local Code sessions an extension handles pi's `tool_result` event for `read` of a PDF: pi has
  decided about and done the read by then, and the result is replaced by the same read (pi's own
  `createReadTool`) of the Markdown. Nothing runs for other files. pi on an SSH host has no parser
  and keeps its own behaviour.

## Performance

- Each column (sidebar, chat, right panel, terminal) is a cached GPUI view: it re-renders only
  when its own state changes, so a streaming answer does not re-render the sidebar or terminal.
- Streaming text is appended to retained markdown state (no full re-parse or copy per delta);
  pi events are drained in batches of up to 512 per UI update.
- Transcript, diff and file lists are virtualized; git, directory listing and session scans run
  on background threads, and session rescans re-read only files whose mtime changed.
- Measured on macOS (release build, ~2700×1750 Retina window): 89 MB physical footprint idle,
  96 MB with the terminal and Diff panel open; the app's own heap is ~9 MB, the rest is GPU
  drawable surfaces. Each live pi process adds ~110 MB (Node), so only the visible chat and
  still-working chats keep one.
- The terminal repaints only on the emulator's coalesced wakeups and copies just the visible
  viewport into a reused buffer; a hidden terminal costs nothing.
