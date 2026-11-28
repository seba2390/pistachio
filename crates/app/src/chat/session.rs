//! `PiSession`: one pi process and the live state derived from it.
//!
//! This entity is the only place the GUI talks to pi. It spawns `pi --mode rpc`,
//! pumps its events into the [`Transcript`] reducer, and exposes pi's commands as
//! methods. Views observe it and render; they never hold protocol state.

pub use deepsearch::config::Mode as WebMode;
use deepsearch::effort::Effort;
use pi_sessions::Mode;
use remote::HostId;
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gpui_kit::{Context, EventEmitter, Task};
use pi_rpc::protocol::{
    AgentMessage, CompactionReason, ContentBlock, Disposition, ExtensionUiAnswer,
    ExtensionUiMethod, ExtensionUiRequest, ImageContent, MessageContent, Model, NotifyKind,
    SessionState, SessionStats, SlashCommand, StreamingBehavior, ThinkingLevel,
};
use pi_rpc::{ClientEvent, PiClient, PiProcess, RpcError, SpawnOptions};
use pi_sessions::prompt;
use transcript::rate::{RateMeter, RateView};
use transcript::{BashStatus, InFlight, NoticeKind, RunStatus, Transcript};

use super::queued::QueuedPrompts;
use super::recall::{Recall, Snapshot};
use crate::config::{AppConfig, ChildEnv};

/// Where the pi process is in its life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lifecycle {
    /// A placeholder with no pi process (the welcome screen before any session is open).
    Idle,
    Starting,
    Ready,
    Exited {
        message: String,
    },
}

/// What a view may want to react to.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// Transcript items were appended or changed.
    TranscriptChanged,
    /// Model, thinking level, stats, title or lifecycle changed.
    MetaChanged,
    /// pi reported (or changed) the session file; the sidebar should refresh.
    SessionFileChanged,
    /// The agent finished all automatic work.
    Settled,
    /// An extension asks the user something; answer with [`PiSession::answer_dialog`].
    Dialog(ExtensionUiRequest),
    /// Replace the composer's text (an extension's `set_editor_text`).
    SetEditorText(String),
    /// Give messages back to the composer: queued ones restored by Esc / Alt+Up, prompts
    /// that could not be sent, or a message to edit. `replace` swaps out the draft;
    /// otherwise the restored text goes before it, so nothing typed is lost.
    RestoreToEditor {
        text: String,
        images: Vec<ImageContent>,
        replace: bool,
    },
    Notify {
        kind: NotifyKind,
        message: String,
    },
    /// [`PiSession::is_busy`] changed.
    BusyChanged,
    /// pi switched to another session file in the same process (fork, clone): the transcript
    /// was replaced, not appended to.
    Reloaded,
}

/// What to do with the message a branch starts from (see [`PiSession::fork_at`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterFork {
    /// Put it in the composer to edit and send.
    Edit,
    /// Send it again as it was (retry).
    Resend,
    /// Leave it out: the branch ends before it.
    Keep,
}

/// How long events pi sent before an answer (or before it stopped) may take to be handled after
/// it.
const EVENT_GRACE: Duration = Duration::from_secs(1);

/// The images of the user message `from_end` messages before the last one (0 is the
/// newest), if its text is `text`; empty when it cannot be matched.
fn user_images(messages: &[AgentMessage], from_end: usize, text: &str) -> Vec<ImageContent> {
    let user = messages
        .iter()
        .rev()
        .filter_map(|message| match message {
            AgentMessage::User(user) => Some(user),
            _ => None,
        })
        .nth(from_end);
    match user {
        Some(user) if user.content.text().trim() == text.trim() => match &user.content {
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Image(image) => Some(image.clone()),
                    _ => None,
                })
                .collect(),
            MessageContent::Text(_) => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// pi's fork entry for the user message `from_end` places before the last one, if its text
/// matches. Counting from the end keeps the match right when pi's list holds more messages
/// than the transcript (history before a compaction).
fn fork_entry(
    messages: &[pi_rpc::protocol::ForkMessage],
    from_end: usize,
    text: &str,
) -> Option<String> {
    let index = messages.len().checked_sub(from_end + 1)?;
    let message = &messages[index];
    (message.text.trim() == text.trim()).then(|| message.entry_id.clone())
}

/// Runs `gh gist create` (secret) for `path`; returns pi's viewer link and the gist URL.
fn create_gist(path: &str, env: &ChildEnv) -> Result<(String, String), String> {
    let output = ::platform::command("gh")
        .args(["gist", "create", "--public=false", path])
        .envs(env.vars().iter().cloned())
        .output()
        .map_err(|_| {
            "Sharing needs the GitHub CLI (gh): https://cli.github.com, then `gh auth login`."
                .to_owned()
        })?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh could not create the gist: {}", error.trim()));
    }
    let gist = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let id = gist.rsplit('/').next().unwrap_or_default();
    if id.is_empty() {
        return Err(format!("Unexpected answer from gh: {gist}"));
    }
    let base = env
        .var("PI_SHARE_VIEWER_URL")
        .and_then(|url| url.into_string().ok())
        .unwrap_or_else(|| "https://pi.dev/session/".to_owned());
    Ok((format!("{base}#{id}"), gist))
}

/// A prompt waiting to be sent to pi.
struct PendingPrompt {
    text: String,
    images: Vec<ImageContent>,
    /// Queue it behind the current run (Alt+Enter) instead of steering it (Enter).
    follow_up: bool,
}

/// The prompt pi has not answered yet. It is kept to give it back if pi refuses it.
struct Sending {
    prompt: PendingPrompt,
    /// [`PiSession::runs_started`] when it was sent.
    runs_before: u64,
    /// Nothing was said in the conversation before it: it names it once pi accepts it.
    first: bool,
}

/// Compaction outside a run.
#[derive(Default)]
struct Compaction {
    /// `/compact` was sent and pi has not answered yet.
    sent: bool,
    /// pi compacts (`/compact`, or an extension asked for it) and refuses prompts meanwhile.
    running: bool,
    /// How many ended so far.
    ended: u64,
}

pub struct PiSession {
    /// The SSH host pi runs on, or `None` for this computer.
    host: Option<HostId>,
    mode: Mode,
    cwd: PathBuf,
    resumed_from: Option<PathBuf>,
    process: Option<PiProcess>,
    client: Option<PiClient>,
    lifecycle: Lifecycle,
    transcript: Transcript,
    /// The run's reply and tools that pi has not saved yet, for a side chat.
    in_flight: InFlight,
    /// How fast the model writes, for the tokens-per-second pill.
    rate: RateMeter,
    state: Option<SessionState>,
    models: Vec<Model>,
    /// All levels pi offers for the model; see [`Self::thinking_levels`].
    thinking_levels: Vec<ThinkingLevel>,
    stats: Option<SessionStats>,
    commands: Vec<SlashCommand>,
    /// `setStatus` entries from extensions, shown in the status line.
    extension_status: BTreeMap<String, String>,
    /// `setWidget` blocks from extensions, shown above the composer.
    widgets: BTreeMap<String, Vec<String>>,
    /// Prompts not yet sent: pi gets one at a time, each after the previous was accepted, so a
    /// second message never arrives while pi still thinks it is idle (it would start a second
    /// run or be refused as busy).
    outbox: VecDeque<PendingPrompt>,
    /// A suggested local model is starting, so prompts wait for it (see [`crate::local_models`]).
    /// `Some(false)` once it could not start: then they go to pi, which says what is wrong.
    local_model: Option<bool>,
    /// The prompt that was sent and that pi has not answered yet.
    sending: Option<Sending>,
    /// The prompts in pi's queue, for their images.
    queued: QueuedPrompts,
    /// `clear_queue` commands pi has not answered yet. The queue updates they cause don't
    /// mean that messages were delivered.
    clearing_queue: usize,
    /// pi said it started a run whose first event has not arrived yet (until then,
    /// `transcript.is_running()` is still false). Responses and events travel separately, so
    /// the run may also have started (and even ended) before the response is handled: see
    /// `runs_started`.
    awaiting_start: bool,
    /// `agent_start` events seen so far.
    runs_started: u64,
    compaction: Compaction,
    /// [`Self::is_busy`] as last reported with [`SessionEvent::BusyChanged`].
    reported_busy: bool,
    /// `(provider, model id)` to switch to once pi has started (after adding a model).
    pending_model: Option<(String, String)>,
    /// The model and thinking level of the conversation a side chat came from, set once pi
    /// has started where they differ from what pi restored.
    inherited: Option<Inherited>,
    _event_pump: Task<()>,
    /// The resumed session's history as read from its file, shown until pi answers.
    preview: Preview,
    /// What the composer shows before pi answers (see [`crate::chat::recall`]).
    early: Early,
    _preview_task: Task<()>,
    /// Where automatic naming is (see [`crate::naming`]).
    naming: Naming,
    _naming_task: Task<()>,
    /// Starts pi once the login shell's environment is known and, on a host, its settings for
    /// this computer's models are written.
    _start_task: Task<()>,
}

/// Extension statuses the composer shows, remembered with the conversation (see
/// [`crate::chat::recall`]).
const RECALLED_STATUS: [&str; 2] = [deepsearch::STATUS_KEY, deepsearch::EFFORT_KEY];

/// Session files up to this size are read as the conversation opens (a few milliseconds),
/// longer ones in the background.
const SYNC_PREVIEW_BYTES: u64 = 4 * 1024 * 1024;

/// What the composer shows before pi has answered.
#[derive(Default)]
struct Early {
    /// The model and thinking level pi had for this conversation last time, or that its file
    /// selected.
    shown: Option<(Option<Model>, Option<ThinkingLevel>)>,
    /// `shown` is what pi said about this very conversation.
    exact: bool,
    /// pi reported the web search statuses itself.
    web_reported: bool,
}

/// The model (`provider`, `id`) and thinking level a side chat takes over.
type Inherited = (Option<(String, String)>, Option<ThinkingLevel>);

/// A side chat's file, written and ready to open ([`PiSession::side_chat`]).
#[derive(Debug, Clone)]
pub struct SideChatStart {
    pub cwd: PathBuf,
    /// The new session file (on `host` for a remote session).
    pub file: PathBuf,
    pub host: Option<HostId>,
    /// The original's model (`provider`, `id`) and thinking level, to take over.
    pub model: Option<(String, String)>,
    pub thinking_level: Option<ThinkingLevel>,
    /// The prompts it took over (shown folded).
    pub inherited_prompts: usize,
}

/// Automatic naming looks at the first prompt only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Naming {
    /// pi accepted no prompt yet.
    NotYet,
    /// Started, skipped or not wanted; later prompts never name the conversation.
    Considered,
}

/// Whether the transcript shows a preview read from the session file (see
/// [`pi_sessions::branch`]) and whether pi's own history has arrived since.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Preview {
    #[default]
    None,
    /// Being read in the background (a long file).
    Reading,
    Shown,
    /// pi's history arrived: a late preview must not replace it.
    Superseded,
}

impl EventEmitter<SessionEvent> for PiSession {}

impl PiSession {
    /// Starts pi in `cwd`, resuming `session_file` when given; on `host` (a saved SSH host)
    /// pi runs on that machine and `cwd` / `session_file` are paths there.
    pub fn new(
        cwd: PathBuf,
        session_file: Option<PathBuf>,
        host: Option<HostId>,
        cx: &mut Context<Self>,
    ) -> Self {
        let config = cx.global::<AppConfig>().clone();
        let saved = saved_host(host.as_ref(), cx);
        // pi on a host uses this computer's models once the host's settings for them are
        // written; when they must be written first, pi starts after that.
        let hosts = cx
            .try_global::<crate::hosts::HostsHandle>()
            .map(|hosts| hosts.0.clone());
        let remote_env = match (&host, hosts) {
            (Some(id), Some(hosts)) => hosts.update(cx, |hosts, cx| hosts.session_env(id, cx)),
            _ => Ok(Vec::new()),
        };
        // Remote sessions are always Code (see `spawn_options`).
        let mode = match saved {
            Some(_) => Mode::Code,
            None => config.mode_of(&cwd),
        };
        if mode == Mode::Chat
            && let Err(error) = std::fs::create_dir_all(&cwd)
        {
            log::warn!(
                "could not create the chat folder {}: {error}",
                cwd.display()
            );
        }

        let mut this = Self {
            host,
            mode,
            cwd,
            resumed_from: session_file,
            process: None,
            client: None,
            lifecycle: Lifecycle::Starting,
            transcript: Transcript::new(),
            in_flight: InFlight::default(),
            rate: RateMeter::default(),
            state: None,
            models: Vec::new(),
            thinking_levels: Vec::new(),
            stats: None,
            commands: Vec::new(),
            extension_status: BTreeMap::new(),
            widgets: BTreeMap::new(),
            outbox: VecDeque::new(),
            local_model: None,
            sending: None,
            queued: QueuedPrompts::default(),
            clearing_queue: 0,
            awaiting_start: false,
            runs_started: 0,
            compaction: Compaction::default(),
            reported_busy: false,
            pending_model: None,
            inherited: None,
            _event_pump: Task::ready(()),
            preview: Preview::None,
            early: Early::default(),
            _preview_task: Task::ready(()),
            naming: Naming::NotYet,
            _naming_task: Task::ready(()),
            _start_task: Task::ready(()),
        };
        this.recall_early(cx);
        this.show_preview(cx);
        match remote_env {
            Ok(env) if config.child_env.ready().is_some() => this.start(&env, cx),
            remote_env => {
                this._start_task = cx.spawn(async move |this, cx| {
                    let env = match remote_env {
                        Ok(env) => env,
                        Err(link) => link.await,
                    };
                    config.child_env.wait().await;
                    let _ = this.update(cx, |this, cx| {
                        this.start(&env, cx);
                        cx.emit(SessionEvent::MetaChanged);
                    });
                });
            }
        }
        this
    }

    /// Starts pi (on a host with `remote_env`) with the login shell's environment, which must
    /// be ready.
    fn start(&mut self, remote_env: &[(String, String)], cx: &mut Context<Self>) {
        let config = cx.global::<AppConfig>().clone();
        let saved = saved_host(self.host.as_ref(), cx);
        let (mut options, mode) = spawn_options(
            &config,
            &self.cwd,
            self.resumed_from.as_deref(),
            saved.as_ref(),
            remote_env,
        );
        if mode == Mode::Code && saved.is_none() {
            optional_tool_args(&code_tools(cx), &mut options);
            help_tool_env(help_tool(cx), &mut options);
        }
        self.start_process(&options, cx);
    }

    /// Starts pi with `options` and follows its events.
    fn start_process(&mut self, options: &SpawnOptions, cx: &mut Context<Self>) {
        let this = self;
        match PiProcess::spawn(options) {
            Ok((process, events)) => {
                log::info!(
                    "started pi (pid {}) in {}",
                    process.pid(),
                    this.cwd.display()
                );
                this.client = Some(process.client().clone());
                this.process = Some(process);
                this._event_pump = cx.spawn(async move |this, cx| {
                    while let Ok(first) = events.recv().await {
                        // Drain what is already buffered so a burst of deltas costs one update.
                        let mut batch = vec![first];
                        while batch.len() < 512 {
                            match events.try_recv() {
                                Ok(event) => batch.push(event),
                                Err(_) => break,
                            }
                        }
                        let alive = this.update(cx, |this, cx| {
                            for event in batch {
                                this.handle(event, cx);
                            }
                            this.report_busy(cx);
                            cx.emit(SessionEvent::TranscriptChanged);
                            cx.notify();
                        });
                        if alive.is_err() {
                            break;
                        }
                    }
                });
                this.initialize(cx);
            }
            Err(error) => {
                let message = format!(
                    "Could not start `{}`: {error}. Is pi installed and on your PATH? \
                     (Set PISTACHIO_PI_BIN to override.)",
                    options.program.display()
                );
                // An empty chat shows the message itself; a preview shown meanwhile hides it.
                this._preview_task = Task::ready(());
                if !this.transcript.items().is_empty() {
                    this.push_notice(NoticeKind::Error, message.clone(), cx);
                }
                this.lifecycle = Lifecycle::Exited { message };
                this.return_outbox(cx);
                cx.notify();
            }
        }
    }

    /// A placeholder without a pi process: nothing runs, nothing is read from disk. Shown
    /// (behind the welcome screen) until the user opens or starts a session.
    pub fn idle(cwd: PathBuf, mode: Mode) -> Self {
        Self {
            host: None,
            mode,
            cwd,
            resumed_from: None,
            process: None,
            client: None,
            lifecycle: Lifecycle::Idle,
            transcript: Transcript::new(),
            in_flight: InFlight::default(),
            rate: RateMeter::default(),
            state: None,
            models: Vec::new(),
            thinking_levels: Vec::new(),
            stats: None,
            commands: Vec::new(),
            extension_status: BTreeMap::new(),
            widgets: BTreeMap::new(),
            outbox: VecDeque::new(),
            local_model: None,
            sending: None,
            queued: QueuedPrompts::default(),
            clearing_queue: 0,
            awaiting_start: false,
            runs_started: 0,
            compaction: Compaction::default(),
            reported_busy: false,
            pending_model: None,
            inherited: None,
            _event_pump: Task::ready(()),
            preview: Preview::None,
            early: Early::default(),
            _preview_task: Task::ready(()),
            naming: Naming::NotYet,
            _naming_task: Task::ready(()),
            _start_task: Task::ready(()),
        }
    }

    /// A placeholder of a session on `host`.
    #[cfg(test)]
    pub fn idle_on_host(host: HostId) -> Self {
        Self {
            host: Some(host),
            ..Self::idle(PathBuf::from("/srv"), Mode::Code)
        }
    }

    pub fn is_idle(&self) -> bool {
        self.lifecycle == Lifecycle::Idle
    }

    // ---- reads ---------------------------------------------------------------

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The SSH host this session runs on (`None`: this computer).
    pub fn host(&self) -> Option<&HostId> {
        self.host.as_ref()
    }

    /// A Code session on this computer (its folder is a local path).
    pub fn is_local_code(&self) -> bool {
        self.host.is_none() && self.mode == Mode::Code
    }

    /// A Code session, here or on an SSH host: it has a project folder, so the terminal,
    /// diff and files panels and the file picker apply.
    pub fn has_folder_tools(&self) -> bool {
        self.mode == Mode::Code
    }

    /// Chat sessions have no tools and no project folder.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn lifecycle(&self) -> &Lifecycle {
        &self.lifecycle
    }

    pub fn transcript(&self) -> &Transcript {
        &self.transcript
    }

    /// The tokens-per-second pill's content at `now`.
    pub fn token_rate(&self, now: Instant, cx: &gpui_kit::App) -> Option<RateView> {
        let rates = cx.try_global::<super::token_rate::TokenRates>()?;
        self.rate.view(now, &rates.0)
    }

    pub fn is_running(&self) -> bool {
        self.transcript.is_running()
    }

    /// Whether a shell command (`!cmd`) is running.
    pub fn is_bash_running(&self) -> bool {
        self.transcript.is_bash_running()
    }

    /// Whether the conversation is being compacted, in a run or between runs.
    pub fn is_compacting(&self) -> bool {
        self.holds_prompts() || *self.transcript.status() == RunStatus::Compacting
    }

    /// Whether the transcript ends with the working row.
    pub fn shows_working(&self) -> bool {
        self.is_running() || self.is_compacting()
    }

    /// What the working row says.
    pub fn working_status(&self) -> RunStatus {
        if self.is_compacting() {
            RunStatus::Compacting
        } else {
            self.transcript.status().clone()
        }
    }

    /// Whether something visible goes on that Esc and the stop button end.
    pub fn can_stop(&self) -> bool {
        self.shows_working() || self.is_bash_running()
    }

    /// Whether this session has work in flight: a run, a shell command, compaction, prompts
    /// waiting to be sent, or a prompt pi accepted but has not started showing yet. A busy chat
    /// must stay alive when the user switches away, or its messages are lost.
    pub fn is_busy(&self) -> bool {
        matches!(self.lifecycle, Lifecycle::Starting | Lifecycle::Ready)
            && (self.agent_busy() || self.is_bash_running())
    }

    /// [`Self::is_busy`] without shell commands.
    fn agent_busy(&self) -> bool {
        self.shows_working()
            || self.sending.is_some()
            || self.awaiting_start
            || !self.outbox.is_empty()
    }

    pub fn model(&self) -> Option<&Model> {
        match (&self.state, &self.early.shown) {
            (Some(state), _) => state.model.as_ref(),
            (None, Some((model, _))) => model.as_ref(),
            (None, None) => None,
        }
    }

    pub fn models(&self) -> &[Model] {
        &self.models
    }

    /// The thinking level, as the one of [`Self::thinking_levels`] that does the same.
    pub fn thinking_level(&self) -> Option<ThinkingLevel> {
        let level = self
            .transcript
            .thinking_level()
            .or_else(|| self.state.as_ref().map(|state| state.thinking_level))
            .or_else(|| self.early.shown.as_ref().and_then(|(_, level)| *level))?;
        Some(self.shown_thinking_level(level))
    }

    /// The one of [`Self::thinking_levels`] that does the same as `level`.
    pub fn shown_thinking_level(&self, level: ThinkingLevel) -> ThinkingLevel {
        match self.model() {
            Some(model) => model.shown_thinking_level(level, &self.thinking_levels),
            None => level,
        }
    }

    /// The thinking levels that differ on the model.
    pub fn thinking_levels(&self) -> Vec<ThinkingLevel> {
        match self.model() {
            Some(model) => model.distinct_thinking_levels(&self.thinking_levels),
            None => self.thinking_levels.clone(),
        }
    }

    /// All levels pi accepts, also those that do the same as another on the model.
    pub fn accepted_thinking_levels(&self) -> &[ThinkingLevel] {
        &self.thinking_levels
    }

    pub fn commands(&self) -> &[SlashCommand] {
        &self.commands
    }

    pub fn widgets(&self) -> &BTreeMap<String, Vec<String>> {
        &self.widgets
    }

    pub fn extension_status(&self) -> &BTreeMap<String, String> {
        &self.extension_status
    }

    /// How this chat uses the web (as its pi extension reports).
    pub fn web_mode(&self) -> WebMode {
        self.extension_status
            .get(deepsearch::STATUS_KEY)
            .and_then(|text| WebMode::parse(text))
            .unwrap_or_default()
    }

    /// How much effort this chat's deep search makes.
    pub fn web_effort(&self) -> Effort {
        self.extension_status
            .get(deepsearch::EFFORT_KEY)
            .and_then(|text| Effort::parse(text))
            .unwrap_or_default()
    }

    /// What deep search is doing right now.
    pub fn web_progress(&self) -> Option<&str> {
        self.extension_status
            .get(deepsearch::PROGRESS_KEY)
            .map(String::as_str)
    }

    /// Whether this chat's pi loaded web search (it offers the `/websearch` command).
    pub fn web_search_available(&self) -> bool {
        self.mode == Mode::Chat
            && self
                .commands
                .iter()
                .any(|command| command.name == deepsearch::COMMAND)
    }

    /// Sets this chat's web mode through the extension's `/websearch` command; the extension
    /// remembers it in the session and confirms it with its status.
    pub fn set_web_mode(&mut self, mode: WebMode, cx: &mut Context<Self>) {
        if !self.web_search_available() {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        // Shown right away; the extension confirms it with its status, and a failed command
        // puts the previous mode back.
        let previous = self.web_mode();
        self.show_web_mode(mode, cx);
        if mode != WebMode::Off {
            crate::search_server::start_if_used(cx);
        }
        let command = format!("/{} {}", deepsearch::COMMAND, mode.as_str());
        cx.spawn(async move |this, cx| {
            let result = client
                .prompt(command, Vec::new(), Some(StreamingBehavior::FollowUp))
                .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(Disposition::Handled) => {}
                Ok(_) => {
                    // pi did not run the command (the extension is gone): it went to the model.
                    this.show_web_mode(previous, cx);
                    this.push_notice(
                        NoticeKind::Error,
                        "Web search is not available in this chat.",
                        cx,
                    );
                }
                Err(error) => {
                    this.show_web_mode(previous, cx);
                    this.report(&error, cx);
                }
            });
        })
        .detach();
    }

    /// Sets the effort of this chat's deep search, from the next message on.
    pub fn set_web_effort(&mut self, effort: Effort, cx: &mut Context<Self>) {
        if !self.web_search_available() || self.web_effort() == effort {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        // Shown right away, like the mode.
        let previous = self.web_effort();
        self.show_web_effort(effort, cx);
        let command = format!("/{} effort {}", deepsearch::COMMAND, effort.as_str());
        cx.spawn(async move |this, cx| {
            let result = client
                .prompt(command, Vec::new(), Some(StreamingBehavior::FollowUp))
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.show_web_effort(previous, cx);
                    this.report(&error, cx);
                }
            });
        })
        .detach();
    }

    fn show_web_effort(&mut self, effort: Effort, cx: &mut Context<Self>) {
        self.extension_status
            .insert(deepsearch::EFFORT_KEY.into(), effort.as_str().into());
        cx.emit(SessionEvent::MetaChanged);
        cx.notify();
    }

    /// As if pi had loaded web search and set this.
    #[cfg(test)]
    pub fn set_web_for_tests(&mut self, mode: WebMode, effort: Effort, cx: &mut Context<Self>) {
        self.commands = vec![
            serde_json::from_value(serde_json::json!({
                "name": deepsearch::COMMAND, "source": "extension",
            }))
            .unwrap(),
        ];
        self.show_web_mode(mode, cx);
        self.show_web_effort(effort, cx);
    }

    pub fn session_file(&self) -> Option<PathBuf> {
        self.state
            .as_ref()
            .and_then(|state| state.session_file.as_ref().map(PathBuf::from))
            .or_else(|| self.resumed_from.clone())
    }

    pub fn title(&self) -> Option<String> {
        self.transcript
            .session_name()
            .map(str::to_owned)
            .or_else(|| {
                self.state
                    .as_ref()
                    .and_then(|state| state.session_name.clone())
            })
            .filter(|name| !name.is_empty() && name != "New thread")
    }

    /// The name to show: pi's session name, else the first prompt's first line.
    pub fn display_title(&self) -> Option<String> {
        self.title().or_else(|| {
            self.transcript
                .items()
                .iter()
                .find_map(|item| match &item.kind {
                    transcript::ItemKind::User(user) => {
                        user.title().map(|line| line.chars().take(80).collect())
                    }
                    _ => None,
                })
        })
    }

    pub fn is_empty(&self) -> bool {
        self.transcript.items().is_empty() && self.outbox.is_empty()
    }

    /// Whether nothing was sent in this conversation yet. pi writes a session's file with its
    /// first message, so a new one is not in the sidebar and has no file to resume.
    pub fn is_new(&self) -> bool {
        // A branch that starts before the first message is new too, unlike what it came from.
        let resumed = self.resumed_from.is_some() && self.session_file() == self.resumed_from;
        !resumed
            && !self.is_idle()
            && !self.is_busy()
            && self.outbox.is_empty()
            && self.transcript.items().iter().all(|item| {
                use transcript::ItemKind::{Bash, Notice};
                matches!(item.kind, Bash { .. } | Notice(_))
            })
    }

    /// Context-window usage as `(used tokens, window)`, live while streaming.
    pub fn context_usage(&self) -> Option<(u64, u64)> {
        let window = self
            .stats
            .as_ref()
            .and_then(|stats| stats.context_usage.as_ref())
            .map(|usage| usage.context_window)
            .filter(|window| *window > 0)
            .or_else(|| self.model().map(|model| model.context_window))?;
        let live = self
            .is_running()
            .then(|| self.transcript.last_usage())
            .flatten();
        let used = match live {
            Some(usage) => usage.total_tokens,
            None => self
                .stats
                .as_ref()
                .and_then(|stats| stats.context_usage.as_ref())
                .and_then(|usage| usage.tokens)
                .unwrap_or(0),
        };
        Some((used, window))
    }

    pub fn stats(&self) -> Option<&SessionStats> {
        self.stats.as_ref()
    }

    pub fn total_cost(&self) -> Option<f64> {
        self.stats.as_ref().map(|stats| stats.cost)
    }

    // ---- commands ------------------------------------------------------------

    /// Submits the composer text. While the agent works, Enter steers and
    /// Alt+Enter queues a follow-up — exactly like pi's terminal.
    pub fn submit(
        &mut self,
        text: String,
        images: Vec<ImageContent>,
        follow_up: bool,
        cx: &mut Context<Self>,
    ) {
        // Only a visible run can be steered; anything sent before it shows (typed while pi
        // starts, or right after another message) waits its turn as a follow-up.
        let follow_up = follow_up || !self.is_running();
        self.outbox.push_back(PendingPrompt {
            text,
            images,
            follow_up,
        });
        self.flush_outbox(cx);
        self.report_busy(cx);
        cx.notify();
    }

    /// Whether nothing was said in this conversation yet: a new session, no prompt shown.
    fn is_first_prompt(&self) -> bool {
        self.resumed_from.is_none()
            && !self
                .transcript
                .items()
                .iter()
                .any(|item| matches!(item.kind, transcript::ItemKind::User(_)))
    }

    /// Names the conversation after the prompt pi just accepted, if it is the first one.
    /// Later prompts never do.
    fn consider_naming(&mut self, sent: &Sending, cx: &mut Context<Self>) {
        if self.naming == Naming::NotYet {
            self.naming = Naming::Considered;
            if sent.first {
                self.start_naming(&about(&sent.prompt.text), cx);
            }
        }
    }

    /// Starts naming the conversation after its first `message`, if the user turned automatic
    /// names on (see [`crate::naming`]). Naming runs beside the conversation, so a slow namer
    /// never holds it up.
    fn start_naming(&mut self, message: &str, cx: &mut Context<Self>) {
        let Some(settings) = cx.try_global::<crate::settings::AppSettings>() else {
            return;
        };
        if !settings.auto_names || self.title().is_some() || !titles::worth_naming(message) {
            return;
        }
        let namer = settings.namer;
        if let Some(title) = titles::as_is(message) {
            // Already what the row shows: saved, but not typed out again.
            self.apply_name(title, false, cx);
            return;
        }
        match namer {
            crate::naming::Namer::Local => match crate::naming::local_job(message, cx) {
                Some(job) => self.run_naming(job, cx),
                None => log::info!("no local naming model is installed; not naming"),
            },
            crate::naming::Namer::Conversation => {
                if let Some(job) = self.pi_naming_job(message.to_owned(), cx) {
                    self.run_naming(job, cx);
                }
            }
        }
    }

    /// Naming with the conversation's model, through a separate `pi --print` started the
    /// way this conversation's pi was (on its SSH host for a remote conversation).
    fn pi_naming_job(&self, message: String, cx: &gpui_kit::App) -> Option<crate::naming::Job> {
        let model = self.model()?;
        let host = saved_host(self.host.as_ref(), cx);
        if self.host.is_some() && host.is_none() {
            return None;
        }
        let config = cx.try_global::<AppConfig>()?;
        Some(crate::naming::Job::Pi {
            launch: Box::new(crate::naming::PiLaunch::new(config, self.cwd.clone(), host)),
            provider: model.provider.clone(),
            model: model.id.clone(),
            message,
        })
    }

    fn run_naming(&mut self, job: crate::naming::Job, cx: &mut Context<Self>) {
        self._naming_task = cx.spawn(async move |this, cx| {
            if let Some(title) = crate::naming::run(job).await {
                let _ = this.update(cx, |this, cx| this.apply_name(title, true, cx));
            }
        });
    }

    /// Saves an automatic name through pi, unless the conversation got a name meanwhile
    /// (Rename, `/name`): those are never replaced. `announce` has the sidebar type it out.
    fn apply_name(&mut self, title: String, announce: bool, cx: &mut Context<Self>) {
        if self.title().is_some() || self.client.is_none() {
            return;
        }
        self.rename(title.clone(), cx);
        if announce {
            crate::naming::announce(self.host.clone(), self.session_file(), title, cx);
        }
    }

    /// Whether prompts wait for a compaction between runs to end: pi refuses them while it
    /// lasts, and one sent just before it would run into it.
    fn holds_prompts(&self) -> bool {
        self.compaction.sent || self.compaction.running
    }

    /// Whether prompts wait because the session's model is a suggested local model that is
    /// starting; starts it when it does not run, and sends them once it answers.
    fn waits_for_local_model(&mut self, cx: &mut Context<Self>) -> bool {
        match self.local_model {
            Some(true) => return true,
            Some(false) => return false,
            None => {}
        }
        let Some(provider) = self.model().map(|model| model.provider.clone()) else {
            return false;
        };
        let Some(started) = crate::local_models::ensure_running(&provider, cx) else {
            return false;
        };
        self.local_model = Some(true);
        cx.spawn(async move |this, cx| {
            let ok = started.await;
            let _ = this.update(cx, |this, cx| {
                // Once it runs, later prompts check again (it may stop); after a failure the
                // waiting prompts go to pi, which reports the unreachable server.
                this.local_model = (!ok).then_some(false);
                this.flush_outbox(cx);
                this.local_model = None;
            });
        })
        .detach();
        true
    }

    /// Sends the next waiting prompt once pi is ready and has answered the previous one.
    fn flush_outbox(&mut self, cx: &mut Context<Self>) {
        if self.sending.is_some() || self.lifecycle != Lifecycle::Ready || self.holds_prompts() {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        if self.outbox.is_empty() || self.waits_for_local_model(cx) {
            return;
        }
        let Some(prompt) = self.outbox.pop_front() else {
            return;
        };
        // pi ignores the behaviour while idle and queues the prompt while a run is active.
        let behavior = Some(if prompt.follow_up {
            StreamingBehavior::FollowUp
        } else {
            StreamingBehavior::Steer
        });
        if self.mode == Mode::Chat && self.web_mode() != WebMode::Off {
            crate::search_server::start_if_used(cx);
        }
        let (text, images) = (prompt.text.clone(), prompt.images.clone());
        self.queued.sending(&text, prompt.follow_up);
        self.sending = Some(Sending {
            prompt,
            runs_before: self.runs_started,
            first: self.is_first_prompt(),
        });
        cx.spawn(async move |this, cx| {
            let result = client.prompt(text, images, behavior).await;
            if matches!(result, Err(RpcError::Disconnected)) {
                // Events pi sent before it stopped may still wait to be handled, and they say
                // whether the prompt's run started. The disconnect comes after them and gives
                // the prompt back; this is for when it doesn't come.
                cx.background_executor().timer(EVENT_GRACE).await;
            }
            let _ = this.update(cx, |this, cx| this.prompt_answered(result, cx));
        })
        .detach();
    }

    /// pi accepted or refused the prompt being sent.
    fn prompt_answered(&mut self, result: Result<Disposition, RpcError>, cx: &mut Context<Self>) {
        // pi stopped, and the prompt was given back already.
        let Some(sent) = self.sending.take() else {
            return;
        };
        if !matches!(result, Ok(Disposition::Queued)) {
            self.queued.not_queued();
        }
        match result {
            Ok(Disposition::Started) => {
                // Unless its run already showed up: then the transcript tracks it.
                self.awaiting_start = self.runs_started == sent.runs_before;
                self.consider_naming(&sent, cx);
            }
            // Joins the active run, which the transcript tracks.
            Ok(Disposition::Queued) => {
                self.consider_naming(&sent, cx);
                self.queued.accepted(sent.prompt.images);
            }
            // A command: nothing was said.
            Ok(Disposition::Handled) => {}
            // pi took the prompt, but its answer could not be read.
            Err(error @ RpcError::Decode { .. }) => self.report(&error, cx),
            Err(error) => {
                // The ones behind it go back too: sent now, they would be refused as well, or
                // be answered without it.
                self.outbox.push_front(sent.prompt);
                self.return_outbox(cx);
                self.report(&error, cx);
            }
        }
        self.flush_outbox(cx);
        self.report_busy(cx);
        cx.notify();
    }

    /// Emits [`SessionEvent::BusyChanged`] when [`Self::is_busy`] changed since last time.
    fn report_busy(&mut self, cx: &mut Context<Self>) {
        let busy = self.is_busy();
        if busy != self.reported_busy {
            self.reported_busy = busy;
            cx.emit(SessionEvent::BusyChanged);
        }
    }

    /// Removes the prompts waiting to be sent, returning their texts and images.
    fn take_outbox(&mut self, cx: &mut Context<Self>) -> (Vec<String>, Vec<ImageContent>) {
        if self.outbox.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let mut texts = Vec::new();
        let mut images = Vec::new();
        for prompt in self.outbox.drain(..) {
            if !prompt.text.is_empty() {
                texts.push(prompt.text);
            }
            images.extend(prompt.images);
        }
        self.report_busy(cx);
        cx.notify();
        (texts, images)
    }

    /// Gives prompts that can no longer be sent (pi exited, or refused one), or should not be
    /// (Esc), back to the composer.
    fn return_outbox(&mut self, cx: &mut Context<Self>) {
        let (texts, images) = self.take_outbox(cx);
        restore(&texts, images, cx);
    }

    /// Prompts typed but not sent to pi yet (pi is starting, or busy with the previous one).
    pub fn pending_prompts(&self) -> impl Iterator<Item = (&str, usize, bool)> {
        self.outbox
            .iter()
            .map(|prompt| (prompt.text.as_str(), prompt.images.len(), prompt.follow_up))
    }

    /// Cancels the `index`th prompt waiting to be sent and returns it to the composer.
    pub fn cancel_pending(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(prompt) = self.outbox.remove(index) {
            cx.emit(SessionEvent::RestoreToEditor {
                text: prompt.text,
                images: prompt.images,
                replace: false,
            });
            self.report_busy(cx);
            cx.notify();
        }
    }

    /// Esc: restore queued messages (pi's queue and prompts not sent yet) to the editor,
    /// then abort the run. While the agent rests, stop the shell commands instead (pi's
    /// terminal has the same order).
    pub fn interrupt(&mut self, cx: &mut Context<Self>) {
        if !self.agent_busy() && self.is_bash_running() {
            self.run(cx, async move |client| client.abort_bash().await);
            return;
        }
        // Prompts still waiting here would otherwise be sent right after the abort. They are
        // back in the editor before anything can let this chat go (it may not be busy now);
        // pi's queue goes before them.
        self.return_outbox(cx);
        let Some(client) = self.client.clone() else {
            return;
        };
        let queue = self.take_queue(client.clone(), cx);
        cx.spawn(async move |this, cx| {
            let (texts, images) = queue.await;
            let _ = this.update(cx, |_, cx| restore(&texts, images, cx));
            if let Err(error) = client.abort().await {
                let _ = this.update(cx, |this, cx| this.report(&error, cx));
            }
        })
        .detach();
    }

    /// Alt+Up: move queued messages back into the editor without aborting.
    pub fn dequeue(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let queue = self.take_queue(client, cx);
        cx.spawn(async move |this, cx| {
            let (texts, images) = queue.await;
            let _ = this.update(cx, |_, cx| restore(&texts, images, cx));
        })
        .detach();
    }

    /// Takes the messages out of pi's queue: their texts and images.
    fn take_queue(
        &mut self,
        client: PiClient,
        cx: &mut Context<Self>,
    ) -> Task<(Vec<String>, Vec<ImageContent>)> {
        self.clearing_queue += 1;
        cx.spawn(async move |this, cx| {
            let cleared = client.clear_queue().await;
            this.update(cx, |this, _| {
                this.clearing_queue -= 1;
                let Ok(cleared) = cleared else {
                    return (Vec::new(), Vec::new());
                };
                let images = this.queued.take(&cleared.steering, &cleared.follow_up);
                let texts = cleared.steering.into_iter().chain(cleared.follow_up);
                // One with only images has no text.
                (texts.filter(|text| !text.is_empty()).collect(), images)
            })
            .unwrap_or_default()
        })
    }

    pub fn set_model(&mut self, model: &Model, cx: &mut Context<Self>) {
        let (provider, id) = (model.provider.clone(), model.id.clone());
        let Some(client) = self.client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = client.set_model(provider, id).await;
            let levels = client.get_available_thinking_levels().await;
            let state = client.get_state().await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(_) => {
                        if let Ok(levels) = levels {
                            this.thinking_levels = levels;
                        }
                        if let Ok(state) = state {
                            this.state = Some(state);
                        }
                        this.remember(false, cx);
                    }
                    Err(error) => this.report(&error, cx),
                }
                this.refresh_stats(cx);
                cx.emit(SessionEvent::MetaChanged);
                cx.notify();
            });
        })
        .detach();
    }

    /// Switches to `provider/model_id` as soon as pi has started and listed its models (right
    /// away if it already has).
    pub fn select_model_when_ready(
        &mut self,
        provider: String,
        model_id: String,
        cx: &mut Context<Self>,
    ) {
        self.pending_model = Some((provider, model_id));
        if self.lifecycle == Lifecycle::Ready {
            self.apply_pending_model(cx);
        }
    }

    fn apply_pending_model(&mut self, cx: &mut Context<Self>) {
        let Some((provider, id)) = self.pending_model.take() else {
            return;
        };
        match self
            .models
            .iter()
            .find(|model| model.provider == provider && model.id == id)
            .cloned()
        {
            Some(model) => self.set_model(&model, cx),
            None => self.push_notice(
                NoticeKind::Error,
                format!(
                    "pi did not load {provider}/{id}. Check the provider's base URL and API key \
                     (pi lists only models it can authenticate)."
                ),
                cx,
            ),
        }
    }

    pub fn set_thinking_level(&mut self, level: ThinkingLevel, cx: &mut Context<Self>) {
        if let Some(state) = &mut self.state {
            state.thinking_level = level;
            self.remember(false, cx);
        } else if let Some((_, early)) = &mut self.early.shown {
            *early = Some(level);
        }
        cx.notify();
        self.run(cx, async move |client| {
            client.set_thinking_level(level).await
        });
    }

    /// Shift+Tab: next available thinking level.
    pub fn cycle_thinking_level(&mut self, cx: &mut Context<Self>) {
        let levels = self.thinking_levels();
        let (Some(current), false) = (self.thinking_level(), levels.is_empty()) else {
            return;
        };
        let index = levels
            .iter()
            .position(|level| *level == current)
            .unwrap_or(0);
        let next = levels[(index + 1) % levels.len()];
        self.set_thinking_level(next, cx);
    }

    /// Compacts the conversation (`/compact`), aborting a run like pi's terminal does.
    /// Prompts submitted meanwhile wait.
    pub fn compact(&mut self, instructions: Option<String>, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            self.push_notice(NoticeKind::Error, "pi is not running.", cx);
            return;
        };
        if self.holds_prompts() {
            let text = "The conversation is already being compacted.";
            self.push_notice(NoticeKind::Info, text, cx);
            return;
        }
        self.compaction.sent = true;
        let ended = self.compaction.ended;
        cx.spawn(async move |this, cx| {
            let result = client.compact(instructions).await;
            let _ = this.update(cx, |this, cx| {
                this.compaction.sent = false;
                this.compaction.running = false;
                this.flush_outbox(cx);
                this.transcript_changed(cx);
            });
            match result {
                // A lost connection says so itself.
                Ok(()) | Err(RpcError::Disconnected) => return,
                Err(RpcError::Rejected { .. }) => {
                    // pi tells how a compaction ended with an event, which may still wait to
                    // be handled.
                    cx.background_executor().timer(EVENT_GRACE).await;
                }
                Err(_) => {}
            }
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result
                    && this.compaction.ended == ended
                {
                    this.report(&error, cx);
                }
            });
        })
        .detach();
        self.transcript_changed(cx);
    }

    pub fn rename(&mut self, name: String, cx: &mut Context<Self>) {
        self.transcript.set_session_name(Some(name.clone()));
        cx.emit(SessionEvent::SessionFileChanged);
        self.run(cx, async move |client| client.set_session_name(name).await);
    }

    /// [`Self::rename`] for a pi that may still be starting; tells when pi has saved the name.
    pub fn rename_when_ready(
        &mut self,
        name: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let (changed, changes) = async_channel::bounded(1);
        let observed = cx.observe_self(move |_, _| {
            let _ = changed.try_send(());
        });
        cx.spawn(async move |this, cx| {
            let _observed = observed;
            let gone = |_| "pi was stopped.".to_owned();
            let client = loop {
                let client = this
                    .update(cx, |this, _| match &this.lifecycle {
                        Lifecycle::Ready => Ok(this.client.clone()),
                        Lifecycle::Exited { message } => Err(message.clone()),
                        Lifecycle::Idle | Lifecycle::Starting => Ok(None),
                    })
                    .map_err(gone)??;
                match client {
                    Some(client) => break client,
                    None => changes.recv().await.map_err(|error| error.to_string())?,
                }
            };
            client
                .set_session_name(name.clone())
                .await
                .map_err(|error| match error {
                    RpcError::Rejected { message, .. } => message,
                    other => other.to_string(),
                })?;
            this.update(cx, |this, cx| {
                this.transcript.set_session_name(Some(name));
                cx.emit(SessionEvent::SessionFileChanged);
                cx.notify();
            })
            .map_err(gone)
        })
    }

    /// `!cmd` / `!!cmd`: run a shell command through pi (added to context unless `!!`). Its
    /// row shows its output as it comes.
    pub fn run_bash(
        &mut self,
        command: String,
        exclude_from_context: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client.clone() else {
            self.push_notice(NoticeKind::Error, "pi is not running.", cx);
            return;
        };
        let id = client.next_id();
        self.transcript
            .start_bash(&id, command.clone(), Instant::now());
        cx.spawn(async move |this, cx| {
            let result = client.bash(id.clone(), command, exclude_from_context).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(result) => this.transcript.finish_bash(&id, result),
                    // The disconnect says why.
                    Err(RpcError::Disconnected) => {
                        this.transcript.stop_bash(&id, BashStatus::Cancelled);
                    }
                    Err(error) => {
                        this.transcript.stop_bash(&id, BashStatus::Failed);
                        this.report(&error, cx);
                    }
                }
                this.transcript_changed(cx);
            });
        })
        .detach();
        self.transcript_changed(cx);
    }

    fn transcript_changed(&mut self, cx: &mut Context<Self>) {
        self.report_busy(cx);
        cx.emit(SessionEvent::TranscriptChanged);
        cx.notify();
    }

    pub fn answer_dialog(&mut self, id: &str, answer: &ExtensionUiAnswer) {
        if let Some(client) = &self.client
            && let Err(error) = client.answer_extension_ui(id, answer)
        {
            log::warn!("could not answer extension dialog: {error}");
        }
    }

    pub fn push_notice(
        &mut self,
        kind: NoticeKind,
        text: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.transcript.push_notice(kind, text);
        cx.emit(SessionEvent::TranscriptChanged);
        cx.notify();
    }

    // ---- history ---------------------------------------------------------------

    /// Branches the conversation at a user message: pi continues in a new session file that
    /// holds everything before that message (the original stays as it was). `from_end` counts
    /// the transcript's user messages from the last one (0); `text` is that message, to check
    /// the match. Then `after` decides what happens with the message.
    pub fn fork_at(
        &mut self,
        from_end: usize,
        text: String,
        after: AfterFork,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client.clone() else {
            return;
        };
        if self.is_busy() {
            self.push_notice(
                NoticeKind::Info,
                "Wait for the current response to finish before branching.",
                cx,
            );
            return;
        }
        cx.spawn(async move |this, cx| {
            let result = async {
                let messages = client.get_fork_messages().await?;
                let Some(entry_id) = fork_entry(&messages, from_end, &text) else {
                    return Ok(None);
                };
                // The transcript keeps only an image count; Edit and Retry need the images.
                let images = if after == AfterFork::Keep {
                    Vec::new()
                } else {
                    client
                        .get_messages()
                        .await
                        .map_or_default(|messages| user_images(&messages, from_end, &text))
                };
                Ok(client.fork(entry_id).await?.map(|text| (text, images)))
            }
            .await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(Some((text, images))) => this.reload(Some((text, images, after)), cx),
                Ok(None) => this.push_notice(
                    NoticeKind::Warning,
                    "pi could not branch from this message (it may be before a compaction).",
                    cx,
                ),
                Err(error) => this.report(&error, cx),
            });
        })
        .detach();
    }

    /// pi's user messages that a branch can start from, oldest first (for `/fork`).
    pub fn fork_messages(
        &self,
        cx: &mut Context<Self>,
    ) -> Task<Result<Vec<pi_rpc::protocol::ForkMessage>, RpcError>> {
        let client = self.client.clone();
        cx.background_executor().spawn(async move {
            match client {
                Some(client) => client.get_fork_messages().await,
                None => Err(RpcError::Disconnected),
            }
        })
    }

    /// Branches at pi's fork entry `entry_id` (from [`Self::fork_messages`]).
    pub fn fork_entry_id(&mut self, entry_id: String, after: AfterFork, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = client.fork(entry_id).await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(Some(text)) => this.reload(Some((text, Vec::new(), after)), cx),
                Ok(None) => {}
                Err(error) => this.report(&error, cx),
            });
        })
        .detach();
    }

    /// Continues in a copy of the whole conversation (a new session file).
    pub fn clone_session(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        // Replacing the transcript mid-run would lose track of the run still streaming.
        if self.is_busy() {
            self.push_notice(
                NoticeKind::Info,
                "Wait for the current response to finish before cloning.",
                cx,
            );
            return;
        }
        cx.spawn(async move |this, cx| {
            let result = client.clone_session().await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(true) => this.reload(None, cx),
                Ok(false) => {}
                Err(error) => this.report(&error, cx),
            });
        })
        .detach();
    }

    /// Writes the file of a side chat of this conversation: everything it holds right now,
    /// also what a run that is still going has not saved yet (see
    /// [`pi_sessions::side_chat`]). This conversation goes on undisturbed.
    pub fn side_chat(&self, cx: &mut Context<Self>) -> Task<Result<SideChatStart, String>> {
        if self.is_empty() && self.in_flight.is_empty() {
            return Task::ready(Err(
                "There is nothing in this conversation to take into a side chat yet.".to_owned(),
            ));
        }
        let Some(parent) = self.session_file() else {
            return Task::ready(Err(
                "This conversation has no session file to start a side chat from.".to_owned(),
            ));
        };
        let Some(place) = crate::pi_place::PiPlace::for_host(self.host.as_ref(), cx) else {
            return Task::ready(Err(
                "The side chat cannot be written until the connection is ready.".to_owned(),
            ));
        };
        let state = self.state.as_ref();
        let model = state.and_then(|state| state.model.as_ref());
        let now = std::time::SystemTime::now();
        let now_ms = now
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|since| i64::try_from(since.as_millis()).ok())
            .unwrap_or_default();
        // The moment that counts: what was on the screen when it was asked for.
        let tail = self
            .in_flight
            .messages(model.map(|model| model.api.as_str()), now_ms);
        let start = SideChatStart {
            cwd: self.cwd.clone(),
            file: PathBuf::new(),
            host: self.host.clone(),
            model: model.map(|model| (model.provider.clone(), model.id.clone())),
            thinking_level: state.map(|state| state.thinking_level),
            inherited_prompts: 0,
        };
        let client = self.client.clone();
        cx.background_executor().spawn(async move {
            let parent_text = parent.to_string_lossy().into_owned();
            let text = place.read(&parent_text)?;
            let messages;
            let source = match &text {
                Some(text) => pi_sessions::side_chat::Source::File(text),
                // pi writes the file with the first reply; until then it has the messages.
                None => {
                    let client = client.ok_or("pi is not running in this conversation.")?;
                    messages = client
                        .get_messages_raw()
                        .await
                        .map_err(|error| error.to_string())?;
                    pi_sessions::side_chat::Source::Messages(&messages)
                }
            };
            let file = pi_sessions::side_chat::build(source, &parent, &start.cwd, &tail, now)?;
            let path = pi_sessions::side_chat::path_beside(&parent, &file.file_name);
            place.write(&path.to_string_lossy(), &file.text, false)?;
            Ok(SideChatStart {
                file: path,
                inherited_prompts: file.prompts,
                ..start
            })
        })
    }

    /// Takes over the model and thinking level of the conversation this side chat came from,
    /// once pi has started (pi restores them from the file; this covers what it cannot).
    pub fn inherit(
        &mut self,
        model: Option<(String, String)>,
        thinking_level: Option<ThinkingLevel>,
        cx: &mut Context<Self>,
    ) {
        self.inherited = Some((model, thinking_level));
        if self.lifecycle == Lifecycle::Ready {
            self.apply_inherited(cx);
        }
    }

    fn apply_inherited(&mut self, cx: &mut Context<Self>) {
        let (Some((model, level)), Some(client)) = (self.inherited.take(), self.client.clone())
        else {
            return;
        };
        let current = self.state.as_ref().and_then(|state| state.model.as_ref());
        let model = model
            .filter(|(provider, id)| {
                current.is_none_or(|current| current.provider != *provider || current.id != *id)
            })
            .and_then(|(provider, id)| {
                self.models
                    .iter()
                    .any(|model| model.provider == provider && model.id == id)
                    .then_some((provider, id))
            });
        let level = level.filter(|level| self.thinking_level() != Some(*level));
        if model.is_none() && level.is_none() {
            return;
        }
        cx.spawn(async move |this, cx| {
            if let Some((provider, id)) = model
                && let Err(error) = client.set_model(provider, id).await
            {
                log::warn!("side chat: could not take over the model: {error}");
            }
            if let Some(level) = level
                && let Err(error) = client.set_thinking_level(level).await
            {
                log::warn!("side chat: could not take over the thinking level: {error}");
            }
            let levels = client.get_available_thinking_levels().await;
            let state = client.get_state().await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(levels) = levels {
                    this.thinking_levels = levels;
                }
                if let Ok(state) = state {
                    this.state = Some(state);
                }
                this.remember(false, cx);
                cx.emit(SessionEvent::MetaChanged);
                cx.notify();
            });
        })
        .detach();
    }

    /// Exports the conversation as HTML (to `path`, or next to the session) and opens it.
    pub fn export_html(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = client.export_html(path).await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(path) => {
                    this.push_notice(NoticeKind::Info, format!("Exported to {path}"), cx);
                    if this.host.is_none() {
                        cx.open_with_system(Path::new(&path));
                    }
                }
                Err(error) => this.report(&error, cx),
            });
        })
        .detach();
    }

    /// Uploads the conversation (as HTML) to a secret GitHub gist with `gh`, like pi's
    /// terminal `/share`, and copies the viewer link.
    pub fn share(&mut self, cx: &mut Context<Self>) {
        if self.host.is_some() {
            self.push_notice(
                NoticeKind::Info,
                "Sharing works for sessions on this computer; use /export on the host instead.",
                cx,
            );
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        let env = cx.global::<AppConfig>().child_env.clone();
        let dir = std::env::temp_dir().join(format!("pistachio-share-{}", std::process::id()));
        let file = dir.join("session.html");
        self.push_notice(NoticeKind::Info, "Creating a secret gist…", cx);
        cx.spawn(async move |this, cx| {
            let _ = std::fs::create_dir_all(&dir);
            let result = match client
                .export_html(Some(file.to_string_lossy().into_owned()))
                .await
            {
                Ok(path) => {
                    cx.background_executor()
                        .spawn(async move { create_gist(&path, &env) })
                        .await
                }
                Err(error) => Err(error.to_string()),
            };
            let _ = std::fs::remove_dir_all(&dir);
            let _ = this.update(cx, |this, cx| match result {
                Ok((viewer, gist)) => {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(viewer.clone()));
                    this.push_notice(
                        NoticeKind::Info,
                        format!("Shared (link copied): {viewer}\nGist: {gist}"),
                        cx,
                    );
                }
                Err(error) => this.push_notice(NoticeKind::Error, error, cx),
            });
        })
        .detach();
    }

    /// Reads the state and history again after pi switched session files in this process,
    /// then handles the message a fork started from.
    fn reload(
        &mut self,
        forked: Option<(String, Vec<ImageContent>, AfterFork)>,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let state = client.get_state().await;
            let messages = client.get_messages().await;
            let stats = client.get_session_stats().await.ok();
            let _ = this.update(cx, |this, cx| {
                if let Ok(state) = state {
                    this.state = Some(state);
                }
                match messages {
                    Ok(messages) => {
                        this.transcript = Transcript::from_messages(&messages);
                        let name = this.state.as_ref().and_then(|s| s.session_name.clone());
                        this.transcript.set_session_name(name);
                    }
                    Err(error) => this.report(&error, cx),
                }
                this.stats = stats;
                // Another session in pi: the old one's queue and compaction are over.
                this.queued.clear();
                this.compaction.running = false;
                cx.emit(SessionEvent::Reloaded);
                cx.emit(SessionEvent::SessionFileChanged);
                cx.emit(SessionEvent::MetaChanged);
                match forked {
                    Some((text, images, AfterFork::Edit)) => {
                        cx.emit(SessionEvent::RestoreToEditor {
                            text,
                            images,
                            replace: true,
                        });
                    }
                    Some((text, images, AfterFork::Resend)) => {
                        // pi adds its notes on the pictures again.
                        this.submit(prompt::split(&text).as_typed(), images, false, cx);
                    }
                    Some((_, _, AfterFork::Keep)) | None => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn show_web_mode(&mut self, mode: WebMode, cx: &mut Context<Self>) {
        if mode == WebMode::Off {
            self.extension_status.remove(deepsearch::STATUS_KEY);
        } else {
            self.extension_status
                .insert(deepsearch::STATUS_KEY.into(), mode.as_str().into());
        }
        cx.emit(SessionEvent::MetaChanged);
        cx.notify();
    }

    // ---- internals -----------------------------------------------------------

    /// Runs a command, surfacing failures in the transcript.
    fn run<F>(&mut self, cx: &mut Context<Self>, command: F)
    where
        F: AsyncFnOnce(PiClient) -> Result<(), RpcError> + 'static,
    {
        let Some(client) = self.client.clone() else {
            self.push_notice(NoticeKind::Error, "pi is not running.", cx);
            return;
        };
        cx.spawn(async move |this, cx| {
            if let Err(error) = command(client).await {
                let _ = this.update(cx, |this, cx| this.report(&error, cx));
            }
        })
        .detach();
    }

    fn report(&mut self, error: &RpcError, cx: &mut Context<Self>) {
        log::warn!("pi command failed: {error}");
        let text = match error {
            RpcError::Rejected { message, .. } => message.clone(),
            other => other.to_string(),
        };
        self.push_notice(NoticeKind::Error, text, cx);
    }

    /// Shows what pi said about this conversation's composer last time (its model, thinking
    /// level, context usage, web search mode, its folder's commands) until pi answers again.
    fn recall_early(&mut self, cx: &mut Context<Self>) {
        if self.host.is_some() {
            return;
        }
        let Some(early) = cx
            .try_global::<Recall>()
            .and_then(|recall| recall.early(&self.cwd, self.resumed_from.as_deref()))
        else {
            return;
        };
        self.models = early.models;
        self.commands = early.commands;
        // A conversation pi never answered for here: its folder's defaults until the preview
        // has read what the conversation itself selected (see `guess_early`).
        let exact = early.snapshot.is_some();
        let Some(snapshot) = early.snapshot.or(early.defaults) else {
            return;
        };
        self.thinking_levels = snapshot.thinking_levels;
        self.stats = snapshot.stats.filter(|_| exact);
        self.extension_status = snapshot.status;
        self.early.shown = Some((
            snapshot
                .model
                .filter(|_| exact || self.resumed_from.is_none()),
            snapshot.thinking_level,
        ));
        self.early.exact = exact;
    }

    /// For a resumed conversation pi never answered for here: the model, thinking level and
    /// web search mode its file selected, as pi restores them (a model only when pi listed
    /// it before, so it is known to be usable).
    fn guess_early(&mut self, selection: &pi_sessions::branch::Selection) {
        if self.early.exact || self.state.is_some() {
            return;
        }
        if self.mode == Mode::Chat && !self.early.web_reported {
            self.extension_status = web_status(&selection.custom, &self.extension_status);
        }
        let model = selection.model.as_ref().and_then(|(provider, id)| {
            self.models
                .iter()
                .find(|model| model.provider == *provider && model.id == *id)
                .cloned()
        });
        let Some(model) = model else {
            return;
        };
        let defaults = self.early.shown.as_ref().and_then(|(_, level)| *level);
        let level = selection
            .thinking_level
            .as_deref()
            .and_then(|level| serde_json::from_value(serde_json::json!(level)).ok())
            .or(defaults)
            .map(|level| {
                if model.reasoning {
                    level
                } else {
                    ThinkingLevel::Off
                }
            });
        self.early.shown = Some((Some(model), level));
    }

    /// Remembers what pi says about this conversation's composer, to show it at once when
    /// the conversation (or, when `new`, a new one in this folder) is opened again.
    fn remember(&self, new: bool, cx: &mut Context<Self>) {
        let (None, Some(state)) = (&self.host, &self.state) else {
            return;
        };
        let snapshot = Snapshot {
            model: state.model.clone(),
            thinking_level: Some(state.thinking_level),
            thinking_levels: self.thinking_levels.clone(),
            stats: self.stats.clone(),
            status: self
                .extension_status
                .iter()
                .filter(|(key, _)| RECALLED_STATUS.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        };
        // A new conversation's file has nothing in it until the first message.
        let file = self.session_file().filter(|_| !self.is_new());
        Recall::update(cx, |recall| {
            recall.remember(
                &self.cwd,
                file.as_deref(),
                new,
                &self.models,
                &self.commands,
                snapshot,
            )
        });
    }

    /// Shows a resumed local session's history right away, read from its file on the
    /// background executor, instead of waiting a second or so for pi to start and send it.
    /// pi's history replaces the preview when it arrives, unless they are the same.
    fn show_preview(&mut self, cx: &mut Context<Self>) {
        let (None, Some(file)) = (&self.host, self.resumed_from.clone()) else {
            return;
        };
        // Read right away when it takes a few milliseconds, so the conversation's first
        // frame already shows its history: read later, that frame would be empty.
        let small = std::fs::metadata(&file).is_ok_and(|meta| meta.len() <= SYNC_PREVIEW_BYTES);
        if small {
            let (messages, selection) = read_preview(&file);
            self.apply_preview(messages, selection, cx);
            return;
        }
        self.preview = Preview::Reading;
        self._preview_task = cx.spawn(async move |this, cx| {
            let (messages, selection) = cx
                .background_executor()
                .spawn(async move { read_preview(&file) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.preview == Preview::Reading {
                    this.preview = Preview::None;
                }
                this.apply_preview(messages, selection, cx);
                cx.emit(SessionEvent::MetaChanged);
                cx.emit(SessionEvent::Reloaded);
                cx.notify();
            });
        });
    }

    fn apply_preview(
        &mut self,
        messages: Option<Vec<AgentMessage>>,
        selection: Option<pi_sessions::branch::Selection>,
        cx: &mut Context<Self>,
    ) {
        if let Some(selection) = selection {
            self.guess_early(&selection);
        }
        let Some(messages) = messages else {
            return;
        };
        if self.preview != Preview::None {
            return;
        }
        let notices = self.notices();
        self.transcript = Transcript::from_messages(&messages);
        for notice in notices {
            self.transcript.push_notice(notice.kind, notice.text);
        }
        self.preview = Preview::Shown;
        cx.notify();
    }

    /// Whether this is a resumed conversation whose history has not arrived yet (it shows
    /// nothing rather than a new conversation's greeting).
    pub fn loads_history(&self) -> bool {
        self.resumed_from.is_some()
            && self.transcript.items().is_empty()
            && matches!(self.lifecycle, Lifecycle::Starting)
    }

    /// Notices in the transcript (e.g. an extension that failed to load while pi started).
    fn notices(&self) -> Vec<transcript::Notice> {
        self.transcript
            .items()
            .iter()
            .filter_map(|item| match &item.kind {
                transcript::ItemKind::Notice(notice) => Some(notice.clone()),
                _ => None,
            })
            .collect()
    }

    /// Without the models that a host's pi lists and this computer does not have.
    fn usable(&self, mut models: Vec<Model>, cx: &gpui_kit::App) -> Vec<Model> {
        let hosts = cx.try_global::<crate::hosts::HostsHandle>();
        if let (Some(id), Some(hosts)) = (&self.host, hosts)
            && let Some(served) = hosts.0.read(cx).served_models(id)
        {
            models.retain(|model| served.usable(&model.provider, &model.id));
        }
        models
    }

    fn initialize(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let resumed = self.resumed_from.is_some();
        cx.spawn(async move |this, cx| {
            let state = client.get_state().await;
            let models = client.get_available_models().await.unwrap_or_default();
            let levels = client
                .get_available_thinking_levels()
                .await
                .unwrap_or_default();
            let messages = if resumed {
                client.get_messages().await.ok()
            } else {
                None
            };
            let stats = client.get_session_stats().await.ok();
            let commands = client.get_commands().await.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                match state {
                    Ok(state) => {
                        this.state = Some(state);
                        this.lifecycle = Lifecycle::Ready;
                    }
                    Err(error) => {
                        this.lifecycle = Lifecycle::Exited {
                            message: this.exit_message(&error),
                        };
                        this.return_outbox(cx);
                    }
                }
                this.models = this.usable(models, cx);
                this.thinking_levels = levels;
                this.stats = stats;
                this.commands = commands;
                let mut replaced = false;
                if let Some(messages) = messages {
                    let name = this.transcript.session_name().map(str::to_owned);
                    // Notices pi sent while starting (e.g. an extension that failed to load)
                    // stay visible after the history.
                    let notices = this.notices();
                    let mut fresh = Transcript::from_messages(&messages);
                    for notice in notices {
                        fresh.push_notice(notice.kind, notice.text);
                    }
                    // The preview from the file is usually exactly this: keep it, so the rows
                    // (and the scroll position) stay as they are.
                    let same =
                        this.preview == Preview::Shown && fresh.items() == this.transcript.items();
                    if !same {
                        this.transcript = fresh;
                        replaced = true;
                    }
                    this.preview = Preview::Superseded;
                    this.transcript.set_session_name(
                        name.or_else(|| this.state.as_ref().and_then(|s| s.session_name.clone())),
                    );
                }
                this.early.shown = None;
                this.remember(!resumed, cx);
                this.flush_outbox(cx);
                this.report_busy(cx);
                if this.lifecycle == Lifecycle::Ready {
                    this.apply_pending_model(cx);
                    this.apply_inherited(cx);
                }
                // A replaced transcript restarts item ids: rebuild every row.
                cx.emit(if replaced {
                    SessionEvent::Reloaded
                } else {
                    SessionEvent::TranscriptChanged
                });
                cx.emit(SessionEvent::MetaChanged);
                cx.notify();
            });
        })
        .detach();
    }

    fn exit_message(&self, error: &RpcError) -> String {
        let tail = self.process.as_ref().map_or_default(PiProcess::stderr_tail);
        let detail = tail
            .iter()
            .rev()
            .take(6)
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        if detail.is_empty() {
            format!("pi stopped: {error}")
        } else {
            format!("pi stopped: {error}\n{detail}")
        }
    }

    fn refresh_stats(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let stats = client.get_session_stats().await;
            let state = client.get_state().await;
            let _ = this.update(cx, |this, cx| {
                let old_file = this.session_file();
                if let Ok(stats) = stats {
                    this.stats = Some(stats);
                }
                if let Ok(state) = state {
                    this.state = Some(state);
                }
                this.remember(false, cx);
                if this.session_file() != old_file {
                    cx.emit(SessionEvent::SessionFileChanged);
                }
                cx.emit(SessionEvent::MetaChanged);
                cx.notify();
            });
        })
        .detach();
    }

    fn handle(&mut self, event: ClientEvent, cx: &mut Context<Self>) {
        match event {
            ClientEvent::Event(event) => {
                let now = Instant::now();
                self.transcript.apply(&event, now);
                self.in_flight.apply(&event);
                if cx.has_global::<super::token_rate::TokenRates>() {
                    let rates = &mut cx.global_mut::<super::token_rate::TokenRates>().0;
                    if self.rate.apply(&event, now, rates) {
                        super::token_rate::TokenRates::save(cx);
                    }
                }
                match *event {
                    pi_rpc::protocol::Event::AgentStart => {
                        self.runs_started += 1;
                        self.awaiting_start = false;
                        self.compaction.running = false;
                    }
                    pi_rpc::protocol::Event::QueueUpdate {
                        steering,
                        follow_up,
                    } if self.clearing_queue == 0 => self.queued.update(&steering, &follow_up),
                    pi_rpc::protocol::Event::AgentSettled { .. } => {
                        self.awaiting_start = false;
                        if self.clearing_queue == 0 && self.transcript.queue().is_empty() {
                            self.queued.clear();
                        }
                        self.refresh_stats(cx);
                        cx.emit(SessionEvent::Settled);
                    }
                    pi_rpc::protocol::Event::CompactionStart {
                        reason: CompactionReason::Manual,
                    } => self.compaction.running = true,
                    pi_rpc::protocol::Event::CompactionEnd {
                        reason: CompactionReason::Manual,
                        ..
                    } => {
                        self.compaction.running = false;
                        self.compaction.ended += 1;
                        self.flush_outbox(cx);
                        self.refresh_stats(cx);
                    }
                    pi_rpc::protocol::Event::TurnEnd { .. }
                    | pi_rpc::protocol::Event::CompactionEnd { .. } => self.refresh_stats(cx),
                    pi_rpc::protocol::Event::SessionInfoChanged { .. } => {
                        cx.emit(SessionEvent::SessionFileChanged);
                        cx.emit(SessionEvent::MetaChanged);
                    }
                    _ => {}
                }
            }
            ClientEvent::ExtensionUi(request) => self.handle_extension_ui(request, cx),
            ClientEvent::ProtocolError { line, error } => {
                log::warn!("undecodable pi record ({error}): {line}");
            }
            ClientEvent::Disconnected => {
                let message = self.exit_message(&RpcError::Disconnected);
                self.transcript.force_settle(Instant::now());
                self.in_flight = InFlight::default();
                self.rate = RateMeter::default();
                if self.lifecycle == Lifecycle::Ready {
                    self.transcript
                        .push_notice(NoticeKind::Error, message.clone());
                }
                self.lifecycle = Lifecycle::Exited { message };
                self.client = None;
                if let Some(sent) = self.sending.take()
                    && self.runs_started == sent.runs_before
                {
                    // pi stopped before it took the prompt.
                    self.outbox.push_front(sent.prompt);
                }
                self.awaiting_start = false;
                self.compaction.sent = false;
                self.compaction.running = false;
                self.queued = QueuedPrompts::default();
                self.return_outbox(cx);
                cx.emit(SessionEvent::MetaChanged);
            }
        }
    }

    fn handle_extension_ui(&mut self, request: ExtensionUiRequest, cx: &mut Context<Self>) {
        match &request.method {
            ExtensionUiMethod::Notify {
                message,
                notify_type,
            } => {
                cx.emit(SessionEvent::Notify {
                    kind: *notify_type,
                    message: message.clone(),
                });
            }
            ExtensionUiMethod::SetStatus {
                status_key,
                status_text,
            } => {
                match status_text {
                    Some(text) => drop(
                        self.extension_status
                            .insert(status_key.clone(), text.clone()),
                    ),
                    None => drop(self.extension_status.remove(status_key)),
                }
                if RECALLED_STATUS.contains(&status_key.as_str()) {
                    self.early.web_reported = true;
                    if self.state.is_some() {
                        self.remember(false, cx);
                    }
                }
            }
            ExtensionUiMethod::SetWidget {
                widget_key,
                widget_lines,
                ..
            } => match widget_lines {
                Some(lines) => drop(self.widgets.insert(widget_key.clone(), lines.clone())),
                None => drop(self.widgets.remove(widget_key)),
            },
            ExtensionUiMethod::SetEditorText { text } => {
                cx.emit(SessionEvent::SetEditorText(text.clone()));
            }
            ExtensionUiMethod::SetTitle { .. } | ExtensionUiMethod::Unknown => {}
            ExtensionUiMethod::Select { .. }
            | ExtensionUiMethod::Confirm { .. }
            | ExtensionUiMethod::Input { .. }
            | ExtensionUiMethod::Editor { .. } => cx.emit(SessionEvent::Dialog(request)),
        }
        cx.emit(SessionEvent::MetaChanged);
    }
}

/// What a namer reads of the prompt `text`: the message, and the names of the files.
fn about(text: &str) -> String {
    let prompt = prompt::split(text);
    if prompt.files.is_empty() {
        return prompt.text.to_owned();
    }
    let names: Vec<_> = prompt.files.iter().map(|(f, _)| f.name.as_str()).collect();
    format!("{}\n\n(Attached: {})", prompt.text, names.join(", "))
        .trim()
        .to_owned()
}

/// Gives messages back to the composer, if there are any.
fn restore(texts: &[String], images: Vec<ImageContent>, cx: &mut Context<PiSession>) {
    if !texts.is_empty() || !images.is_empty() {
        cx.emit(SessionEvent::RestoreToEditor {
            text: prompt::merge(texts),
            images,
            replace: false,
        });
    }
}

/// The saved SSH host `host` with the `pi` to run there.
fn saved_host(host: Option<&HostId>, cx: &gpui_kit::App) -> Option<remote::SavedHost> {
    let id = host?;
    let hosts = cx.try_global::<crate::hosts::HostsHandle>()?.0.read(cx);
    let mut saved = hosts.get(id).cloned()?;
    // Prefer the exact path the probe found (conda / nvm installs may only be on an
    // interactive shell's PATH; pistachio's own install is not on PATH at all).
    saved.pi_command = hosts.pi_command(id);
    Some(saved)
}

/// How to launch pi for a session in `cwd`. Chat sessions get only the web tools
/// (`--tools web_search,web_fetch`, which the extension keeps off until the chat turns web
/// search on) and `python` (Monty: no files or network), no MCP, and no built-in tools: a chat can never read or change files. An
/// explicit `--tools` list wins over the tool set pi recorded in the session, also for chats
/// started with `--no-tools` before web search existed. If the extension cannot be written,
/// chats fall back to `--no-tools`.
///
/// On a remote `host`, pi runs there through the host's shared ssh connection, in `cwd` on the
/// remote, via the login shell (see [`remote::Ssh::command`]); remote sessions are always Code.
/// The active branch's messages from a session file, or `None` when pi has to rebuild them
/// (see [`pi_sessions::branch`]), and what the branch selected; both `None` when the file
/// can't be read.
fn read_preview(
    file: &Path,
) -> (
    Option<Vec<AgentMessage>>,
    Option<pi_sessions::branch::Selection>,
) {
    let Some(branch) = std::fs::read_to_string(file)
        .ok()
        .and_then(|text| pi_sessions::branch::Branch::parse(&text))
    else {
        return (None, None);
    };
    let messages = branch.messages().and_then(|messages| {
        messages
            .into_iter()
            .map(|message| serde_json::from_value(message).ok())
            .collect()
    });
    let mut selection = branch.selection(deepsearch::STATUS_KEY);
    // Without a level of its own, pi resumes a conversation at its settings' default.
    if selection.thinking_level.is_none() {
        let default = pi_sessions::agent_dir()
            .and_then(|dir| std::fs::read_to_string(dir.join("settings.json")).ok())
            .and_then(|text| pi_config::settings::startup_defaults(&text).ok())
            .and_then(|defaults| defaults.thinking_level);
        selection.thinking_level = Some(default.unwrap_or_else(|| "medium".to_owned()));
    }
    (messages, Some(selection))
}

/// The web search statuses the extension sets for a chat with these entries of its own, as
/// it reads them when the chat is resumed, starting from `defaults` (a new chat's).
fn web_status(
    entries: &[serde_json::Value],
    defaults: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let get = |key: &str| defaults.get(key).map(String::as_str);
    let mut mode = get(deepsearch::STATUS_KEY)
        .and_then(WebMode::parse)
        .unwrap_or(WebMode::Off);
    let mut effort = get(deepsearch::EFFORT_KEY)
        .and_then(Effort::parse)
        .unwrap_or_default();
    for data in entries {
        if let Some(chosen) = data["mode"].as_str().and_then(WebMode::parse) {
            mode = chosen;
        } else if let Some(enabled) = data["enabled"].as_bool() {
            // Chats from before the modes existed stored `enabled`.
            mode = if enabled { WebMode::Deep } else { WebMode::Off };
        }
        if let Some(chosen) = data["effort"].as_str().and_then(Effort::parse) {
            effort = chosen;
        }
    }
    let mut status = BTreeMap::new();
    if mode != WebMode::Off {
        status.insert(deepsearch::STATUS_KEY.to_owned(), mode.as_str().to_owned());
    }
    if mode == WebMode::Deep {
        status.insert(
            deepsearch::EFFORT_KEY.to_owned(),
            effort.as_str().to_owned(),
        );
    }
    status
}

/// pi's options for a conversation in `mode` on this computer that keeps no session file:
/// the same tools and extensions as a real one, to ask pi about them (Settings → Tools). A
/// Code one runs in pi's agent folder, so no project adds its own.
pub(crate) fn probe_options(config: &AppConfig, mode: Mode) -> SpawnOptions {
    let cwd = match mode {
        Mode::Chat => {
            if let Err(error) = std::fs::create_dir_all(&config.chat_dir) {
                log::warn!(
                    "could not create the chat folder {}: {error}",
                    config.chat_dir.display()
                );
            }
            config.chat_dir.clone()
        }
        Mode::Code => pi_sessions::agent_dir()
            .filter(|dir| dir.is_dir())
            .or_else(|| config.home.clone())
            .unwrap_or_else(std::env::temp_dir),
    };
    let (mut options, _) = spawn_options(config, &cwd, None, None, &[]);
    options.extra_args.push("--no-session".into());
    options
}

fn spawn_options(
    config: &AppConfig,
    cwd: &Path,
    session_file: Option<&Path>,
    host: Option<&remote::SavedHost>,
    remote_env: &[(String, String)],
) -> (SpawnOptions, Mode) {
    let mut options = SpawnOptions::new(cwd);
    options.program.clone_from(&config.pi_program);
    config
        .child_env
        .ready()
        .unwrap_or_default()
        .clone_into(&mut options.env);
    options.session = session_file.map(Path::to_path_buf);
    let Some(host) = host else {
        let mode = config.mode_of(cwd);
        // Pistachio's own tools read the text written for the model in Settings → Tools.
        options.env.push((
            pi_config::tools::TEXTS_ENV.into(),
            config.tools_dir.join(pi_config::tools::TEXTS_FILE).into(),
        ));
        if mode == Mode::Chat {
            chat_args(config, &mut options);
        } else {
            builtin_skill_args(config, &mut options);
            document_args(config, &mut options);
            sandbox_args(config, &mut options);
        }
        return (options, mode);
    };
    let argv: Vec<String> = std::iter::once(host.pi_command().to_owned())
        .chain(
            options
                .args()
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned()),
        )
        .collect();
    let ssh = remote::Ssh::new(host.target.clone());
    let (program, args) = ssh.command_with_env(Some(&cwd.to_string_lossy()), remote_env, &argv);
    options.launcher = Some(pi_rpc::Launcher { program, args });
    // The local ssh client just needs some existing folder to run in.
    options.cwd = config.home.clone().unwrap_or_else(std::env::temp_dir);
    (options, Mode::Code)
}

/// The optional Code tools turned on in Settings → Tools (see
/// [`pi_config::tools::OPTIONAL_CODE_TOOLS`]).
pub(crate) fn code_tools(cx: &gpui_kit::App) -> Vec<String> {
    cx.try_global::<crate::settings::AppSettings>()
        .map_or_else(Vec::new, |settings| settings.code_tools.clone())
}

/// `--tools +grep,+find` for `tools` (pi adds them to its default tools); only the optional
/// ones are passed. Code sessions on this computer only: there the sandbox checks them, like
/// `read`.
pub(crate) fn optional_tool_args(tools: &[String], options: &mut SpawnOptions) {
    let added = tools
        .iter()
        .filter(|tool| pi_config::tools::OPTIONAL_CODE_TOOLS.contains(&tool.as_str()))
        .map(|tool| format!("+{tool}"))
        .collect::<Vec<_>>();
    if !added.is_empty() {
        options
            .extra_args
            .extend(["--tools".into(), added.join(",").into()]);
    }
}

/// Whether `request_user_help` is switched on in Settings → Tools (see [`sandbox::help`]).
pub(crate) fn help_tool(cx: &gpui_kit::App) -> bool {
    cx.try_global::<crate::settings::AppSettings>()
        .is_none_or(|settings| settings.help_tool)
}

/// Tells the sandbox's extension that `request_user_help` is switched off; it is offered
/// otherwise, while the sandbox is on for the session's folder.
pub(crate) fn help_tool_env(on: bool, options: &mut SpawnOptions) {
    if !on {
        options.env.push((sandbox::help::ENV.into(), "off".into()));
    }
}

/// `--skill` for each skill that comes with the app (see [`crate::builtin_skills`]). Chats
/// leave them out: they have no tools to act on them.
fn builtin_skill_args(config: &AppConfig, options: &mut SpawnOptions) {
    match crate::builtin_skills::install(&config.builtin_skills_dir) {
        Ok(folders) => {
            for folder in folders {
                options.extra_args.extend(["--skill".into(), folder.into()]);
            }
        }
        Err(error) => log::warn!(
            "the built-in skills are unavailable: could not write them to {}: {error}",
            config.builtin_skills_dir.display()
        ),
    }
}

/// The extension that has `read` return PDF files as Markdown ([`documents::EXTENSION_SOURCE`]).
/// Chats have no `read`, and pi on an SSH host no parser.
fn document_args(config: &AppConfig, options: &mut SpawnOptions) {
    let Some(exe) = &config.exe else {
        return;
    };
    match documents::install_extension(&config.documents_dir) {
        Ok(extension) => {
            options
                .extra_args
                .extend(["--extension".into(), extension.into()]);
            options
                .env
                .push((documents::BIN_ENV.into(), exe.clone().into()));
            document_dir_env(config, options);
        }
        Err(error) => log::warn!(
            "PDF files are not read as Markdown: could not write the extension to {}: {error}",
            config.documents_dir.display()
        ),
    }
}

/// The extension that runs the agent's tools in the sandbox (see [`sandbox`]). Only local Code
/// sessions have tools on this computer; it is loaded also while the sandbox is off, so turning
/// it on applies to running sessions.
fn sandbox_args(config: &AppConfig, options: &mut SpawnOptions) {
    if !sandbox::SUPPORTED {
        return;
    }
    let Some(exe) = &config.exe else {
        return;
    };
    match sandbox::install_extension(&config.sandbox_dir) {
        Ok(extension) => {
            options
                .extra_args
                .extend(["--extension".into(), extension.into()]);
            options
                .env
                .push((sandbox::BIN_ENV.into(), exe.clone().into()));
            options
                .env
                .push((sandbox::DIR_ENV.into(), config.sandbox_dir.clone().into()));
        }
        Err(error) => log::warn!(
            "commands are not sandboxed: could not write the extension to {}: {error}",
            config.sandbox_dir.display()
        ),
    }
}

/// Where the helper finds the settings of document parsing.
fn document_dir_env(config: &AppConfig, options: &mut SpawnOptions) {
    options.env.push((
        documents::config::DIR_ENV.into(),
        config.documents_dir.clone().into(),
    ));
}

/// pi's arguments and environment for a chat: the web tools and python (see [`spawn_options`]).
fn chat_args(config: &AppConfig, options: &mut SpawnOptions) {
    let extension = match (
        &config.exe,
        deepsearch::install_extension(&config.deepsearch_dir),
    ) {
        (Some(exe), Ok(extension)) => Some((exe, extension)),
        (None, _) => {
            log::warn!("web search is unavailable: the app's own executable could not be found");
            None
        }
        (_, Err(error)) => {
            log::warn!(
                "web search is unavailable: could not write its extension to {}: {error}",
                config.deepsearch_dir.display()
            );
            None
        }
    };
    let Some((exe, extension)) = extension else {
        options.extra_args.push("--no-tools".into());
        return;
    };
    // The python tool comes along when its extension can be written.
    let python = match python_tool::install_extension(&config.python_dir) {
        Ok(path) => Some(path),
        Err(error) => {
            log::warn!(
                "chats cannot compute with Python: could not write its extension to {}: {error}",
                config.python_dir.display()
            );
            None
        }
    };
    let tools = deepsearch::TOOL_NAMES
        .into_iter()
        .chain(python.is_some().then_some(python_tool::TOOL_NAME))
        .collect::<Vec<_>>()
        .join(",");
    options.extra_args.extend([
        "--tools".into(),
        tools.into(),
        "--no-mcp".into(),
        "--extension".into(),
        extension.into(),
    ]);
    if let Some(python) = python {
        options
            .extra_args
            .extend(["--extension".into(), python.into()]);
        options
            .env
            .push((python_tool::BIN_ENV.into(), exe.clone().into()));
        options.env.push((
            python_tool::DIR_ENV.into(),
            config.python_dir.clone().into(),
        ));
    }
    options
        .env
        .push((deepsearch::BIN_ENV.into(), exe.clone().into()));
    options.env.push((
        deepsearch::config::DIR_ENV.into(),
        config.deepsearch_dir.clone().into(),
    ));
    document_dir_env(config, options);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn fork_entries_match_from_the_end() {
        let message = |id: &str, text: &str| pi_rpc::protocol::ForkMessage {
            entry_id: id.into(),
            text: text.into(),
        };
        // pi still lists a prompt from before a compaction that the transcript no longer has.
        let messages = [
            message("e0", "old"),
            message("e1", "first"),
            message("e2", "second"),
        ];
        assert_eq!(
            super::fork_entry(&messages, 0, "second").as_deref(),
            Some("e2")
        );
        assert_eq!(
            super::fork_entry(&messages, 1, " first\n").as_deref(),
            Some("e1")
        );
        assert_eq!(super::fork_entry(&messages, 1, "other"), None);
        assert_eq!(super::fork_entry(&messages, 3, "x"), None);
    }

    /// The optional Code tools turned on reach pi as additions to its defaults; others are not
    /// passed.
    #[test]
    fn optional_code_tools_are_added_to_pis_defaults() {
        let mut options = pi_rpc::SpawnOptions::new("/tmp");
        super::optional_tool_args(&[], &mut options);
        assert!(options.extra_args.is_empty(), "nothing when none are on");
        let tools = ["grep", "codemode", "rm-rf-tool"].map(str::to_owned);
        super::optional_tool_args(&tools, &mut options);
        assert_eq!(
            options.extra_args,
            [
                std::ffi::OsString::from("--tools"),
                std::ffi::OsString::from("+grep,+codemode")
            ]
        );
    }

    /// Every local pi learns where the text written for the model in Settings → Tools is.
    #[test]
    fn local_sessions_get_the_tool_texts() {
        let config = AppConfig::for_tests("pi".into());
        let (options, _) = super::spawn_options(&config, &config.chat_dir, None, None, &[]);
        let texts = config.tools_dir.join(pi_config::tools::TEXTS_FILE);
        assert!(
            options
                .env
                .contains(&(pi_config::tools::TEXTS_ENV.into(), texts.into()))
        );
    }

    /// A resumed chat's web search mode, read from its file like the extension reads it.
    #[test]
    fn web_status_follows_the_extension() {
        use serde_json::json;
        use std::collections::BTreeMap;
        let (mode, effort) = (deepsearch::STATUS_KEY, deepsearch::EFFORT_KEY);
        let defaults = BTreeMap::from([(mode.to_owned(), "search".to_owned())]);
        assert_eq!(
            super::web_status(&[], &defaults),
            defaults,
            "the default mode"
        );
        assert_eq!(
            super::web_status(&[json!({"mode": "off", "effort": "max"})], &defaults),
            BTreeMap::new()
        );
        assert_eq!(
            super::web_status(
                &[
                    json!({"mode": "deep", "effort": "max"}),
                    json!({"effort": "light"})
                ],
                &BTreeMap::new()
            ),
            BTreeMap::from([
                (mode.to_owned(), "deep".to_owned()),
                (effort.to_owned(), "light".to_owned())
            ])
        );
        // Chats from before the modes stored `enabled`.
        assert_eq!(
            super::web_status(&[json!({"enabled": true})], &BTreeMap::new()),
            BTreeMap::from([
                (mode.to_owned(), "deep".to_owned()),
                (effort.to_owned(), "standard".to_owned())
            ])
        );
    }

    #[test]
    fn chat_sessions_launch_with_only_web_and_python_tools() {
        let config = AppConfig::for_tests("pi".into());
        let (chat, mode) = super::spawn_options(&config, &config.chat_dir.clone(), None, None, &[]);
        assert_eq!(mode, super::Mode::Chat);
        let extension = config.deepsearch_dir.join("extension.ts");
        let python = config.python_dir.join("extension.ts");
        assert_eq!(
            chat.extra_args,
            [
                "--tools".into(),
                "web_search,web_fetch,python".into(),
                "--no-mcp".into(),
                "--extension".into(),
                std::ffi::OsString::from(&extension),
                "--extension".into(),
                std::ffi::OsString::from(&python),
            ]
        );
        assert!(extension.exists());
        assert!(python.exists());
        assert!(chat.env.contains(&(
            python_tool::DIR_ENV.into(),
            std::ffi::OsString::from(&config.python_dir)
        )));
        for variable in [deepsearch::BIN_ENV, python_tool::BIN_ENV] {
            assert!(
                chat.env
                    .contains(&(variable.into(), std::ffi::OsString::from("/app/pistachio")))
            );
        }

        let project = std::env::temp_dir().join("some-project");
        let session = project.join("s.jsonl");
        let (code, mode) = super::spawn_options(&config, &project, Some(&session), None, &[]);
        assert_eq!(mode, super::Mode::Code);
        let mut expected: Vec<std::ffi::OsString> = vec![
            "--skill".into(),
            std::ffi::OsString::from(config.builtin_skills_dir.join("add-skill")),
            "--extension".into(),
            config.documents_dir.join("extension.ts").into(),
        ];
        if sandbox::SUPPORTED {
            expected.extend([
                "--extension".into(),
                config.sandbox_dir.join("extension.ts").into(),
            ]);
            assert!(config.sandbox_dir.join("extension.ts").exists());
            assert!(code.env.contains(&(
                sandbox::BIN_ENV.into(),
                std::ffi::OsString::from("/app/pistachio")
            )));
        }
        assert_eq!(code.extra_args, expected);
        assert!(config.documents_dir.join("extension.ts").exists());
        for (key, value) in [
            (
                documents::BIN_ENV,
                std::ffi::OsString::from("/app/pistachio"),
            ),
            (
                documents::config::DIR_ENV,
                config.documents_dir.clone().into(),
            ),
        ] {
            assert!(code.env.contains(&(key.into(), value)), "{key}");
        }
        assert!(
            config
                .builtin_skills_dir
                .join("add-skill/SKILL.md")
                .exists()
        );
        assert_eq!(code.session.as_deref(), Some(session.as_path()));
    }

    #[test]
    fn remote_sessions_launch_pi_through_ssh() {
        let config = AppConfig::for_tests("pi".into());
        let host = remote::SavedHost {
            id: remote::HostId("box".into()),
            label: "Box".into(),
            target: remote::Target::Alias {
                alias: "box".into(),
            },
            pi_command: Some("/opt/pi/bin/pi".into()),
            recent_folders: Vec::new(),
        };
        let folder = std::path::Path::new("/srv/my app");
        let (options, mode) = super::spawn_options(&config, folder, None, Some(&host), &[]);
        assert_eq!(mode, super::Mode::Code);
        let launcher = options.launcher.expect("remote sessions use a launcher");
        assert_eq!(launcher.program, PathBuf::from("ssh"));
        let args: Vec<String> = launcher
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        let joined = args.join(" ");
        assert!(args.iter().any(|arg| arg == "box"), "{joined}");
        assert!(
            joined.contains("/opt/pi/bin/pi")
                && joined.contains("--mode")
                && joined.contains("rpc"),
            "{joined}"
        );
        assert!(
            joined.contains("'/srv/my app'"),
            "remote folder is quoted: {joined}"
        );
        // The local ssh client runs in an existing local folder, not the remote path.
        assert_ne!(options.cwd, folder);
    }

    /// End to end on a real SSH host with real pi (e.g. after `remote`'s `live_install`):
    /// pi is started through the same launch path as a remote session, runs a shell command
    /// on the host through pi's RPC, its session file is listed from the host, and the
    /// session resumes with its history. No model is needed.
    /// `PISTACHIO_SSH_TEST_HOST=<alias> cargo test -p pistachio -- --ignored real_pi_over_ssh`
    /// The preview read from a session file shows exactly what pi's own history does. Runs
    /// real pi on every `.jsonl` in `PISTACHIO_PREVIEW_CHECK` (use copies: pi may migrate a
    /// file it opens).
    #[test]
    #[ignore = "needs pi and session files"]
    fn preview_matches_pi() {
        use futures::executor::block_on;
        let Ok(dir) = std::env::var("PISTACHIO_PREVIEW_CHECK") else {
            return;
        };
        let (mut same, mut left_to_pi) = (0, 0);
        for entry in std::fs::read_dir(&dir).expect("readable folder") {
            let path = entry.expect("entry").path();
            if path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            let (Some(preview), _) = super::read_preview(&path) else {
                left_to_pi += 1;
                continue;
            };
            let mut options = pi_rpc::SpawnOptions::new(std::env::temp_dir());
            options.session = Some(path.clone());
            let (process, _events) = pi_rpc::PiProcess::spawn(&options).expect("spawn pi");
            let messages = block_on(process.client().get_messages()).expect("get_messages");
            let ours = transcript::Transcript::from_messages(&preview);
            let theirs = transcript::Transcript::from_messages(&messages);
            assert_eq!(
                ours.items(),
                theirs.items(),
                "preview differs from pi for {}",
                path.display()
            );
            same += 1;
        }
        eprintln!("{same} previews match pi; {left_to_pi} left to pi");
        assert!(same > 0, "no session was checked");
    }

    #[test]
    #[ignore = "runs pi on PISTACHIO_SSH_TEST_HOST"]
    fn real_pi_over_ssh() {
        use futures::executor::block_on;
        let Ok(alias) = std::env::var("PISTACHIO_SSH_TEST_HOST") else {
            return;
        };
        let ssh = remote::Ssh::new(remote::Target::Alias {
            alias: alias.clone(),
        });
        ssh.connect(false).expect("connect");
        let probe = ssh.probe("pi").expect("probe");
        let pi = probe.pi_path.expect("pi is installed on the host");
        // A scratch folder with a project, a pi settings folder whose only model is a dummy
        // that refuses connections (so pi records an error reply without any real model or
        // key), and a custom pi command (a wrapper) that points pi at those settings.
        let scratch = ssh
            .exec(&format!(
                "d=$(mktemp -d /tmp/pistachio-e2e.XXXXXX) && cd \"$d\" && mkdir -p proj agent && \
                 (cd proj && git init -q) && \
                 printf '%s' '{{\"providers\":{{\"dummy\":{{\"baseUrl\":\"http://127.0.0.1:9/v1\",\"api\":\"openai-completions\",\"apiKey\":\"none\",\"models\":[{{\"id\":\"none\",\"name\":\"None\"}}]}}}}}}' > agent/models.json && \
                 printf '%s' '{{\"defaultProvider\":\"dummy\",\"defaultModel\":\"none\"}}' > agent/settings.json && \
                 printf '#!/bin/sh\\nPI_CODING_AGENT_DIR=%s/agent\\nexport PI_CODING_AGENT_DIR\\nexec %s \"$@\"\\n' \"$d\" {} > pi && chmod 755 pi && pwd -P",
                remote::sh_quote(&pi)
            ))
            .expect("scratch folder")
            .trim()
            .to_owned();
        let folder = format!("{scratch}/proj");
        let host = remote::SavedHost {
            id: remote::HostId("e2e".into()),
            label: alias,
            target: ssh.target().clone(),
            pi_command: Some(format!("{scratch}/pi")),
            recent_folders: Vec::new(),
        };
        let config = AppConfig::for_tests("pi".into());
        let (options, mode) = super::spawn_options(
            &config,
            std::path::Path::new(&folder),
            None,
            Some(&host),
            &[],
        );
        assert_eq!(mode, super::Mode::Code);
        let (process, events) = pi_rpc::PiProcess::spawn(&options).expect("spawn pi over ssh");
        let client = process.client().clone();
        block_on(client.get_state()).expect("get_state");
        let result = block_on(client.bash(
            client.next_id(),
            "printf e2e > made-by-pi.txt && pwd -P".into(),
            false,
        ))
        .expect("bash through pi");
        eprintln!("bash: {:?} exit {:?}", result.output, result.exit_code);
        assert_eq!(result.output.trim(), folder);
        assert!(ssh.exists(&format!("{folder}/made-by-pi.txt")).unwrap());

        // A prompt: the dummy model fails, and pi records the turn (and writes its file).
        block_on(client.prompt("hello from the e2e test".into(), Vec::new(), None))
            .expect("prompt accepted");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut settled = false;
        while std::time::Instant::now() < deadline {
            match block_on(events.recv()) {
                Ok(pi_rpc::ClientEvent::Event(event)) => {
                    if matches!(*event, pi_rpc::protocol::Event::AgentSettled { .. }) {
                        settled = true;
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        assert!(settled, "the turn settles");
        let file = block_on(client.get_state())
            .expect("state")
            .session_file
            .expect("pi reports its session file");
        drop(process);

        // The sidebar's listing of the host finds it (pi writes it once there is history).
        let mut index = remote::SessionIndex::default();
        let sessions = index
            .refresh(&ssh, &format!("{scratch}/agent/sessions"))
            .expect("list sessions");
        eprintln!("sessions on the host: {}", sessions.len());
        let listed = sessions
            .iter()
            .find(|session| session.path == std::path::Path::new(&file));
        eprintln!("listed: {listed:?}");

        // Resume it: the history comes back.
        let (options, _) = super::spawn_options(
            &config,
            std::path::Path::new(&folder),
            Some(std::path::Path::new(&file)),
            Some(&host),
            &[],
        );
        let (process, _events) = pi_rpc::PiProcess::spawn(&options).expect("resume");
        let messages = block_on(process.client().get_messages()).expect("messages");
        eprintln!("resumed with {} messages", messages.len());
        drop(process);
        let _ = ssh.exec(&format!("rm -rf {}", remote::sh_quote(&scratch)));
        let listed = listed.expect("the session is listed from the host");
        assert_eq!(listed.title(), "hello from the e2e test");
        assert_eq!(listed.cwd, std::path::Path::new(&folder));
        assert!(!messages.is_empty(), "the resumed session has its history");
    }

    /// pi on an SSH host uses a model configured only on this computer, through the relay:
    /// a fake OpenAI-compatible server here (on loopback, unreachable from the host) answers,
    /// and gets the real key and header that only this computer has. A login (Codex),
    /// also only here, works there too.
    /// `PISTACHIO_SSH_TEST_HOST=<alias> cargo test -p pistachio -- --ignored --test-threads 1
    /// models_relay_over`
    #[cfg(unix)]
    #[test]
    #[ignore = "runs pi on PISTACHIO_SSH_TEST_HOST"]
    fn models_relay_over_ssh() {
        models_relay(false);
    }

    /// The same without connection sharing, as on Windows: the relay listens on a port, and a
    /// tunnel holds the connection.
    #[cfg(unix)]
    #[test]
    #[ignore = "runs pi on PISTACHIO_SSH_TEST_HOST"]
    fn models_relay_over_a_tunnel() {
        models_relay(true);
    }

    #[cfg(unix)]
    fn models_relay(tunnel: bool) {
        use std::io::{BufRead as _, BufReader, Read as _, Write as _};
        use std::sync::{Arc, Mutex};

        use futures::executor::block_on;
        let Ok(alias) = std::env::var("PISTACHIO_SSH_TEST_HOST") else {
            return;
        };

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let log = log.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream);
                    let mut head = Vec::new();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                            break;
                        }
                        head.push(line.trim().to_owned());
                    }
                    let header = |name: &str| {
                        head.iter()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case(name)
                                    .then(|| value.trim().to_owned())
                            })
                            .unwrap_or_default()
                    };
                    let length: usize = header("content-length").parse().unwrap_or(0);
                    let mut body = vec![0; length];
                    let _ = reader.read_exact(&mut body);
                    // pi's node does this too.
                    if header("content-encoding") == "zstd" {
                        let script = "process.stdout.write(require('zlib').zstdDecompressSync(\
                                      require('fs').readFileSync(0)))";
                        let mut node = ::platform::command("node")
                            .args(["-e", script])
                            .stdin(std::process::Stdio::piped())
                            .stdout(std::process::Stdio::piped())
                            .spawn()
                            .expect("node");
                        node.stdin.take().unwrap().write_all(&body).unwrap();
                        body = node.wait_with_output().unwrap().stdout;
                    }
                    log.lock().unwrap().push(format!(
                        "{} auth={} extra={} account={} body={}",
                        head.first().cloned().unwrap_or_default(),
                        header("authorization"),
                        header("x-extra"),
                        header("chatgpt-account-id"),
                        String::from_utf8_lossy(&body)
                    ));
                    let mut out = reader.into_inner();
                    // ChatGPT: no WebSocket, and reasoning that only it can read.
                    if let Some(line) = head.first().filter(|line| line.contains("/codex/")) {
                        if line.starts_with("GET") {
                            let _ = out.write_all(
                                b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                            );
                            return;
                        }
                        let _ = out.write_all(
                            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
                        );
                        let reasoning = serde_json::json!({ "type": "reasoning", "id": "rs_1",
                            "summary": [{ "type": "summary_text", "text": "Hm." }],
                            "encrypted_content": "only-chatgpt-can-read-this" });
                        let text = serde_json::json!({ "type": "message", "id": "msg_1",
                            "role": "assistant", "status": "completed", "content":
                            [{ "type": "output_text", "text": "Hello from ChatGPT.", "annotations": [] }] });
                        let mut events = vec![serde_json::json!({ "type": "response.created",
                            "response": { "id": "resp_1" } })];
                        for (index, item) in [reasoning, text].into_iter().enumerate() {
                            for kind in ["added", "done"] {
                                events.push(serde_json::json!({
                                    "type": format!("response.output_item.{kind}"),
                                    "output_index": index, "item": item }));
                            }
                        }
                        events.push(serde_json::json!({ "type": "response.completed",
                            "response": { "id": "resp_1", "status": "completed", "usage": {
                                "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 } } }));
                        for event in events {
                            let _ = write!(out, "data: {event}\n\n");
                        }
                        return;
                    }
                    // A careless server that writes the credentials it got into its answer.
                    if head
                        .first()
                        .is_some_and(|line| line.starts_with("GET /v1/echo"))
                    {
                        let body = format!(
                            "{{\"got\":\"{}\",\"extra\":\"{}\"}}",
                            header("authorization"),
                            header("x-extra")
                        );
                        let _ = write!(
                            out,
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                             content-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        return;
                    }
                    let _ = out.write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
                    );
                    let chunk = |delta: serde_json::Value, finish: serde_json::Value| {
                        let event = serde_json::json!({
                            "id": "e2e", "object": "chat.completion.chunk", "created": 0,
                            "model": "fake-1",
                            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
                        });
                        format!("data: {event}\n\n")
                    };
                    let mut text = chunk(
                        serde_json::json!({ "role": "assistant", "content": "Hello from" }),
                        serde_json::Value::Null,
                    );
                    text += &chunk(
                        serde_json::json!({ "content": " this computer." }),
                        serde_json::Value::Null,
                    );
                    text += &chunk(serde_json::json!({}), "stop".into());
                    text += "data: [DONE]\n\n";
                    let _ = out.write_all(text.as_bytes());
                });
            }
        });

        // This computer's pi settings: one model on the fake server, with a key and a header
        // that must never reach the host.
        let local = tempfile::tempdir().unwrap();
        std::fs::write(
            local.path().join("models.json"),
            serde_json::json!({ "providers": { "fake": {
                "baseUrl": format!("http://127.0.0.1:{port}/v1"),
                "api": "openai-completions",
                "apiKey": "real-secret-key",
                "headers": { "x-extra": "real-header" },
                "models": [{ "id": "fake-1", "name": "Fake One" }],
            }, "openai-codex": { "baseUrl": format!("http://127.0.0.1:{port}") }}})
            .to_string(),
        )
        .unwrap();
        // A login whose token names the account, as ChatGPT's does.
        let access = {
            use base64::Engine as _;
            let part = |value: serde_json::Value| {
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string())
            };
            let claims = serde_json::json!({ "https://api.openai.com/auth":
                { "chatgpt_account_id": "real-account" } });
            let header = part(serde_json::json!({ "alg": "none" }));
            format!("{header}.{}.real-signature", part(claims))
        };
        std::fs::write(
            local.path().join("auth.json"),
            serde_json::json!({ "openai-codex": { "type": "oauth", "access": access,
                "refresh": "real-refresh", "expires": 32_503_680_000_000_u64,
                "accountId": "real-account" } })
            .to_string(),
        )
        .unwrap();
        let launch = crate::relay::Launch {
            program: crate::config::default_pi_program(),
            env: vec![(
                std::ffi::OsString::from("PI_CODING_AGENT_DIR"),
                local.path().as_os_str().to_owned(),
            )]
            .into(),
            dir: local.path().join("relay"),
            tcp: tunnel,
        };

        let mut ssh = remote::Ssh::new(remote::Target::Alias {
            alias: alias.clone(),
        });
        if tunnel {
            ssh = ssh.without_sharing();
        }
        ssh.connect(false).expect("connect");
        assert!(ssh.is_connected());
        let probe = ssh.probe("pi").expect("probe");
        let host_id = remote::HostId("e2e-relay".into());
        let link = crate::relay::link(&ssh, &host_id, &probe, None, &launch).expect("link");
        let token = crate::relay::token(&host_id);
        assert_eq!(link.master_pid, ssh.master_pid());
        assert!(link.models.usable("fake", "fake-1") && !link.models.usable("fake", "fake-2"));
        eprintln!("linked on port {}: {:?}", link.port, link.env);
        let agent = &link.env[0].1;
        let models = String::from_utf8(
            ssh.read_file(&format!("{agent}/models.json"))
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert!(
            models.contains(&format!("/p/fake/http/127.0.0.1:{port}/v1")),
            "{models}"
        );
        assert!(
            !models.contains("real-") && !models.contains(&access),
            "no credential on the host: {models}"
        );

        // Linking again on the same connection keeps the port.
        let again = crate::relay::link(&ssh, &host_id, &probe, Some(&link), &launch).unwrap();
        assert_eq!(again.port, link.port);
        assert_eq!(again.master_pid, link.master_pid);
        assert_eq!(crate::relay::token(&host_id), token);

        // pi there, with its sessions in a scratch folder.
        let scratch = ssh
            .exec("mktemp -d /tmp/pistachio-relay.XXXXXX")
            .unwrap()
            .trim()
            .to_owned();
        // With the relay port, so pi starts only once that answers.
        let env = vec![
            link.env[0].clone(),
            ("PI_CODING_AGENT_SESSION_DIR".to_owned(), scratch.clone()),
            link.env[2].clone(),
        ];
        let host = remote::SavedHost {
            id: host_id.clone(),
            label: alias,
            target: ssh.target().clone(),
            pi_command: probe.pi_path.clone(),
            recent_folders: Vec::new(),
        };
        let config = AppConfig::for_tests("pi".into());
        let (options, _) = super::spawn_options(
            &config,
            std::path::Path::new(&scratch),
            None,
            Some(&host),
            &env,
        );
        let (process, events) = pi_rpc::PiProcess::spawn(&options).expect("spawn pi over ssh");
        let client = process.client().clone();
        let available = block_on(client.get_available_models()).expect("models");
        eprintln!(
            "available on the host: {:?}",
            available
                .iter()
                .map(|m| m.qualified_id())
                .collect::<Vec<_>>()
        );
        assert!(available.iter().any(|model| model.id == "fake-1"));
        block_on(client.request(pi_rpc::protocol::Command::SetModel {
            provider: "fake".into(),
            model_id: "fake-1".into(),
        }))
        .expect("set model");
        let prompt = |text: &str| {
            block_on(client.prompt(text.into(), Vec::new(), None)).expect("prompt accepted");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while std::time::Instant::now() < deadline {
                match block_on(events.recv()) {
                    Ok(pi_rpc::ClientEvent::Event(event))
                        if matches!(*event, pi_rpc::protocol::Event::AgentSettled { .. }) =>
                    {
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        };
        prompt("hi");
        // Twice: the second time with what it reasoned before.
        let chatgpt = available
            .iter()
            .find(|model| model.qualified_id().starts_with("openai-codex/"))
            .expect("the ChatGPT login's models");
        block_on(client.request(pi_rpc::protocol::Command::SetModel {
            provider: "openai-codex".into(),
            model_id: chatgpt.id.clone(),
        }))
        .expect("set model");
        prompt("hi ChatGPT");
        prompt("and again");
        let messages = block_on(client.get_messages()).expect("messages");
        drop(process);
        // Without the host's token, or for the relay's own models list, the host gets nothing.
        let refused = ssh
            .exec(&format!(
                "command -v curl >/dev/null || exit 0; \
                 curl -s -m 5 -X POST -H 'authorization: Bearer pistachio-{}-{}' -d '{{}}' \
                   http://127.0.0.1:{}/p/fake/http/127.0.0.1:{port}/v1/chat/completions; echo; \
                 curl -s -m 5 http://127.0.0.1:{}/_pistachio/models",
                "0".repeat(16),
                "0".repeat(64),
                link.port,
                link.port
            ))
            .unwrap();
        eprintln!("without the token: {refused}");
        // An answer that is not a stream never carries this computer's credentials back.
        let echoed = ssh
            .exec(&format!(
                "command -v curl >/dev/null || exit 0; \
                 curl -s -m 5 -H 'authorization: Bearer {token}' \
                   http://127.0.0.1:{}/p/fake/http/127.0.0.1:{port}/v1/echo",
                link.port
            ))
            .unwrap();
        eprintln!("echoed: {echoed}");
        assert!(
            echoed.is_empty() || (echoed.contains("[redacted]") && !echoed.contains("real-")),
            "{echoed}"
        );
        assert!(
            refused.is_empty()
                || (refused.contains("did not come from a Pistachio session")
                    && refused.contains("forbidden")),
            "{refused}"
        );
        // Its pi's own protocol needs the token too, and the token only opens this host's models.
        let post = |token: &str, path: &str, body: &str| {
            ssh.exec(&format!(
                "command -v curl >/dev/null || exit 0; curl -s -m 5 -X POST \
                 -H 'authorization: Bearer {token}' -d '{body}' http://127.0.0.1:{}{path}",
                link.port
            ))
            .unwrap()
        };
        let hello = r#"{"model":"fake-1","context":{"messages":[]}}"#;
        let wrong = format!("pistachio-{}-{}", "0".repeat(16), "0".repeat(64));
        for (token, path, body, answer) in [
            (
                wrong.as_str(),
                "/m/fake/messages",
                hello,
                "did not come from",
            ),
            (
                &token,
                "/m/fake/messages",
                r#"{"model":"fake-1"}"#,
                "no conversation",
            ),
            (
                &token,
                "/m/other/messages",
                hello,
                "has no model other/fake-1",
            ),
            (
                &token,
                "/m/fake/v1/responses",
                hello,
                "does not have this fake model",
            ),
            (
                &token,
                "/_pistachio/tokens",
                r#"{"nonces":[]}"#,
                "forbidden",
            ),
        ] {
            let got = post(token, path, body);
            assert!(got.is_empty() || got.contains(answer), "{path}: {got}");
        }
        let _ = ssh.exec(&format!("rm -rf {}", remote::sh_quote(&scratch)));

        // The relay ended by itself: nothing on this computer may take over its port. The next
        // session gets a new one, over a connection with a new token.
        if tunnel {
            let remote::Endpoint::Port(old) = link.relay else {
                panic!("{:?}", link.relay)
            };
            crate::relay::kill_for_tests();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            while ssh
                .master_pid()
                .is_none_or(|pid| Some(pid) == link.master_pid)
            {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the forward is still there"
                );
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            let listener = std::net::TcpListener::bind(("127.0.0.1", old)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let _ = ssh.exec(&format!(
                "command -v curl >/dev/null && curl -s -m 5 http://127.0.0.1:{}/",
                link.port
            ));
            assert!(
                listener.accept().is_err(),
                "the host still reaches the old port"
            );
            let new = crate::relay::link(&ssh, &host_id, &probe, Some(&link), &launch).unwrap();
            assert_ne!(new.relay, link.relay);
            assert_eq!(new.master_pid, ssh.master_pid());
            assert_ne!(crate::relay::token(&host_id), token);
            ssh.disconnect();
            assert!(!ssh.is_connected());
        }
        let transcript = serde_json::to_string(&messages).unwrap();
        let seen = seen.lock().unwrap().clone();
        eprintln!("the fake server saw: {seen:?}");
        assert!(
            transcript.contains("Hello from this computer."),
            "{transcript}"
        );
        assert!(
            seen.iter()
                .any(|request| request.starts_with("POST /v1/chat/completions")
                    && request.contains("auth=Bearer real-secret-key extra=real-header ")),
            "{seen:?}"
        );
        assert!(transcript.contains("Hello from ChatGPT."), "{transcript}");
        assert!(!transcript.contains("real-"), "{transcript}");
        let chatgpt: Vec<_> = seen
            .iter()
            .filter(|request| request.starts_with("POST /codex/responses"))
            .collect();
        assert!(
            chatgpt.len() == 2
                && chatgpt.iter().all(|request| request.contains(&format!(
                    "auth=Bearer {access} extra= account=real-account "
                ))),
            "{seen:?}"
        );
        assert!(
            chatgpt[1].contains("only-chatgpt-can-read-this"),
            "it gets its reasoning back: {seen:?}"
        );
        crate::relay::stop();
    }

    use gpui_kit::{AppContext as _, TestAppContext};
    use transcript::{BlockKind, ItemKind};

    use super::{Lifecycle, PiSession, SessionEvent};
    use crate::config::AppConfig;

    #[test]
    fn a_conversation_is_named_after_its_message_and_files() {
        use pi_sessions::prompt::block;
        assert_eq!(super::about("Hi there"), "Hi there");
        let file = block("paper.pdf", None, "## Page 1");
        assert_eq!(
            super::about(&format!("{file}\n\nExplain this")),
            "Explain this\n\n(Attached: paper.pdf)"
        );
        assert_eq!(super::about(&file), "(Attached: paper.pdf)");
    }

    /// Adding a model, against real pi (`pi` on PATH): pi starts without the model, the
    /// model is added to models.json the way the dialog does it, pi is restarted (as the
    /// window does after saving), and the new model gets selected.
    /// `cargo test -p pistachio -- --ignored added_model_is_loaded_and_selected`
    #[cfg(unix)]
    #[gpui_kit::test]
    #[ignore = "needs pi on PATH"]
    async fn added_model_is_loaded_and_selected(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let agent = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let mut config = AppConfig::for_tests("pi".into());
        config.child_env = vec![("PI_CODING_AGENT_DIR".into(), agent.path().into())].into();
        cx.update(|cx| cx.set_global(config));

        let wait_ready = async |session: &gpui_kit::Entity<PiSession>, cx: &mut TestAppContext| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while cx.read(|cx| session.read(cx).lifecycle() == &Lifecycle::Starting) {
                assert!(std::time::Instant::now() < deadline, "pi did not start");
                cx.background_executor
                    .timer(std::time::Duration::from_millis(50))
                    .await;
            }
        };
        let before = cx.new(|cx| PiSession::new(project.path().into(), None, None, cx));
        wait_ready(&before, cx).await;
        assert!(cx.read(|cx| {
            before
                .read(cx)
                .models()
                .iter()
                .all(|m| m.provider != "pistachio-test")
        }));

        let mut file = pi_config::ModelsFile::parse("").unwrap();
        let provider = pi_config::ProviderSpec {
            name: "Pistachio test".into(),
            base_url: "http://127.0.0.1:9/v1".into(),
            api: "openai-completions".into(),
            api_key: "none".into(),
        };
        let model = pi_config::ModelSpec {
            id: "added-model".into(),
            name: "Added model".into(),
            context_window: Some(32_000),
            ..pi_config::ModelSpec::default()
        };
        file.add_provider("pistachio-test", &provider).unwrap();
        file.set_model("pistachio-test", &model).unwrap();
        std::fs::write(agent.path().join("models.json"), file.to_text()).unwrap();
        drop(before);

        let after = cx.new(|cx| PiSession::new(project.path().into(), None, None, cx));
        after.update(cx, |session, cx| {
            session.select_model_when_ready("pistachio-test".into(), "added-model".into(), cx);
        });
        wait_ready(&after, cx).await;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let current = cx.read(|cx| {
                after
                    .read(cx)
                    .model()
                    .map(pi_rpc::protocol::Model::qualified_id)
            });
            if current.as_deref() == Some("pistachio-test/added-model") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "selected: {current:?}"
            );
            cx.background_executor
                .timer(std::time::Duration::from_millis(50))
                .await;
        }
        cx.read(|cx| {
            let session = after.read(cx);
            assert_eq!(session.model().unwrap().display_name(), "Added model");
            assert_eq!(session.model().unwrap().context_window, 32_000);
        });
    }

    /// Drives a real `PiSession` against the scripted fake pi from `pi-rpc`'s tests.
    #[cfg(unix)]
    /// Messages typed while pi starts are all sent, in order, one after another, and the
    /// session counts as busy from the first keystroke until the last run settles.
    #[gpui_kit::test]
    async fn prompts_typed_during_startup_are_all_sent(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let fake = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../pi-rpc/tests/fake_pi.py");
        cx.update(|cx| {
            cx.set_global(AppConfig::for_tests(fake));
        });
        let session = cx.new(|cx| PiSession::new(std::env::temp_dir(), None, None, cx));
        let (sender, events) = async_channel::unbounded();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&session, move |_, event: &SessionEvent, _| {
                let _ = sender.try_send(event.clone());
            })
        });
        session.update(cx, |session, cx| {
            assert_eq!(session.lifecycle(), &Lifecycle::Starting);
            session.submit("first".into(), Vec::new(), false, cx);
            session.submit("second".into(), Vec::new(), false, cx);
            assert!(session.is_busy(), "waiting messages keep the chat busy");
        });

        let mut settled = 0;
        while settled < 2 || cx.read(|cx| session.read(cx).is_busy()) {
            if let SessionEvent::Settled = events.recv().await.expect("session alive") {
                settled += 1;
            }
        }
        cx.read(|cx| {
            let prompts: Vec<String> = session
                .read(cx)
                .transcript()
                .items()
                .iter()
                .filter_map(|item| match &item.kind {
                    ItemKind::User(user) => Some(user.text.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(prompts, ["first", "second"]);
        });
    }

    /// While the login shell is still being asked for its environment, a new session opens at
    /// once, shows its history and takes prompts; pi starts with the environment when it is
    /// there.
    #[cfg(unix)]
    #[gpui_kit::test]
    async fn pi_starts_once_the_environment_is_known(cx: &mut TestAppContext) {
        use std::os::unix::fs::PermissionsExt as _;
        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        let fake = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../pi-rpc/tests/fake_pi.py");
        let script = dir.path().join("pi");
        let seen = dir.path().join("seen");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$FAKE_API_KEY\" > '{}'\nexec '{}' \"$@\"\n",
                seen.display(),
                fake.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (release, held) = std::sync::mpsc::channel::<()>();
        let mut config = AppConfig::for_tests(script);
        config.child_env = crate::config::ChildEnv::resolve(move || {
            let _ = held.recv();
            vec![("FAKE_API_KEY".into(), "from the shell".into())]
        });
        cx.update(|cx| cx.set_global(config));

        let session = cx.new(|cx| PiSession::new(std::env::temp_dir(), None, None, cx));
        let (sender, events) = async_channel::unbounded();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&session, move |_, event: &SessionEvent, _| {
                let _ = sender.try_send(event.clone());
            })
        });
        session.update(cx, |session, cx| {
            session.submit("early".into(), Vec::new(), false, cx);
            assert!(session.is_busy());
        });
        cx.run_until_parked();
        assert!(!seen.exists(), "pi must not start yet");
        cx.read(|cx| assert_eq!(session.read(cx).lifecycle(), &Lifecycle::Starting));

        release.send(()).unwrap();
        while !matches!(
            events.recv().await.expect("session alive"),
            SessionEvent::Settled
        ) {}
        assert_eq!(
            std::fs::read_to_string(&seen).unwrap().trim(),
            "from the shell"
        );
        cx.read(|cx| assert_eq!(session.read(cx).lifecycle(), &Lifecycle::Ready));
    }

    /// Prompts typed while pi could not start yet go back to the composer when it can't.
    #[gpui_kit::test]
    async fn prompts_come_back_when_a_delayed_pi_cannot_start(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let (release, held) = std::sync::mpsc::channel::<()>();
        let mut config = AppConfig::for_tests("/nonexistent/pi".into());
        config.child_env = crate::config::ChildEnv::resolve(move || {
            let _ = held.recv();
            Vec::new()
        });
        cx.update(|cx| cx.set_global(config));
        let session = cx.new(|cx| PiSession::new(std::env::temp_dir(), None, None, cx));
        let (sender, events) = async_channel::unbounded();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&session, move |_, event: &SessionEvent, _| {
                let _ = sender.try_send(event.clone());
            })
        });
        session.update(cx, |session, cx| {
            session.submit("early".into(), Vec::new(), false, cx);
        });
        release.send(()).unwrap();
        loop {
            if let SessionEvent::RestoreToEditor { text, .. } =
                events.recv().await.expect("session alive")
            {
                assert_eq!(text, "early");
                break;
            }
        }
        cx.read(|cx| {
            let session = session.read(cx);
            assert!(matches!(session.lifecycle(), Lifecycle::Exited { .. }));
            assert!(!session.is_busy());
        });
    }

    /// Automatic names with the conversation's own model: the name comes from a separate
    /// `pi --print --no-session` (a stand-in script here), is saved through pi, and the
    /// conversation itself only ever sees the user's prompt.
    #[cfg(unix)]
    #[gpui_kit::test]
    async fn first_prompt_is_named_by_a_separate_pi(cx: &mut TestAppContext) {
        use std::os::unix::fs::PermissionsExt as _;
        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        let fake = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../pi-rpc/tests/fake_pi.py");
        let stdin = dir.path().join("naming-stdin");
        let script = dir.path().join("pi");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\ncase \" $* \" in *\" --print \"*)\n  \
                 echo \"$*\" > '{args}'; cat > '{stdin}'; echo 'Title: \"CI build failures.\"'; \
                 exit 0;;\nesac\nexec '{fake}' \"$@\"\n",
                args = dir.path().join("naming-args").display(),
                stdin = stdin.display(),
                fake = fake.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        cx.update(|cx| {
            cx.set_global(AppConfig::for_tests(script.clone()));
            cx.set_global(crate::settings::AppSettings {
                auto_names: true,
                ..crate::settings::AppSettings::default()
            });
        });
        let session = cx.new(|cx| PiSession::new(std::env::temp_dir(), None, None, cx));
        let (sender, events) = async_channel::unbounded();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&session, move |_, event: &SessionEvent, _| {
                let _ = sender.try_send(event.clone());
            })
        });
        // Typed while pi starts: naming waits for the model, the prompt for pi.
        let message = "Please look at why the build fails on CI since this morning";
        session.update(cx, |session, cx| {
            session.submit(message.into(), Vec::new(), false, cx);
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while cx.read(|cx| session.read(cx).title().is_none()) {
            assert!(std::time::Instant::now() < deadline, "no name arrived");
            let _ = events.recv().await;
        }
        cx.read(|cx| {
            let session = session.read(cx);
            assert_eq!(session.title().as_deref(), Some("CI build failures"));
            let prompts: Vec<&str> = session
                .transcript()
                .items()
                .iter()
                .filter_map(|item| match &item.kind {
                    ItemKind::User(user) => Some(user.text.as_str()),
                    _ => None,
                })
                .collect();
            assert!(prompts.len() <= 1 && prompts.iter().all(|p| *p == message));
        });
        let args = std::fs::read_to_string(dir.path().join("naming-args")).unwrap();
        assert!(
            args.contains("--no-session") && args.contains("fake-1"),
            "{args}"
        );
        assert!(std::fs::read_to_string(&stdin).unwrap().contains(message));
    }

    #[gpui_kit::test]
    async fn prompt_round_trip_with_fake_pi(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let fake = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../pi-rpc/tests/fake_pi.py");
        cx.update(|cx| {
            cx.set_global(AppConfig::for_tests(fake));
        });
        let session = cx.new(|cx| PiSession::new(std::env::temp_dir(), None, None, cx));
        let (sender, events) = async_channel::unbounded();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&session, move |_, event: &SessionEvent, _| {
                let _ = sender.try_send(event.clone());
            })
        });

        // Startup handshake: get_state etc. complete and the session is ready.
        while cx.read(|cx| session.read(cx).lifecycle() != &Lifecycle::Ready) {
            events.recv().await.expect("session alive");
        }
        assert_eq!(
            cx.read(|cx| session
                .read(cx)
                .model()
                .map(pi_rpc::protocol::Model::qualified_id)),
            Some("test/fake-1".to_owned())
        );

        session.update(cx, |session, cx| {
            session.submit("hi".into(), Vec::new(), false, cx);
        });
        while !matches!(
            events.recv().await.expect("session alive"),
            SessionEvent::Settled
        ) {}

        cx.read(|cx| {
            let session = session.read(cx);
            let items = session.transcript().items();
            assert!(matches!(&items[0].kind, ItemKind::User(user) if user.text == "hi"));
            let ItemKind::Assistant(assistant) = &items[1].kind else { panic!("{items:?}") };
            assert!(matches!(&assistant.blocks[0].kind, BlockKind::Text(text) if text.text == "Hello \u{2028} world"));
            assert!(matches!(items.last().map(|item| &item.kind), Some(ItemKind::TurnSummary(_))));
            assert!(!session.is_running());
            assert_eq!(session.display_title().as_deref(), Some("hi"));
        });
    }
}
