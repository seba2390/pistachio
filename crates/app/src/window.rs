//! The main window: sidebar | chat (+ terminal below) | right panel.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::resizable::{h_resizable, resizable_panel};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Selectable as _, Sizable as _, TitleBar, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_sessions::SessionSummary;

use pi_sessions::Mode;

use crate::actions::{
    CheckForUpdates, FindInChat, FocusComposer, NewChat, OpenLogin, OpenSettings, ReloadSession,
    ResumeSession, ShowChatMode, ShowCodeMode, ShowDiff, ShowFiles, ToggleRightPanel,
    ToggleSidebar, ToggleTerminal, WINDOW_CONTEXT,
};
use crate::chat::models_dialog::Open as ModelsOpen;
use crate::chat::session::PiSession;
use crate::chat::{ChatEvent, ChatView, ComposerEvent, Draft};
use crate::config::AppConfig;
use crate::hosts::HostsEvent;
use crate::panels::{RightPanel, RightPanelEvent, RightTab, TerminalPanel, TerminalPanelEvent};
use crate::remote_ui::{
    AddHost, AddHostEvent, ConnectFlow, ConnectFlowEvent, EditHost, EditHostEvent, ManageHosts,
    ManageHostsEvent, SavedHostAction,
};
use crate::settings::AppSettings;
use crate::settings::view::{SettingsEvent, SettingsView};
use crate::sidebar::{Activity, Sidebar, SidebarEvent};
use crate::ui::QuietButton as _;
use crate::ui::dialog::{on_submit, open_form_dialog};
use crate::ui::{AppIcon, TOP_BAR_HEIGHT, TRAFFIC_LIGHT_INSET, WindowDrag};
use crate::ui_state::{LastSession, UiState};
use remote::HostId;

mod drafts;
mod side;
#[cfg(test)]
mod tests;
mod update_prompt;

use drafts::{DraftKey, Drafts};

/// Identifies the notification of "Check for updates", so that the result takes its place.
struct UpdateCheck;

/// Registers the window's key bindings and global models, once per process.
pub fn init(cx: &mut App) {
    crate::chat::init(cx);
    crate::panels::terminal_panel::init(cx);
    crate::hosts::init(cx);
}

/// Brings the main window back (Dock icon clicked, or a closed window on macOS), opening a
/// new one only if none exists.
pub fn reopen(cx: &mut App) {
    let windows = cx.windows();
    if windows.is_empty() {
        open_main_window(cx);
    } else {
        for handle in windows {
            let _ = handle.update(cx, |_, window, _| window.activate_window());
        }
    }
    cx.activate(true);
}

/// Hides the app instead of closing its window (macOS): chats that are still working keep
/// running, and clicking the Dock icon brings everything back as it was.
fn keep_alive_on_close(window: &mut Window, cx: &mut App) {
    if cfg!(target_os = "macos") {
        window.on_window_should_close(cx, |_, cx| {
            cx.hide();
            false
        });
    }
}

pub fn open_main_window(cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(1320.), px(860.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(560.), px(420.))),
        app_id: Some("pistachio".into()),
        ..TitleBar::window_options()
    };
    // GPUI caps a window that is not focused at 30 fps, so hovering it while another app
    // is in front (a common case) answered up to 33 ms late. Frames are still drawn only
    // when something changes; 60 fps keeps hover snappy. A benchmark run, which macOS may
    // keep in the background, measures what an active window does.
    let options = WindowOptions {
        inactive_frame_interval: (!crate::perf::bench::requested())
            .then_some(std::time::Duration::from_micros(16_667)),
        ..options
    };
    let opened = open_window(options, cx, |window, cx| {
        crate::theme::sync_with_window(window, cx);
        // Shown by X11 window managers and in task lists; macOS shows the app's name itself.
        if cfg!(target_os = "linux") {
            window.set_window_title("Pistachio");
        }
        keep_alive_on_close(window, cx);
        crate::perf::frame("startup: first frame", window);
        window.on_next_frame(|_, cx| {
            cx.spawn(async |cx| cx.update(crate::actions::install_menus))
                .detach();
        });
        cx.new(|cx| MainWindow::new(window, cx))
    });
    if let Err(error) = opened {
        log::error!("failed to open the main window: {error:#}");
    }
}

/// Layout of a cached child view: views only re-render when they notify, so streaming
/// in the chat never re-renders the sidebar, panels or terminal (and vice versa).
fn full() -> StyleRefinement {
    StyleRefinement::default().size_full()
}

/// Moves keyboard focus to `chat`'s composer.
fn focus_chat(chat: &Entity<ChatView>, window: &mut Window, cx: &mut App) {
    let focus = chat.read(cx).composer_focus(cx);
    focus.focus(window, cx);
}

/// What changed on disk that pi reads only when it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReloadReason {
    Models,
    Skills,
    /// The user asked for it (`/reload`).
    Asked,
}

/// A restart of one chat's pi that waits for its agent to settle, and the model to select
/// afterwards.
#[derive(Debug, Clone)]
struct PendingReload {
    /// The [`ChatView`] to restart; other chats are never touched.
    chat: EntityId,
    select: Option<(String, String)>,
}

/// A session on an SSH host to open once the host is connected.
#[derive(Debug, Clone)]
struct RemoteTarget {
    host: HostId,
    folder: PathBuf,
    /// The session file to resume; `None` starts a new session.
    file: Option<PathBuf>,
}

/// The conversation last shown in a mode, to come back to when switching to that mode.
#[derive(Debug, Clone)]
struct LastShown {
    chat: EntityId,
    /// Where it was, if nothing was sent in it yet: a new session there takes its place.
    new: Option<(Option<HostId>, PathBuf)>,
}

/// How long pi may take to start and rename a session that is not open.
const RENAME_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

#[allow(clippy::struct_excessive_bools)] // independent layout flags
pub struct MainWindow {
    sidebar: Entity<Sidebar>,
    chat: Entity<ChatView>,
    /// Chats switched away from while busy (a run, or messages not yet started) or waiting
    /// for an answer; kept alive until they settle so work is never lost to navigation.
    background: Vec<(Entity<ChatView>, Subscription)>,
    /// What was typed but not sent in chats that were let go.
    drafts: Drafts,
    /// Each mode's conversation when it was last switched away from (in this run only;
    /// [`UiState`] remembers the sessions pi has written).
    last_shown: HashMap<Mode, LastShown>,
    /// The new session whose composer opened the connect (or "Add SSH connection") dialog:
    /// its draft moves to the session started there.
    moving: Option<EntityId>,
    /// Renames on their way to pi.
    renames: HashMap<(Option<HostId>, PathBuf), Task<()>>,
    /// Sessions that finished in the background and have not been looked at since.
    finished: HashSet<(Option<HostId>, PathBuf)>,
    right_panel: Entity<RightPanel>,
    terminal: Option<Entity<TerminalPanel>>,
    /// Terminals of other folders or machines, kept running (a dev server, a long build)
    /// until their shell exits; shown again when their folder's terminal is opened.
    parked_terminals: Vec<Entity<TerminalPanel>>,
    /// Events of the open SSH dialog (add host / connect flow / manage hosts).
    dialog_subscription: Option<Subscription>,
    /// The remote session to open when its host is ready: from the connect dialog's
    /// "connect, then open" mode, or restored at startup (`quiet`: no dialog).
    pending_remote: Option<(RemoteTarget, bool)>,
    /// `PISTACHIO_OPEN` panels to open once a remote session is shown.
    after_remote: Vec<String>,
    /// `PISTACHIO_OPEN=remote-latest:…`: open this host's newest session once it is listed.
    pending_latest: Option<HostId>,
    /// models.json changed while the agent worked: restart pi (and select this model) once
    /// it settles.
    models_reload: Vec<PendingReload>,
    /// The Skills page, shown in the chat column instead of the chat while `skills_open`.
    skills: Option<(Entity<crate::skills::SkillsView>, Subscription)>,
    skills_open: bool,
    /// The Settings page, shown in the chat column while set.
    settings: Option<(Entity<SettingsView>, Subscription)>,
    /// Set while pi restarts for the Skills page, so showing the new chat keeps the page.
    keep_skills: bool,
    /// The welcome screen is showing (no session open yet; `chat` is an idle placeholder).
    welcome: bool,
    /// The welcome screen's animated background.
    pi_field: Entity<crate::ui::PiField>,
    sidebar_open: bool,
    right_open: bool,
    terminal_open: bool,
    /// The terminal's height (dragged with its top edge; remembered across launches).
    terminal_height: Pixels,
    drag: WindowDrag,
    focus_handle: FocusHandle,
    chat_subscription: Subscription,
    /// The side chat next to the visible one (see [`side`]).
    side: Option<side::SideChat>,
    /// A side chat's file being written.
    side_opening: Option<Task<()>>,
    /// Demo mode: the cursor overlay and the storyboard being played.
    demo: Option<(Entity<crate::demo::DemoCursor>, Task<()>)>,
    /// `PISTACHIO_BENCH`: the navigation benchmark being run.
    bench: Option<Task<()>>,
    /// The update prompt or "What's new" card, centered on the window.
    update_prompt: Option<update_prompt::Prompt>,
    /// What is drawn behind it: the window, blurred.
    update_backdrop: update_prompt::Backdrop,
    _subscriptions: Vec<Subscription>,
}

impl MainWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(|cx| Sidebar::new(window, cx));
        // Start on the welcome screen: no pi process and no folder until the user picks one
        // (starting in the launch folder, often `~` or `/`, was surprising and made the file
        // index walk the whole home folder).
        let mode = sidebar.read(cx).mode();
        let cwd = cx
            .global::<AppConfig>()
            .home
            .clone()
            .unwrap_or_else(std::env::temp_dir);
        let chat = cx.new(|cx| ChatView::idle(cwd.clone(), mode, window, cx));
        let right_panel = cx.new(|cx| RightPanel::new(cwd, window, cx));
        let chat_subscription = cx.subscribe_in(&chat, window, Self::on_chat_event);
        let document_parsers = crate::document_parsers::document_parsers(cx);
        let subscriptions = vec![
            cx.subscribe_in(&sidebar, window, Self::on_sidebar_event),
            cx.subscribe_in(
                &right_panel,
                window,
                |this, _, event: &RightPanelEvent, window, cx| {
                    let RightPanelEvent::InsertPath(path) = event;
                    let mention = format!(
                        "{} ",
                        crate::chat::mention::file_reference(&path.display().to_string())
                    );
                    let chat = this.chat.clone();
                    chat.read(cx).composer().clone().update(cx, |composer, cx| {
                        composer.insert_text(&mention, window, cx);
                    });
                },
            ),
            cx.observe_window_appearance(window, |_, window, cx| {
                crate::theme::sync_with_window(window, cx);
            }),
            cx.subscribe_in(
                &crate::stt::models::models(cx),
                window,
                |_, _, event: &crate::stt::models::SpeechModelsEvent, window, cx| {
                    crate::stt::ui::notify_download(event, window, cx);
                },
            ),
            cx.subscribe_in(
                &crate::search_server::search_server(cx),
                window,
                |_, _, event: &crate::search_server::SearchServerEvent, window, cx| {
                    crate::search_server::notify(event, window, cx);
                },
            ),
            cx.subscribe_in(
                &document_parsers,
                window,
                |_, _, event: &crate::document_parsers::DocumentParsersEvent, window, cx| {
                    crate::document_parsers::notify(event, window, cx);
                },
            ),
            cx.subscribe_in(
                &crate::naming::models(cx),
                window,
                |_, _, event: &crate::naming::NamingModelsEvent, window, cx| {
                    crate::naming::ui::notify_download(event, window, cx);
                },
            ),
            cx.subscribe_in(
                &crate::hosts::hosts(cx),
                window,
                |this, hosts, event: &HostsEvent, window, cx| match event {
                    HostsEvent::Prompt => {
                        while let Some(prompt) = hosts.update(cx, |hosts, _| hosts.take_prompt()) {
                            crate::remote_ui::open_prompt(prompt, window, cx);
                        }
                    }
                    HostsEvent::Changed => this.check_pending_remote(window, cx),
                    HostsEvent::SessionsChanged(host) => {
                        this.check_pending_remote(window, cx);
                        if this.pending_latest.as_ref() == Some(host) {
                            let latest = hosts.read(cx).sessions(host).first().cloned();
                            if let Some(latest) = latest {
                                this.pending_latest = None;
                                let target = RemoteTarget {
                                    host: host.clone(),
                                    folder: latest.cwd,
                                    file: Some(latest.path),
                                };
                                this.open_remote(target, window, cx);
                            }
                        }
                    }
                },
            ),
        ];
        // The window starts on the welcome screen, which has no message box: focus the window
        // itself, so its actions (Settings…, Check for Updates…, ⌘N…) are available in the
        // menu bar and from the keyboard right away. Whenever focus is lost later (a focused
        // control went away), it comes back here for the same reason.
        let focus_handle = cx.focus_handle();
        let mut subscriptions = subscriptions;
        subscriptions.push(cx.on_focus_lost(window, |this, window, cx| {
            this.focus_handle.focus(window, cx);
        }));
        let open_at_startup = cx.global::<AppConfig>().open_at_startup.clone();
        let start_focus = focus_handle.clone();
        window.defer(cx, move |window, cx| start_focus.focus(window, cx));
        let this = cx.weak_entity();
        let (reopen, check_updates) = {
            let settings = cx.default_global::<AppSettings>();
            (settings.reopen_last_session, settings.check_for_updates)
        };
        let scripted = cx.global::<AppConfig>().scripted();
        window.defer(cx, move |window, cx| {
            let _ = this.update(cx, |this, cx| {
                if reopen && !scripted {
                    this.reopen_last_local_session(window, cx);
                }
                this.open_panels(&open_at_startup, window, cx);
                if !scripted {
                    this.show_whats_new_if_updated(window, cx);
                    cx.background_executor()
                        .spawn(async { crate::updater::clean_up() })
                        .detach();
                }
                if check_updates && !scripted {
                    this.check_for_updates(false, window, cx);
                }
            });
        });
        // With "Reopen the last session", a Code session on an SSH host comes back after a
        // restart (once the host connected quietly; nothing happens if it cannot be reached).
        let restore = {
            let state = cx.default_global::<UiState>();
            (reopen && !scripted && state.mode == Mode::Code && mode == Mode::Code)
                .then(|| state.last_sessions.get(&Mode::Code).cloned())
                .flatten()
                .and_then(|last| {
                    Some(RemoteTarget {
                        host: HostId(last.host?),
                        folder: last.folder?,
                        file: Some(last.file),
                    })
                })
                .filter(|target| crate::hosts::hosts(cx).read(cx).get(&target.host).is_some())
        };
        // Its host may not connect at start on its own (that is a separate setting).
        if let Some(target) = &restore {
            let host = target.host.clone();
            crate::hosts::hosts(cx).update(cx, |hosts, cx| hosts.connect_quietly(&host, cx));
        }
        let demo = cx.global::<AppConfig>().demo.clone().map(|storyboard| {
            let cursor = cx.new(|_| crate::demo::DemoCursor::new(point(px(0.), px(0.))));
            let task = crate::demo::start(storyboard, cursor.clone(), window, cx);
            (cursor, task)
        });
        Self {
            sidebar,
            chat,
            background: Vec::new(),
            drafts: Drafts::default(),
            last_shown: HashMap::new(),
            moving: None,
            renames: HashMap::new(),
            right_panel,
            terminal: None,
            parked_terminals: Vec::new(),
            dialog_subscription: None,
            update_prompt: None,
            update_backdrop: update_prompt::Backdrop::None,
            pending_remote: restore.map(|target| (target, true)),
            after_remote: Vec::new(),
            pending_latest: None,
            welcome: true,
            finished: HashSet::new(),
            pi_field: cx.new(|_| crate::ui::PiField::new()),
            models_reload: Vec::new(),
            skills: None,
            skills_open: false,
            settings: None,
            keep_skills: false,
            sidebar_open: true,
            right_open: false,
            terminal_open: false,
            terminal_height: px(cx
                .default_global::<UiState>()
                .terminal_height
                .map_or(DEFAULT_TERMINAL_HEIGHT, f32::from)),
            drag: WindowDrag::default(),
            focus_handle,
            chat_subscription,
            side: None,
            side_opening: None,
            demo,
            bench: None,
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn sidebar_entity(&self) -> &Entity<Sidebar> {
        &self.sidebar
    }

    /// The conversation in the chat column (demo mode drives it).
    pub(crate) fn visible_chat(&self) -> &Entity<ChatView> {
        &self.chat
    }

    /// The welcome screen is showing instead of a conversation.
    pub(crate) fn welcome_visible(&self) -> bool {
        self.welcome && !self.page_open()
    }

    /// Opens panels and dialogs named in `PISTACHIO_OPEN` (scripted screenshots and checks).
    fn open_panels(&mut self, panels: &[String], window: &mut Window, cx: &mut Context<Self>) {
        for (index, panel) in panels.iter().enumerate() {
            // Panels need a session: scripted runs open one in the launch folder.
            if self.welcome
                && matches!(
                    panel.as_str(),
                    "terminal"
                        | "diff"
                        | "diff-expand-all"
                        | "files"
                        | "attach"
                        | "context"
                        | "question"
                        | "question-plain"
                )
                && let Ok(folder) = std::env::current_dir()
            {
                self.new_chat_in(folder, window, cx);
            }
            match panel.as_str() {
                "terminal" => self.toggle_terminal(&ToggleTerminal, window, cx),
                "diff" => self.show_diff(&ShowDiff, window, cx),
                "diff-expand-all" => {
                    self.right_panel
                        .update(cx, |panel, cx| panel.expand_all_diffs(cx));
                    self.show_diff(&ShowDiff, window, cx);
                }
                "expand-tools" => {
                    let chat = self.chat.clone();
                    chat.update(cx, |chat, cx| chat.set_expand_tools(true, cx));
                }
                "files" => self.show_files(&ShowFiles, window, cx),
                "no-sidebar" => self.toggle_sidebar(&ToggleSidebar, window, cx),
                "chat" => self.switch_to_mode(Mode::Chat, window, cx),
                // Once the restored conversation has loaded.
                "side-chat" => cx
                    .spawn_in(window, async move |this, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(2))
                            .await;
                        let _ = this.update_in(cx, |this, window, cx| {
                            this.open_side_chat(&crate::actions::OpenSideChat, window, cx);
                        });
                    })
                    .detach(),
                "add-host" => self.open_add_host(window, cx),
                "manage-hosts" => self.open_manage_hosts(window, cx),
                "skills" => self.toggle_skills(window, cx),
                "settings" => self.open_settings(&OpenSettings, window, cx),
                "settings-models" => {
                    self.open_settings(&OpenSettings, window, cx);
                    if let Some(settings) = self.settings.as_ref().map(|s| s.0.clone()) {
                        settings.update(cx, |settings, _| settings.start_on_models());
                    }
                }
                "settings-web" => {
                    self.open_settings(&OpenSettings, window, cx);
                    if let Some(settings) = self.settings.as_ref().map(|s| s.0.clone()) {
                        settings.update(cx, |settings, _| settings.start_on_web_search());
                    }
                }
                "web-advanced" => {
                    if let Some(settings) = self.settings.as_ref().map(|s| s.0.clone()) {
                        settings.update(cx, |settings, _| settings.open_advanced());
                    }
                }
                "searxng-setup" => crate::settings::open_searxng_setup(window, cx),
                "searxng-get" => crate::settings::use_local_searxng(cx),
                "settings-ssh" => {
                    self.open_settings(&OpenSettings, window, cx);
                    if let Some(settings) = self.settings.as_ref().map(|s| s.0.clone()) {
                        settings.update(cx, |settings, _| settings.start_on_ssh());
                    }
                }
                "settings-speech" => {
                    self.open_settings(&OpenSettings, window, cx);
                    if let Some(settings) = self.settings.as_ref().map(|s| s.0.clone()) {
                        settings.update(cx, |settings, _| settings.start_on_speech());
                    }
                }
                "settings-names" => {
                    self.open_settings(&OpenSettings, window, cx);
                    if let Some(settings) = self.settings.as_ref().map(|s| s.0.clone()) {
                        settings.update(cx, |settings, _| settings.start_on_names());
                    }
                }
                "settings-documents" => {
                    self.open_settings(&OpenSettings, window, cx);
                    if let Some(settings) = self.settings.as_ref().map(|s| s.0.clone()) {
                        settings.update(cx, |settings, _| settings.start_on_documents());
                    }
                }
                "settings-security" => {
                    self.open_settings(&OpenSettings, window, cx);
                    if let Some(settings) = self.settings.as_ref().map(|s| s.0.clone()) {
                        settings.update(cx, |settings, _| settings.start_on_security());
                    }
                }
                "settings-tools" => {
                    self.open_settings(&OpenSettings, window, cx);
                    if let Some(settings) = self.settings.as_ref().map(|s| s.0.clone()) {
                        settings.update(cx, |settings, _| settings.start_on_tools());
                    }
                }
                // The sandbox gate's question, docked above the composer: with a warning (open
                // at first) or without (one line).
                "question" | "question-plain" => {
                    let warning = if panel == "question" {
                        "\n⚠ main is the remote's default branch"
                    } else {
                        ""
                    };
                    let request = pi_rpc::protocol::ExtensionUiRequest {
                        id: "layout-check".into(),
                        method: pi_rpc::protocol::ExtensionUiMethod::Select {
                            title: format!(
                                "Push 1 commit on feature to github.com/acme/fibonacci as ana\n\
                                 · 1e6145f Initial commit: Fibonacci sequence printer{warning}"
                            ),
                            options: ["Allow", "Always for this project", "Don't allow"]
                                .map(str::to_owned)
                                .to_vec(),
                            timeout: None,
                        },
                    };
                    self.chat
                        .update(cx, |chat, cx| chat.ask(&request, window, cx));
                }
                // A `request_user_help` pill, folded (`help-request`) or open (`help-request-open`).
                "help-request" | "help-request-open" => {
                    let title = format!(
                        "{}{}",
                        sandbox::help::MARK,
                        serde_json::json!({
                            "title": "Install the Xcode command line tools",
                            "reason": "Building needs clang, and installing the command line tools changes \
                                       /Library, which the sandbox doesn't let commands write.",
                            "suggested_command": "xcode-select --install",
                            "blocked": {
                                "command": "xcode-select --install",
                                "cwd": "/Users/ana/code/fibonacci",
                                "error": "xcode-select: error: Operation not permitted",
                            },
                        })
                    );
                    let request = pi_rpc::protocol::ExtensionUiRequest {
                        id: "layout-check-help".into(),
                        method: pi_rpc::protocol::ExtensionUiMethod::Select {
                            title,
                            options: [sandbox::help::DONE, sandbox::help::DISMISS]
                                .map(str::to_owned)
                                .to_vec(),
                            timeout: None,
                        },
                    };
                    self.chat.update(cx, |chat, cx| {
                        chat.ask(&request, window, cx);
                        if panel == "help-request-open" {
                            chat.expand_help(cx);
                        }
                    });
                }
                "settings-git" => {
                    self.open_settings(&OpenSettings, window, cx);
                    if let Some(settings) = self.settings.as_ref().map(|s| s.0.clone()) {
                        settings.update(cx, |settings, _| settings.start_on_git());
                    }
                }
                // `settings-at:<page>:<section>`: Settings on that page (0 is General), scrolled
                // to that section (for the documentation's screenshots).
                other if other.starts_with("settings-at:") => {
                    let mut numbers = other["settings-at:".len()..]
                        .split(':')
                        .map(|number| number.parse::<usize>().ok());
                    let page = numbers.next().flatten().unwrap_or(0);
                    let group = numbers.next().flatten();
                    self.open_settings(&OpenSettings, window, cx);
                    if let Some(settings) = self.settings.as_ref().map(|s| s.0.clone()) {
                        settings.update(cx, |settings, _| settings.start_on(page, group));
                    }
                }
                "speech-download" => crate::stt::ui::open_download_dialog(window, cx),
                "dictate" => {
                    let composer = self.chat.read(cx).composer().clone();
                    composer.update(cx, |composer, cx| composer.toggle_dictation(window, cx));
                }
                other if other.starts_with("speech-get:") => {
                    if let Some(model) = crate::stt::catalog::model(&other["speech-get:".len()..]) {
                        crate::stt::ui::start_download(model, cx);
                    }
                }
                // `download-ask:<id>`: asks whether to download that model or parser.
                other if other.starts_with("download-ask:") => {
                    let id = &other["download-ask:".len()..];
                    if let Some(model) = crate::stt::catalog::model(id) {
                        crate::stt::ui::ask_to_download(model, window, cx);
                    } else if let Some(model) = crate::naming::catalog::model(id) {
                        crate::naming::ui::ask_to_download(model, window, cx);
                    } else if let Some(parser) = documents::Parser::from_id(id) {
                        crate::settings::ask_to_download_parser(parser, window, cx);
                    }
                }
                // `view-file:<path>`: the Files tab's viewer on that file (relative to the folder).
                other if other.starts_with("view-file:") => {
                    let path = PathBuf::from(&other["view-file:".len()..]);
                    if self.folder_tools(cx) {
                        self.open_in_files(path, None, window, cx);
                    }
                }
                other if other == "report-check" || other.starts_with("image-viewer:") => {
                    let start = other
                        .strip_prefix("image-viewer:")
                        .map(|start| start.parse().unwrap_or(0));
                    // The session loads in the background; a scripted check waits (up to a few
                    // seconds) for its pictures or report.
                    cx.spawn_in(window, async move |this, cx| {
                        for _ in 0..50 {
                            let opened = this
                                .update_in(cx, |this, window, cx| {
                                    let chat = this.chat.clone();
                                    match start {
                                        Some(start) => {
                                            ChatView::open_images(&chat, start, window, cx)
                                        }
                                        None => ChatView::open_report_check(&chat, window, cx),
                                    }
                                })
                                .unwrap_or(true);
                            if opened {
                                break;
                            }
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(100))
                                .await;
                        }
                    })
                    .detach();
                }
                // Plays the sidebar's typing animation for the open session's row, as if an
                // automatic name arrived 2.5 s after start (the name is not saved).
                other if other.starts_with("type-name:") => {
                    let title = other["type-name:".len()..].to_owned();
                    let key = session_key(&self.chat, cx);
                    cx.spawn(async move |_, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(2500))
                            .await;
                        if let Some((host, path)) = key {
                            cx.update(|cx| crate::naming::announce(host, Some(path), title, cx));
                        }
                    })
                    .detach();
                }
                other if other.starts_with("send:") => {
                    let text = other["send:".len()..].to_owned();
                    let composer = self.chat.read(cx).composer().clone();
                    composer.update(cx, |composer, cx| composer.send_text(&text, window, cx));
                }
                other if other.starts_with("attach:") => {
                    let path = other["attach:".len()..].into();
                    let composer = self.chat.read(cx).composer().clone();
                    composer.update(cx, |composer, cx| composer.attach_paths(vec![path], cx));
                }
                other if other.starts_with("view-text:") => {
                    let path = other["view-text:".len()..].into();
                    crate::chat::text_viewer::open(path, window, cx);
                }
                other if other.starts_with("view-pdf:") => {
                    let path = other["view-pdf:".len()..].into();
                    crate::chat::pdf_viewer::open(path, window, cx);
                }
                other if other.starts_with("find:") => {
                    let chat = self.chat.clone();
                    let text = other["find:".len()..].to_owned();
                    chat.update(cx, |chat, cx| chat.find_text(&text, window, cx));
                }
                other if other.starts_with("open-skill:") => {
                    let path = other.trim_start_matches("open-skill:").to_owned();
                    self.toggle_skills(window, cx);
                    if let Some((skills, _)) = &self.skills {
                        skills.update(cx, |skills, cx| skills.open_skill(path, window, cx));
                    }
                }
                "new-skill" => {
                    self.toggle_skills(window, cx);
                    if let Some((skills, _)) = &self.skills {
                        skills.update(cx, |skills, cx| skills.start_new(window, cx));
                    }
                }
                "models" => self.open_models_dialog(ModelsOpen::Start, None, window, cx),
                "edit-models" => self.open_models_dialog(ModelsOpen::Json, None, window, cx),
                other if other.starts_with("models-provider:") => {
                    let open = ModelsOpen::Provider(other["models-provider:".len()..].to_owned());
                    self.open_models_dialog(open, None, window, cx);
                }
                other if other.starts_with("models-server:") => {
                    let open = ModelsOpen::Server(other["models-server:".len()..].to_owned());
                    self.open_models_dialog(open, None, window, cx);
                }
                other if other.starts_with("ssh:") => {
                    let alias = other.trim_start_matches("ssh:").to_owned();
                    let host = add_alias_host(alias, cx);
                    self.open_connect_flow(host, window, cx);
                }
                // `remote:<ssh alias>:<folder>`: a new session there once connected.
                // `remote:<ssh alias>:<folder>`: a new session there once connected; the
                // panels after it apply to that session.
                other if other.starts_with("remote:") => {
                    let rest = other.trim_start_matches("remote:");
                    if let Some((alias, folder)) = rest.split_once(':') {
                        let host = add_alias_host(alias.to_owned(), cx);
                        let target = RemoteTarget {
                            host,
                            folder: PathBuf::from(folder),
                            file: None,
                        };
                        self.after_remote = panels[index + 1..].to_vec();
                        self.open_remote(target, window, cx);
                        return;
                    }
                }
                // `remote-latest:<ssh alias>`: the newest session on that host, once listed.
                other if other.starts_with("remote-latest:") => {
                    let alias = other.trim_start_matches("remote-latest:").to_owned();
                    let host = add_alias_host(alias, cx);
                    crate::hosts::hosts(cx).update(cx, |hosts, cx| hosts.connect(&host, false, cx));
                    self.after_remote = panels[index + 1..].to_vec();
                    self.pending_latest = Some(host);
                    return;
                }
                "code" => self.switch_to_mode(Mode::Code, window, cx),
                "new-chat" => self.new_chat(&NewChat, window, cx),
                "latest" => {
                    let dir = cx.global::<AppConfig>().sessions_dir.clone();
                    if let Some(latest) =
                        dir.and_then(|dir| pi_sessions::scan(&dir).into_iter().next())
                    {
                        drop(self.open_session(None, &latest, window, cx));
                    }
                }
                "context" => {
                    let composer = self.chat.read(cx).composer().clone();
                    composer.update(cx, |composer, cx| composer.open_context_breakdown(cx));
                }
                "attach" => {
                    let composer = self.chat.read(cx).composer().clone();
                    composer.update(cx, |composer, cx| composer.open_attach_picker(window, cx));
                }
                "bench" => self.bench = Some(crate::perf::bench::start(window, cx)),
                other => log::warn!("PISTACHIO_OPEN: unknown panel {other:?}"),
            }
        }
    }

    fn cwd(&self, cx: &App) -> PathBuf {
        self.chat.read(cx).session().read(cx).cwd().to_owned()
    }

    /// The visible conversation is a Code session on this computer.
    fn local_code(&self, cx: &App) -> bool {
        !self.welcome && self.chat.read(cx).session().read(cx).is_local_code()
    }

    /// The visible conversation has a project folder (here or on an SSH host), so the
    /// terminal and the diff / files panels apply.
    /// A file an answer or a tool row names: the Files tab shows it (at its line).
    fn open_file_link(
        &mut self,
        link: &crate::chat::OpenFileLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.folder_tools(cx) {
            return;
        }
        self.open_in_files(PathBuf::from(&link.path), link.line, window, cx);
    }

    /// Shows a file in the Files tab, opening the panel or the tab if needed. (Not through
    /// [`Self::show_files`], which closes the panel when that tab is already shown.)
    fn open_in_files(
        &mut self,
        path: PathBuf,
        line: Option<u32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.right_panel
            .update(cx, |panel, cx| panel.open_file_at(path, line, window, cx));
        self.set_right_open(true, cx);
    }

    fn folder_tools(&self, cx: &App) -> bool {
        !self.welcome && self.chat.read(cx).session().read(cx).has_folder_tools()
    }

    /// The SSH host of the visible conversation.
    fn host(&self, cx: &App) -> Option<HostId> {
        self.chat.read(cx).session().read(cx).host().cloned()
    }

    /// The mode of the visible conversation.
    fn mode(&self, cx: &App) -> Mode {
        self.chat.read(cx).session().read(cx).mode()
    }

    /// A new conversation in `mode`: a chat in the chat folder. A Code session never takes a
    /// folder by itself (that could be the home folder, where listing files makes macOS ask
    /// for Downloads, Documents and the rest): the welcome screen asks which, and when it is
    /// already shown, the system's folder dialog.
    fn new_in_default_folder(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        match mode {
            Mode::Chat => {
                let folder = cx.global::<AppConfig>().chat_dir.clone();
                self.new_chat_in(folder, window, cx);
            }
            Mode::Code if self.welcome_visible() => self.choose_folder(None, window, cx),
            Mode::Code => {
                self.close_pages(cx);
                self.show_welcome(Mode::Code, window, cx);
            }
        }
    }

    /// The system folder dialog; a new Code session starts in the chosen folder, with the
    /// draft of `moving` (a new session) if that is still shown.
    fn choose_folder(
        &mut self,
        moving: Option<EntityId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = paths.await
                && let Some(folder) = paths.pop()
            {
                let _ = this.update_in(cx, |this, window, cx| {
                    let draft = this.take_moving_draft(moving, window, cx);
                    this.show_new(None, folder, draft, window, cx);
                });
            }
        })
        .detach();
    }

    /// Shows the welcome screen for `mode` (nothing running until the user starts something).
    fn show_welcome(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = cx
            .global::<AppConfig>()
            .home
            .clone()
            .unwrap_or_else(std::env::temp_dir);
        let chat = cx.new(|cx| ChatView::idle(cwd, mode, window, cx));
        self.show_chat(chat, window, cx);
        if !self.welcome {
            self.pi_field.update(cx, |field, cx| field.replay(cx));
        }
        self.welcome = true;
        self.terminal_open = false;
        if self.right_open {
            self.set_right_open(false, cx);
        }
        cx.notify();
    }

    /// Shows `mode`: the conversation last open in it, or a new one. Chat has no project
    /// folder, so the terminal and side panel close.
    fn switch_to_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_mode(mode, window, cx));
        if self.mode(cx) == mode {
            // Already this mode: back to its chat from the Skills or Settings page.
            self.close_pages(cx);
            return;
        }
        if mode == Mode::Chat {
            self.terminal_open = false;
            self.set_right_open(false, cx);
            if self.skills_open {
                self.close_skills(cx);
            }
        }
        if self.show_last_shown(mode, window, cx) {
            return;
        }
        let last = cx
            .default_global::<UiState>()
            .last_sessions
            .get(&mode)
            .cloned();
        match last {
            // A remote session resumes on its host.
            Some(LastSession {
                file,
                host: Some(host),
                folder: Some(folder),
            }) if crate::hosts::hosts(cx)
                .read(cx)
                .get(&HostId(host.clone()))
                .is_some() =>
            {
                let target = RemoteTarget {
                    host: HostId(host),
                    folder,
                    file: Some(file),
                };
                self.open_remote(target, window, cx);
            }
            Some(LastSession {
                file, host: None, ..
            }) if let Some(summary) = self.sidebar.read(cx).summary(&file) => {
                drop(self.open_session(None, &summary, window, cx));
            }
            // Nothing to come back to: the welcome screen for that mode.
            _ => self.show_welcome(mode, window, cx),
        }
    }

    /// Shows the conversation last shown in `mode` if only this run knows it: one still
    /// working in the background, or a new one nothing was sent in. Returns whether it did.
    fn show_last_shown(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(last) = self.last_shown.get(&mode).cloned() else {
            return false;
        };
        let working = self
            .background
            .iter()
            .find(|(chat, _)| chat.entity_id() == last.chat);
        if let Some((chat, _)) = working {
            self.show_chat(chat.clone(), window, cx);
            return true;
        }
        match last.new {
            Some((Some(host), folder)) if crate::hosts::hosts(cx).read(cx).get(&host).is_some() => {
                let target = RemoteTarget {
                    host,
                    folder,
                    file: None,
                };
                self.open_remote(target, window, cx);
                true
            }
            Some((None, folder)) if mode == Mode::Chat || folder.is_dir() => {
                self.new_chat_in(folder, window, cx);
                true
            }
            _ => false,
        }
    }

    fn show_chat_mode(&mut self, _: &ShowChatMode, window: &mut Window, cx: &mut Context<Self>) {
        self.switch_to_mode(Mode::Chat, window, cx);
    }

    fn show_code_mode(&mut self, _: &ShowCodeMode, window: &mut Window, cx: &mut Context<Self>) {
        self.switch_to_mode(Mode::Code, window, cx);
    }

    /// Replaces the visible chat, keeping the old one alive if it is still working. Otherwise
    /// only its draft is kept, until its session is shown again.
    fn show_chat(&mut self, chat: Entity<ChatView>, window: &mut Window, cx: &mut Context<Self>) {
        let previous = std::mem::replace(&mut self.chat, chat.clone());
        self.welcome = false;
        if previous.entity_id() != chat.entity_id() {
            previous.update(cx, |previous, cx| previous.set_visible(false, window, cx));
        }
        chat.update(cx, |chat, cx| chat.set_visible(true, window, cx));
        if let Some(key) = session_key(&chat, cx) {
            self.finished.remove(&key);
        }
        remember(&previous, cx);
        remember(&chat, cx);
        self.close_settings(cx);
        if self.skills_open {
            if self.keep_skills {
                let session = chat.read(cx).session().clone();
                if let Some((skills, _)) = &self.skills {
                    skills.update(cx, |skills, cx| skills.set_session(session, cx));
                }
            } else {
                self.close_skills(cx);
            }
        }
        // Opening a conversation of the other mode (e.g. at startup) switches the sidebar too.
        let mode = chat.read(cx).session().read(cx).mode();
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_mode(mode, window, cx));
        if !chat.read(cx).session().read(cx).has_folder_tools() {
            self.terminal_open = false;
            if self.right_open {
                self.set_right_open(false, cx);
            }
        }
        if previous.entity_id() != chat.entity_id() {
            self.remember_last_shown(&previous, cx);
            if !keeps_working(&previous, cx) {
                self.keep_draft(&previous, window, cx);
            } else if !self
                .background
                .iter()
                .any(|(background, _)| background.entity_id() == previous.entity_id())
            {
                // Keep it alive until it settles, then let it (and its pi process) go.
                let subscription = cx.subscribe_in(&previous, window, Self::on_background_event);
                self.background.push((previous, subscription));
            }
        }
        self.background
            .retain(|(background, _)| background.entity_id() != chat.entity_id());
        self.let_replaced_go(&chat, window, cx);
        self.prune_background(window, cx);
        self.restore_draft(&chat, window, cx);
        self.chat_subscription = cx.subscribe_in(&chat, window, Self::on_chat_event);

        let cwd = self.cwd(cx);
        let host = self.host(cx);
        let session_file = chat.read(cx).session().read(cx).session_file();
        let location = self.location(cx);
        self.right_panel
            .update(cx, |panel, cx| panel.set_cwd(cwd.clone(), location, cx));
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.set_active(session_file, host.clone(), cx);
        });
        // A terminal for another folder or machine is replaced when it is next shown.
        if self.terminal_open
            && !self.terminal.as_ref().is_some_and(|terminal| {
                terminal.read(cx).cwd() == cwd && terminal.read(cx).host() == host.as_ref()
            })
        {
            self.terminal_open = false;
        }
        focus_chat(&chat, window, cx);
        // Back to a chat whose restart waited while it worked in the background.
        self.run_pending_reload(window, cx);
        cx.notify();
    }

    /// Remembers where `previous` was, as its mode's conversation to come back to.
    fn remember_last_shown(&mut self, previous: &Entity<ChatView>, cx: &App) {
        let session = previous.read(cx).session().read(cx);
        if session.is_idle() {
            self.last_shown.remove(&session.mode());
            return;
        }
        let new = session
            .is_new()
            .then(|| (session.host().cloned(), session.cwd().to_path_buf()));
        let last = LastShown {
            chat: previous.entity_id(),
            new,
        };
        self.last_shown.insert(session.mode(), last);
    }

    /// Keeps the draft of `chat`, which is let go, for its session.
    fn keep_draft(&mut self, chat: &Entity<ChatView>, window: &mut Window, cx: &mut Context<Self>) {
        if chat.read(cx).session().read(cx).is_idle() {
            return;
        }
        let key = DraftKey::of(chat.read(cx).session().read(cx));
        self.drafts.put(key, take_draft(chat, window, cx));
    }

    /// Gives `chat` the draft kept for its session, unless something is typed there already.
    fn restore_draft(
        &mut self,
        chat: &Entity<ChatView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (session, composer) = (chat.read(cx).session(), chat.read(cx).composer());
        if session.read(cx).is_idle() || composer.read(cx).has_draft(cx) {
            return;
        }
        let key = DraftKey::of(session.read(cx));
        if let Some(draft) = self.drafts.take(&key) {
            set_draft(chat, draft, window, cx);
        }
    }

    /// The visible chat's draft, if that is the new session `moving`.
    fn take_moving_draft(
        &self,
        moving: Option<EntityId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Draft> {
        let moves =
            moving == Some(self.chat.entity_id()) && self.chat.read(cx).session().read(cx).is_new();
        moves.then(|| take_draft(&self.chat, window, cx))
    }

    /// Lets background chats of the session `chat` shows go: it was opened anew because its
    /// pi had stopped there.
    fn let_replaced_go(
        &mut self,
        chat: &Entity<ChatView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = session_key(chat, cx) else {
            return;
        };
        let (replaced, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.background)
            .into_iter()
            .partition(|(background, _)| session_key(background, cx).as_ref() == Some(&key));
        self.background = kept;
        for (replaced, _) in &replaced {
            self.keep_draft(replaced, window, cx);
        }
    }

    /// Where the visible conversation's folder lives.
    fn location(&self, cx: &mut Context<Self>) -> workspace::Location {
        match self.host(cx) {
            Some(host) => crate::hosts::hosts(cx).update(cx, |hosts, _| hosts.location(&host)),
            None => workspace::Location::Local,
        }
    }

    /// Drops background chats with nothing left to do (but not what was typed in them).
    fn prune_background(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (working, done): (Vec<_>, Vec<_>) = std::mem::take(&mut self.background)
            .into_iter()
            .partition(|(chat, _)| keeps_working(chat, cx));
        self.background = working;
        for (chat, _) in &done {
            self.keep_draft(chat, window, cx);
        }
        // A chat that was let go starts a fresh pi when opened again: nothing to restart.
        let visible = self.chat.entity_id();
        let background = &self.background;
        self.models_reload.retain(|pending| {
            pending.chat == visible
                || background
                    .iter()
                    .any(|(chat, _)| chat.entity_id() == pending.chat)
        });
        self.update_activity(cx);
    }

    /// Restarts the visible chat's pi if a restart waits for it and it has settled.
    /// Returns whether it did.
    fn run_pending_reload(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let visible = self.chat.entity_id();
        let Some(index) = self
            .models_reload
            .iter()
            .position(|pending| pending.chat == visible)
        else {
            return false;
        };
        if self.chat.read(cx).session().read(cx).is_busy() {
            return false;
        }
        let pending = self.models_reload.remove(index);
        self.reload_models(pending.select, ReloadReason::Models, window, cx);
        true
    }

    /// A chat working in the background changed: it finished, or it asks something.
    fn on_background_event(
        &mut self,
        chat: &Entity<ChatView>,
        event: &ChatEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = chat
            .read(cx)
            .session()
            .read(cx)
            .display_title()
            .unwrap_or_else(|| "A chat".to_owned());
        match event {
            ChatEvent::Stopped => {
                let (notify, attention) = notify_settings(cx);
                if notify {
                    window
                        .push_notification(Notification::error(format!("{title}: pi stopped")), cx);
                }
                if attention && !window.is_window_active() {
                    window.request_attention();
                }
                self.update_activity(cx);
            }
            ChatEvent::BusyChanged | ChatEvent::SessionChanged => {
                let stopped = matches!(
                    chat.read(cx).session().read(cx).lifecycle(),
                    crate::chat::session::Lifecycle::Exited { .. }
                );
                // A pi that stopped is reported by `Stopped`, not as finished work.
                if stopped || keeps_working(chat, cx) {
                    self.update_activity(cx);
                    return;
                }
                if let Some(key) = session_key(chat, cx) {
                    self.finished.insert(key);
                }
                let (notify, attention) = notify_settings(cx);
                if notify {
                    window.push_notification(Notification::success(format!("{title}: done")), cx);
                }
                if attention && !window.is_window_active() {
                    window.request_attention();
                }
                self.prune_background(window, cx);
                self.sidebar.update(cx, |sidebar, cx| sidebar.refresh(cx));
            }
            ChatEvent::NeedsAttention => {
                let (notify, attention) = notify_settings(cx);
                if notify {
                    window.push_notification(
                        Notification::warning(format!("{title} needs your input")),
                        cx,
                    );
                }
                if attention && !window.is_window_active() {
                    window.request_attention();
                }
                self.update_activity(cx);
            }
            _ => {}
        }
    }

    /// Tells the sidebar which sessions are working, waiting for an answer, or finished
    /// unseen.
    fn update_activity(&mut self, cx: &mut Context<Self>) {
        let mut activity = HashMap::new();
        let chats = std::iter::once(&self.chat)
            .chain(self.side_chat())
            .chain(self.background.iter().map(|(chat, _)| chat));
        for chat in chats {
            let Some(key) = session_key(chat, cx) else {
                continue;
            };
            let state = if chat.read(cx).needs_attention() {
                Activity::NeedsYou
            } else if chat.read(cx).session().read(cx).is_busy() {
                Activity::Working
            } else {
                continue;
            };
            activity.insert(key, state);
        }
        for key in &self.finished {
            activity.entry(key.clone()).or_insert(Activity::Done);
        }
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_activity(activity, cx));
    }

    pub(crate) fn new_chat_in(
        &mut self,
        cwd: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_new(None, cwd, None, window, cx);
    }

    /// Shows a new session in `folder` (on `host`, or this computer): with `draft`, else with
    /// the one left behind in a new session there.
    fn show_new(
        &mut self,
        host: Option<HostId>,
        folder: PathBuf,
        draft: Option<Draft>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A new session whose pi stopped in the background has no row in the sidebar; what it
        // could not send is in its composer.
        let key = DraftKey::New {
            host: host.clone(),
            folder: folder.clone(),
        };
        let stopped = self
            .background
            .iter()
            .find(|(chat, _)| DraftKey::of(chat.read(cx).session().read(cx)) == key)
            .map(|(chat, _)| chat.clone());
        let chat =
            stopped.unwrap_or_else(|| cx.new(|cx| ChatView::new(folder, None, host, window, cx)));
        if let Some(draft) = draft {
            set_draft(&chat, draft, window, cx);
        }
        self.show_chat(chat, window, cx);
    }

    /// Opens a session (on `host`, or this computer), reusing the visible or a background
    /// chat that already has it.
    fn open_session(
        &mut self,
        host: Option<HostId>,
        summary: &SessionSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ChatView> {
        let target = Some(summary.path.clone());
        let same = |chat: &Entity<ChatView>, cx: &App| {
            let session = chat.read(cx).session().read(cx);
            // One whose connection ended is started again (Reconnect).
            let ended = session.host().is_some()
                && matches!(
                    session.lifecycle(),
                    crate::chat::session::Lifecycle::Exited { .. }
                );
            session.session_file() == target && session.host() == host.as_ref() && !ended
        };
        if same(&self.chat, cx) {
            return self.chat.clone();
        }
        // The side chat is shown already: go there.
        if let Some(side) = self.side_chat().filter(|side| same(side, cx)).cloned() {
            self.close_pages(cx);
            focus_chat(&side, window, cx);
            return side;
        }
        let existing = self
            .background
            .iter()
            .find(|(chat, _)| same(chat, cx))
            .map(|(chat, _)| chat.clone());
        let chat = existing.unwrap_or_else(|| {
            cx.new(|cx| {
                let mut chat = ChatView::new(
                    summary.cwd.clone(),
                    Some(summary.path.clone()),
                    host.clone(),
                    window,
                    cx,
                );
                // A side chat shows what it took over folded.
                if let Some(prompts) = summary.inherited_prompts {
                    chat.set_inherited_prompts(prompts, cx);
                }
                chat
            })
        });
        self.show_chat(chat.clone(), window, cx);
        chat
    }

    /// Opens a session on an SSH host: right away when the host is ready, else through the
    /// connect dialog (trust prompt, password, pi install), which opens it once connected.
    fn open_remote(&mut self, target: RemoteTarget, window: &mut Window, cx: &mut Context<Self>) {
        let ready = crate::hosts::hosts(cx)
            .read(cx)
            .status(&target.host)
            .is_ready();
        if ready {
            self.open_remote_now(target, window, cx);
            return;
        }
        let host = target.host.clone();
        self.pending_remote = Some((target, false));
        self.open_connect_dialog(host, true, window, cx);
    }

    fn open_remote_now(
        &mut self,
        target: RemoteTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let RemoteTarget { host, folder, file } = target;
        match file {
            Some(file) => {
                let summary = crate::hosts::hosts(cx)
                    .read(cx)
                    .sessions(&host)
                    .iter()
                    .find(|session| session.path == file)
                    .cloned()
                    .unwrap_or_else(|| SessionSummary {
                        path: file,
                        id: String::new(),
                        cwd: folder,
                        name: None,
                        first_prompt: None,
                        side_chat_of: None,
                        inherited_prompts: None,
                        modified: std::time::SystemTime::UNIX_EPOCH,
                    });
                drop(self.open_session(Some(host), &summary, window, cx));
            }
            None => {
                let folder = folder.to_string_lossy().into_owned();
                self.new_remote_chat(host, folder, window, cx);
            }
        }
        let after = std::mem::take(&mut self.after_remote);
        if !after.is_empty() {
            self.open_panels(&after, window, cx);
        }
    }

    /// Opens the session restored at startup once its host connected quietly and listed its
    /// sessions, if the session still exists there (dropped if the host cannot be used without
    /// asking, or the session is gone).
    fn check_pending_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((target, true)) = self.pending_remote.clone() else {
            return;
        };
        let (status, loaded, exists) = {
            let hosts = crate::hosts::hosts(cx);
            let hosts = hosts.read(cx);
            let exists = hosts
                .sessions(&target.host)
                .iter()
                .any(|session| Some(&session.path) == target.file.as_ref());
            (
                hosts.status(&target.host),
                hosts.sessions_loaded(&target.host),
                exists,
            )
        };
        if status.is_ready() && loaded {
            self.pending_remote = None;
            // Only if the user has not started something else meanwhile.
            let chat = self.chat.read(cx);
            if exists
                && chat.session().read(cx).is_empty()
                && !chat.composer().read(cx).has_draft(cx)
            {
                self.open_remote_now(target, window, cx);
            }
        } else if !status.is_ready()
            && !status.is_busy()
            && status != crate::hosts::HostStatus::Idle
        {
            self.pending_remote = None;
        }
    }

    /// Connects to the visible session's host again and resumes the session.
    fn reconnect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (host, folder, file) = {
            let session = self.chat.read(cx).session().read(cx);
            match session.host().cloned() {
                Some(host) => {
                    let file = session.session_file().filter(|_| !session.is_new());
                    (host, session.cwd().to_path_buf(), file)
                }
                None => {
                    // A local pi: start it again on the same session file.
                    self.reload_models(None, ReloadReason::Models, window, cx);
                    return;
                }
            }
        };
        self.pending_remote = Some((
            RemoteTarget {
                host: host.clone(),
                folder,
                file,
            },
            false,
        ));
        self.open_connect_dialog(host, true, window, cx);
    }

    fn on_sidebar_event(
        &mut self,
        _: &Entity<Sidebar>,
        event: &SidebarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // Clicking a conversation shows it, even the one already behind the Skills or
            // Settings page (opening it is a no-op then, so close the page here).
            SidebarEvent::Open(None, summary) => {
                self.close_pages(cx);
                drop(self.open_session(None, summary, window, cx));
            }
            SidebarEvent::Open(Some(host), summary) => {
                self.close_pages(cx);
                let target = RemoteTarget {
                    host: host.clone(),
                    folder: summary.cwd.clone(),
                    file: Some(summary.path.clone()),
                };
                self.open_remote(target, window, cx);
            }
            SidebarEvent::NewChat => self.new_chat(&NewChat, window, cx),
            SidebarEvent::NewInFolder(None, folder) => self.new_chat_in(folder.clone(), window, cx),
            SidebarEvent::NewInFolder(Some(host), folder) => {
                let target = RemoteTarget {
                    host: host.clone(),
                    folder: folder.clone(),
                    file: None,
                };
                self.open_remote(target, window, cx);
            }
            SidebarEvent::ModeChanged(mode) => self.switch_to_mode(*mode, window, cx),
            SidebarEvent::Collapse => self.toggle_sidebar(&ToggleSidebar, window, cx),
            SidebarEvent::Delete(host, summary) => {
                self.confirm_delete(host.clone(), summary.clone(), window, cx);
            }
            SidebarEvent::Rename(host, summary) => {
                self.prompt_rename(host.clone(), summary.clone(), window, cx);
            }
            SidebarEvent::ConnectHost(host) => self.open_connect_flow(host.clone(), window, cx),
            SidebarEvent::ShowSkills => self.toggle_skills(window, cx),
            SidebarEvent::ShowSettings => self.toggle_settings(window, cx),
            SidebarEvent::DeleteFolder(host, folder) => {
                self.confirm_delete_folder(host.clone(), folder.clone(), window, cx);
            }
        }
    }

    fn on_chat_event(
        &mut self,
        _: &Entity<ChatView>,
        event: &ChatEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ChatEvent::SessionChanged | ChatEvent::BusyChanged
                if self.run_pending_reload(window, cx) => {}
            ChatEvent::SessionChanged => {
                let session_file = self.chat.read(cx).session().read(cx).session_file();
                let host = self.host(cx);
                remember(&self.chat, cx);
                self.sidebar.update(cx, |sidebar, cx| {
                    sidebar.set_active(session_file, host.clone(), cx);
                    sidebar.refresh(cx);
                });
                self.update_activity(cx);
                if let Some(host) = host {
                    crate::hosts::hosts(cx)
                        .update(cx, |hosts, cx| hosts.refresh_sessions(&host, cx));
                }
                self.right_panel.update(cx, |panel, cx| panel.refresh(cx));
                cx.notify();
            }
            ChatEvent::Reconnect => self.reconnect(window, cx),
            ChatEvent::BusyChanged | ChatEvent::NeedsAttention | ChatEvent::Stopped => {
                self.update_activity(cx);
            }
            ChatEvent::Composer(event) => match event {
                // A new session moves: what is typed in it comes along.
                ComposerEvent::ChangeFolder(folder) => {
                    let moving = Some(self.chat.entity_id());
                    let draft = self.take_moving_draft(moving, window, cx);
                    self.show_new(None, folder.clone(), draft, window, cx);
                }
                ComposerEvent::ChooseHost(None) => {
                    if self.chat.read(cx).session().read(cx).host().is_some() {
                        // Which folder here is asked, never guessed.
                        let moving = Some(self.chat.entity_id());
                        self.choose_folder(moving, window, cx);
                    }
                }
                ComposerEvent::ChooseHost(Some(host)) | ComposerEvent::BrowseRemote(host) => {
                    self.open_connect_flow(host.clone(), window, cx);
                    self.moving = Some(self.chat.entity_id());
                }
                ComposerEvent::AddHost => {
                    self.open_add_host(window, cx);
                    self.moving = Some(self.chat.entity_id());
                }
                ComposerEvent::ManageHosts => self.open_manage_hosts(window, cx),
                ComposerEvent::Models => {
                    let host = self.models_host(cx);
                    self.open_models_dialog(ModelsOpen::Start, host, window, cx);
                }
            },
        }
    }

    /// "Add SSH connection": on success continues straight to connecting.
    fn open_add_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.moving = None;
        let add = cx.new(AddHost::new);
        self.dialog_subscription =
            Some(
                cx.subscribe_in(&add, window, |this, _, event: &AddHostEvent, window, cx| {
                    let AddHostEvent::Added(host) = event;
                    window.close_dialog(cx);
                    let moving = this.moving;
                    this.open_connect_flow(host.clone(), window, cx);
                    this.moving = moving;
                }),
            );
        open_form_dialog(None, window, cx, move |dialog, _, _| {
            dialog
                .title("Add SSH connection")
                .w(px(480.))
                .child(add.clone())
        });
    }

    /// Connects to `host` (trust prompt, password, pi install) and lets the user pick a
    /// folder there.
    fn open_connect_flow(&mut self, host: HostId, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_remote = None;
        self.open_connect_dialog(host, false, window, cx);
    }

    /// The connect dialog; with `auto_open` it closes itself once the host is ready and
    /// opens [`Self::pending_remote`].
    fn open_connect_dialog(
        &mut self,
        host: HostId,
        auto_open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.moving = None;
        let label = crate::hosts::hosts(cx).read(cx).label(&host);
        let title = if auto_open {
            format!("Connecting to {label}")
        } else {
            format!("Open a folder on {label}")
        };
        let flow = cx.new(|cx| ConnectFlow::new(host, auto_open, window, cx));
        self.dialog_subscription = Some(cx.subscribe_in(
            &flow,
            window,
            |this, _, event: &ConnectFlowEvent, window, cx| match event {
                ConnectFlowEvent::Open { host, folder } => {
                    window.close_dialog(cx);
                    let moving = this.moving.take();
                    let draft = this.take_moving_draft(moving, window, cx);
                    this.show_new_remote(host.clone(), folder.clone(), draft, window, cx);
                }
                ConnectFlowEvent::OpenTerminal { host } => {
                    window.close_dialog(cx);
                    this.open_remote_terminal(host.clone(), PathBuf::from("~"), window, cx);
                }
                ConnectFlowEvent::Ready { host } => {
                    window.close_dialog(cx);
                    if let Some((target, _)) = this.pending_remote.take()
                        && target.host == *host
                    {
                        this.open_remote_now(target, window, cx);
                    }
                }
            },
        ));
        open_form_dialog(None, window, cx, move |dialog, _, _| {
            dialog.title(title.clone()).w(px(520.)).child(flow.clone())
        });
    }

    /// The SSH host whose own models the visible session's pi uses; `None` when they are this
    /// computer's.
    fn models_host(&self, cx: &App) -> Option<HostId> {
        self.host(cx)
            .filter(|host| !crate::hosts::hosts(cx).read(cx).uses_local_models(host))
    }

    /// The Models dialog, for this computer's models or those of `host`. After saving, pi restarts
    /// on the same session to load the file.
    fn open_models_dialog(
        &mut self,
        open: ModelsOpen,
        host: Option<HostId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::chat::models_dialog::{ModelsDialog, ModelsDialogEvent};
        let title = match &host {
            Some(host) => format!("Models on {}", crate::hosts::hosts(cx).read(cx).label(host)),
            None => "Models".to_owned(),
        };
        // The visible session's pi reads this file.
        let visible = !self.welcome && self.models_host(cx) == host;
        let mut available = HashMap::new();
        if visible {
            for model in self.chat.read(cx).session().read(cx).models() {
                *available.entry(model.provider.clone()).or_insert(0) += 1;
            }
        }
        let can_sign_in = visible && self.folder_tools(cx);
        let dialog = cx.new(|cx| ModelsDialog::new(host, open, available, can_sign_in, window, cx));
        self.dialog_subscription = Some(cx.subscribe_in(
            &dialog,
            window,
            move |this, _, event: &ModelsDialogEvent, window, cx| {
                window.close_dialog(cx);
                match event {
                    ModelsDialogEvent::Saved { select, note } => {
                        if let Some(note) = note {
                            window.push_notification(note.clone(), cx);
                        }
                        if let Some((settings, _)) = &this.settings {
                            settings.update(cx, SettingsView::models_changed);
                        }
                        if visible {
                            this.reload_models(select.clone(), ReloadReason::Models, window, cx);
                        }
                    }
                    ModelsDialogEvent::SignIn(provider) => {
                        this.close_settings(cx);
                        let login = OpenLogin {
                            logout: false,
                            provider: Some(provider.clone()),
                        };
                        this.open_login(&login, window, cx);
                    }
                }
            },
        ));
        open_form_dialog(None, window, cx, move |dialog_view, _, _| {
            dialog_view
                .title(title.clone())
                .w(px(600.))
                .child(dialog.clone())
        });
    }

    /// pi reads models.json only when it starts: restart the visible session's pi (resuming
    /// its session file) and select `select`. A working agent is not interrupted; the
    /// restart waits until it settles.
    fn reload_models(
        &mut self,
        select: Option<(String, String)>,
        reason: ReloadReason,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Anything in flight (a run, a prompt being sent or waiting) must not be cut off.
        if self.chat.read(cx).session().read(cx).is_busy() {
            let chat = self.chat.entity_id();
            self.models_reload.retain(|pending| pending.chat != chat);
            self.models_reload.push(PendingReload { chat, select });
            let message = match reason {
                ReloadReason::Models => {
                    "Saved models.json. pi reloads it when the current turn finishes."
                }
                ReloadReason::Skills => {
                    "Saved. pi reloads its skills when the current turn finishes."
                }
                ReloadReason::Asked => "pi restarts when the current turn finishes.",
            };
            window.push_notification(message, cx);
            return;
        }
        let chat_id = self.chat.entity_id();
        self.models_reload.retain(|pending| pending.chat != chat_id);
        // Starting pi would connect again to a host that was turned off; Reconnect starts it
        // anew anyway.
        let off = self
            .host(cx)
            .is_some_and(|host| crate::hosts::hosts(cx).read(cx).is_turned_off(&host));
        if off {
            return;
        }
        let (cwd, file, host) = {
            let session = self.chat.read(cx).session().read(cx);
            (
                session.cwd().to_path_buf(),
                // pi has not written a new session's file: there is nothing to resume.
                session.session_file().filter(|_| !session.is_new()),
                session.host().cloned(),
            )
        };
        let chat = cx.new(|cx| ChatView::new(cwd, file, host, window, cx));
        let draft = take_draft(&self.chat, window, cx);
        set_draft(&chat, draft, window, cx);
        if let Some((provider, id)) = select {
            let session = chat.read(cx).session().clone();
            session.update(cx, |session, cx| {
                session.select_model_when_ready(provider, id, cx);
            });
        }
        // The old pi is idle; let it go rather than keep it in the background.
        let previous = self.chat.clone();
        self.keep_skills = self.skills_open;
        self.show_chat(chat, window, cx);
        self.keep_skills = false;
        self.background
            .retain(|(chat, _)| chat.entity_id() != previous.entity_id());
    }

    /// `/reload`: restarts the visible session's pi, which reads sign-ins, extensions, skills
    /// and settings when it starts.
    fn reload_session(&mut self, _: &ReloadSession, window: &mut Window, cx: &mut Context<Self>) {
        let off = self
            .host(cx)
            .is_some_and(|host| crate::hosts::hosts(cx).read(cx).is_turned_off(&host));
        if off {
            window.push_notification("This session's SSH connection is turned off.", cx);
            return;
        }
        let busy = self.chat.read(cx).session().read(cx).is_busy();
        self.reload_models(None, ReloadReason::Asked, window, cx);
        if !busy {
            window.push_notification("Restarted pi.", cx);
        }
    }

    /// Shows (or hides) the Skills page for the visible Code session.
    fn toggle_skills(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.skills_open {
            self.close_skills(cx);
            focus_chat(&self.chat, window, cx);
            cx.notify();
            return;
        }
        if self.mode(cx) != Mode::Code {
            return;
        }
        self.close_settings(cx);
        let session = self.chat.read(cx).session().clone();
        let reuse = self.skills.as_ref().is_some_and(|(skills, _)| {
            let current = skills.read(cx).session_entity();
            let (a, b) = (current.read(cx), session.read(cx));
            a.cwd() == b.cwd() && a.host() == b.host()
        });
        if reuse {
            if let Some((skills, _)) = &self.skills {
                skills.update(cx, |skills, cx| skills.set_session(session, cx));
            }
        } else {
            let skills = cx.new(|cx| crate::skills::SkillsView::new(session, window, cx));
            let subscription = cx.subscribe_in(
                &skills,
                window,
                |this, _, event: &crate::skills::SkillsEvent, window, cx| match event {
                    // pi reads skills (and project trust) only when it starts.
                    crate::skills::SkillsEvent::Changed => {
                        this.reload_models(None, ReloadReason::Skills, window, cx);
                    }
                },
            );
            self.skills = Some((skills, subscription));
        }
        self.skills_open = true;
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_skills_active(true, cx));
        cx.notify();
    }

    fn close_skills(&mut self, cx: &mut Context<Self>) {
        self.skills_open = false;
        // The page holds its chat's pi session; keeping it would keep that pi running.
        self.skills = None;
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_skills_active(false, cx));
    }

    /// Opens the last session of the start mode on this computer ("Reopen the last session";
    /// one on an SSH host is restored once the host connects).
    fn reopen_last_local_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode = self.sidebar.read(cx).mode();
        let Some(last) = cx
            .default_global::<UiState>()
            .last_sessions
            .get(&mode)
            .filter(|last| last.host.is_none())
            .cloned()
        else {
            return;
        };
        let Some(dir) = cx.global::<AppConfig>().sessions_dir.clone() else {
            return;
        };
        if let Some(summary) = pi_sessions::scan(&dir)
            .into_iter()
            .find(|summary| summary.path == last.file)
        {
            drop(self.open_session(None, &summary, window, cx));
        }
    }

    /// Whether the chat column shows a page (Skills or Settings) instead of the chat.
    fn page_open(&self) -> bool {
        self.skills_open || self.settings.is_some()
    }

    /// Back from the Skills or Settings page to the chat behind it.
    fn close_pages(&mut self, cx: &mut Context<Self>) {
        if self.page_open() {
            self.close_skills(cx);
            self.close_settings(cx);
            cx.notify();
        }
    }

    /// Shows (or hides) the Settings page.
    fn toggle_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            self.close_settings(cx);
            focus_chat(&self.chat, window, cx);
            cx.notify();
            return;
        }
        if self.skills_open {
            self.close_skills(cx);
        }
        // The page is about this computer's models.
        let models = if self.models_host(cx).is_none() {
            self.chat
                .read(cx)
                .session()
                .read(cx)
                .models()
                .iter()
                .map(|model| (model.provider.clone(), model.id.clone(), model.name.clone()))
                .collect()
        } else {
            Vec::new()
        };
        let view = cx.new(|cx| SettingsView::new(models, window, cx));
        let subscription = cx.subscribe_in(&view, window, Self::on_settings_event);
        self.settings = Some((view, subscription));
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_settings_active(true, cx));
        cx.notify();
    }

    fn close_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings.take().is_some() {
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.set_settings_active(false, cx));
        }
    }

    fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_none() {
            self.toggle_settings(window, cx);
        }
    }

    /// Settings → Sandboxing & Security (from a session's sandbox badge).
    fn open_security_settings(
        &mut self,
        _: &crate::actions::OpenSecuritySettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.settings.is_none() {
            self.toggle_settings(window, cx);
            if let Some(settings) = self.settings.as_ref().map(|s| s.0.clone()) {
                settings.update(cx, |settings, _| settings.start_on_security());
            }
        }
    }

    fn on_settings_event(
        &mut self,
        _: &Entity<SettingsView>,
        event: &SettingsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SettingsEvent::Models(open) => self.open_models_dialog(open.clone(), None, window, cx),
            SettingsEvent::ModelsRemoved(note) => {
                window.push_notification(note.clone(), cx);
                // The visible session's pi reads this computer's models.json.
                if !self.welcome && self.models_host(cx).is_none() {
                    self.reload_models(None, ReloadReason::Models, window, cx);
                }
            }
            SettingsEvent::Host(action) => self.on_saved_host_action(action, window, cx),
            SettingsEvent::CheckForUpdates => self.check_for_updates(true, window, cx),
        }
    }

    fn on_saved_host_action(
        &mut self,
        action: &SavedHostAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use crate::hosts::HostStatus;
        match action {
            SavedHostAction::Add => self.open_add_host(window, cx),
            SavedHostAction::Connect(host) => {
                let hosts = crate::hosts::hosts(cx);
                let asks = matches!(
                    hosts.read(cx).status(host),
                    HostStatus::NeedsTrust { .. } | HostStatus::KeyChanged
                );
                if asks {
                    // The dialog shows the host key to confirm, then closes itself.
                    self.pending_remote = None;
                    self.open_connect_dialog(host.clone(), true, window, cx);
                } else {
                    hosts.update(cx, |hosts, cx| hosts.connect(host, false, cx));
                }
            }
            SavedHostAction::Disconnect(host) => self.disconnect_host(host, window, cx),
            SavedHostAction::Rename(host) => {
                crate::remote_ui::open_rename_host(host, window, cx);
            }
            SavedHostAction::Change(host) => self.open_edit_host(host, window, cx),
            SavedHostAction::Remove(host) => {
                crate::remote_ui::confirm_remove_host(host, window, cx);
            }
        }
    }

    /// "Change…" of a saved connection: the form its settings are edited in.
    fn open_edit_host(&mut self, host: &HostId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = EditHost::new(host, window, cx) else {
            return;
        };
        let label = crate::hosts::hosts(cx).read(cx).label(host);
        self.dialog_subscription = Some(cx.subscribe_in(
            &edit,
            window,
            |this, _, event: &EditHostEvent, window, cx| {
                window.close_dialog(cx);
                let EditHostEvent::Save(edit) = event;
                let edit = edit.clone();
                if edit.reconnects(cx) {
                    let host = edit.host.clone();
                    this.with_connection_closed(
                        &host,
                        "Reconnect",
                        move |cx| edit.apply(cx),
                        window,
                        cx,
                    );
                } else {
                    edit.apply(cx);
                }
            },
        ));
        open_form_dialog(None, window, cx, move |dialog, _, _| {
            dialog
                .title(format!("Change {label}"))
                .w(px(480.))
                .child(edit.clone())
        });
    }

    /// Turns `host`'s connection off, asking first when pi is working there.
    fn disconnect_host(&mut self, host: &HostId, window: &mut Window, cx: &mut Context<Self>) {
        let id = host.clone();
        self.with_connection_closed(
            host,
            "Disconnect",
            move |cx| {
                crate::hosts::hosts(cx).update(cx, |hosts, cx| hosts.disconnect(&id, cx));
            },
            window,
            cx,
        );
    }

    /// Does `then`, which closes `host`'s connection, after asking ("Disconnect" or
    /// "Reconnect") when pi is working there.
    fn with_connection_closed(
        &mut self,
        host: &HostId,
        verb: &'static str,
        then: impl Fn(&mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let working = std::iter::once(&self.chat)
            .chain(self.background.iter().map(|(chat, _)| chat))
            .filter(|chat| {
                chat.read(cx).session().read(cx).host() == Some(host) && keeps_working(chat, cx)
            })
            .count();
        if working == 0 {
            then(cx);
            return;
        }
        let label = crate::hosts::hosts(cx).read(cx).label(host);
        let title = if verb == "Disconnect" {
            format!("Disconnect from {label}?")
        } else {
            format!("Reconnect to {label}?")
        };
        let description = if working == 1 {
            "pi is working in a session there and stops. You can resume the session later."
                .to_owned()
        } else {
            format!(
                "pi is working in {working} sessions there and stops. You can resume them later."
            )
        };
        let then = std::rc::Rc::new(then);
        window.open_alert_dialog(cx, move |alert, _, _| {
            let then = then.clone();
            alert
                .title(title.clone())
                .description(description.clone())
                .confirm()
                .ok_text(verb)
                .ok_variant(component::button::ButtonVariant::Danger)
                .on_ok(move |_, _, cx| {
                    then(cx);
                    true
                })
        });
    }

    /// Looks for a newer release in the background. `manual` (the menu or Settings) also says
    /// when there is none or the check failed; at start a skipped version is not offered.
    fn check_for_updates(&mut self, manual: bool, window: &mut Window, cx: &mut Context<Self>) {
        if manual {
            Self::show_update_check(window, cx);
        }
        let task = cx
            .background_executor()
            .spawn(async { crate::updates::fetch_latest() });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.update_check_done(manual, result, window, cx);
            });
        })
        .detach();
    }

    /// Says that updates are looked for, until [`Self::update_check_done`].
    fn show_update_check(window: &mut Window, cx: &mut App) {
        let checking = Notification::new()
            .message("Checking for updates…")
            .id::<UpdateCheck>()
            .autohide(false);
        window.push_notification(checking, cx);
    }

    fn update_check_done(
        &mut self,
        manual: bool,
        result: Result<Option<crate::updates::Release>, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = crate::updates::compared_version();
        match result {
            Ok(Some(release)) if crate::updates::is_newer(&release.version, &current) => {
                if manual {
                    window.remove_notification::<UpdateCheck>(cx);
                }
                let skipped = cx.global::<AppSettings>().skipped_version.as_deref()
                    == Some(release.version.as_str());
                // At start, never over the card that says what the last update changed.
                if manual || (!skipped && self.update_prompt.is_none()) {
                    self.offer_update(release, window, cx);
                }
            }
            Ok(_) if manual => window.push_notification(
                Notification::success(format!(
                    "You have the latest version (Pistachio {current})."
                ))
                .id::<UpdateCheck>(),
                cx,
            ),
            Err(error) if manual => window.push_notification(
                Notification::error(format!("Could not check for updates: {error}"))
                    .id::<UpdateCheck>(),
                cx,
            ),
            Ok(_) => {}
            Err(error) => log::info!("update check failed: {error}"),
        }
    }

    /// "Manage SSH connections".
    fn open_manage_hosts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let manage = cx.new(|cx| ManageHosts::new(window, cx));
        self.dialog_subscription = Some(cx.subscribe_in(
            &manage,
            window,
            |this, _, event: &ManageHostsEvent, window, cx| match event {
                ManageHostsEvent::Add => {
                    window.close_dialog(cx);
                    this.open_add_host(window, cx);
                }
            },
        ));
        open_form_dialog(None, window, cx, move |dialog, _, _| {
            dialog
                .title("SSH connections")
                .w(px(560.))
                .child(manage.clone())
        });
    }

    fn new_remote_chat(
        &mut self,
        host: HostId,
        folder: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_new_remote(host, folder, None, window, cx);
    }

    fn show_new_remote(
        &mut self,
        host: HostId,
        folder: String,
        draft: Option<Draft>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::hosts::hosts(cx).update(cx, |hosts, cx| hosts.touch_folder(&host, &folder, cx));
        self.show_new(Some(host), PathBuf::from(folder), draft, window, cx);
    }

    pub(crate) fn confirm_delete(
        &mut self,
        host: Option<HostId>,
        summary: SessionSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.weak_entity();
        let title = summary.title().to_owned();
        let description = match &host {
            Some(host) => format!(
                "The session file is moved to the trash on {} and can be restored from there.",
                crate::hosts::hosts(cx).read(cx).label(host)
            ),
            None => {
                "The session file is moved to the trash and can be restored from there.".to_owned()
            }
        };
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (weak, summary, host) = (weak.clone(), summary.clone(), host.clone());
            alert
                .title(format!("Move “{title}” to the trash?"))
                .description(description.clone())
                .confirm()
                .ok_text("Move to trash")
                .ok_variant(component::button::ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let host = host.clone();
                    let _ = weak.update(cx, |this, cx| {
                        this.delete_session(host, &summary, window, cx);
                    });
                    true
                })
        });
    }

    /// Asks before moving every session of `folder` (on `host`) to the trash. The folder
    /// itself and its files are not touched.
    fn confirm_delete_folder(
        &mut self,
        host: Option<HostId>,
        folder: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sessions: Vec<SessionSummary> = match &host {
            None => self.sidebar.read(cx).sessions_in(&folder),
            Some(host) => crate::hosts::hosts(cx)
                .read(cx)
                .sessions(host)
                .iter()
                .filter(|session| session.cwd == folder)
                .cloned()
                .collect(),
        };
        if sessions.is_empty() {
            return;
        }
        let count = sessions.len();
        let name = folder.file_name().map_or_else(
            || folder.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let place = match &host {
            Some(host) => format!(" on {}", crate::hosts::hosts(cx).read(cx).label(host)),
            None => String::new(),
        };
        let sessions_text = if count == 1 {
            "its session".to_owned()
        } else {
            format!("its {count} sessions")
        };
        let description = format!(
            "This moves {sessions_text} to the trash{place}, so the folder leaves the sidebar. \
             The folder {} and its files are not touched.",
            folder.display()
        );
        let ok_text = if count == 1 {
            "Move 1 session to trash".to_owned()
        } else {
            format!("Move {count} sessions to trash")
        };
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (weak, host, folder, sessions) =
                (weak.clone(), host.clone(), folder.clone(), sessions.clone());
            alert
                .title(format!("Remove “{name}” from the sidebar?"))
                .description(description.clone())
                .confirm()
                .ok_text(ok_text.clone())
                .ok_variant(component::button::ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    let (host, folder, sessions) = (host.clone(), folder.clone(), sessions.clone());
                    let _ = weak.update(cx, |this, cx| {
                        this.delete_folder_sessions(host, &folder, sessions, window, cx);
                    });
                    true
                })
        });
    }

    fn delete_folder_sessions(
        &mut self,
        host: Option<HostId>,
        folder: &std::path::Path,
        sessions: Vec<SessionSummary>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths: Vec<PathBuf> = sessions.into_iter().map(|session| session.path).collect();
        let in_folder = |chat: &Entity<ChatView>, cx: &App| {
            let session = chat.read(cx).session().read(cx);
            session.host() == host.as_ref()
                && (session.cwd() == folder
                    || session
                        .session_file()
                        .is_some_and(|file| paths.contains(&file)))
        };
        if !self.welcome && in_folder(&self.chat, cx) {
            self.show_welcome(Mode::Code, window, cx);
        }
        self.background.retain(|(chat, _)| !in_folder(chat, cx));
        for path in &paths {
            self.drafts.forget(host.as_ref(), path);
            self.renames.remove(&(host.clone(), path.clone()));
        }
        match host {
            Some(host) => {
                let task = crate::hosts::hosts(cx)
                    .update(cx, |hosts, cx| hosts.trash_sessions(&host, paths, cx));
                cx.spawn_in(window, async move |_, cx| {
                    if let Err(error) = task.await {
                        let _ = cx.update(|window, cx| {
                            window.push_notification(
                                format!("Some sessions could not be moved to the trash: {error}"),
                                cx,
                            );
                        });
                    }
                })
                .detach();
            }
            None => {
                let task = self
                    .sidebar
                    .update(cx, |sidebar, cx| sidebar.trash_sessions(paths, cx));
                let folder = folder.to_path_buf();
                cx.spawn_in(window, async move |_, cx| {
                    if let Err(error) = task.await {
                        log::warn!("could not trash sessions of {}: {error}", folder.display());
                        let _ = cx.update(|window, cx| {
                            window.push_notification(
                                format!("Some sessions could not be moved to the trash: {error}"),
                                cx,
                            );
                        });
                    }
                })
                .detach();
            }
        }
    }

    pub(crate) fn delete_session(
        &mut self,
        host: Option<HostId>,
        summary: &SessionSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let matches = |chat: &Entity<ChatView>, cx: &App| {
            let session = chat.read(cx).session().read(cx);
            session.session_file().as_deref() == Some(summary.path.as_path())
                && session.host() == host.as_ref()
        };
        if matches(&self.chat, cx) {
            match &host {
                Some(host) => {
                    let folder = summary.cwd.to_string_lossy().into_owned();
                    self.new_remote_chat(host.clone(), folder, window, cx);
                }
                None => self.new_chat_in(summary.cwd.clone(), window, cx),
            }
        }
        if self.side_chat().is_some_and(|side| matches(side, cx)) {
            self.set_side(None, window, cx);
        }
        self.background.retain(|(chat, _)| !matches(chat, cx));
        self.drafts.forget(host.as_ref(), &summary.path);
        self.renames.remove(&(host.clone(), summary.path.clone()));
        if let Some(host) = host {
            let task = crate::hosts::hosts(cx).update(cx, |hosts, cx| {
                hosts.trash_session(&host, summary.path.clone(), cx)
            });
            cx.spawn_in(window, async move |_, cx| {
                if let Err(error) = task.await {
                    let _ = cx.update(|window, cx| {
                        window.push_notification(
                            format!("Could not move the session to the trash: {error}"),
                            cx,
                        );
                    });
                }
            })
            .detach();
            return;
        }
        crate::chat::recall::Recall::update(cx, |recall| recall.forget(&summary.path));
        let path = summary.path.clone();
        let task = self.sidebar.update(cx, |sidebar, cx| {
            sidebar.trash_sessions(vec![path.clone()], cx)
        });
        cx.spawn_in(window, async move |_, cx| {
            if let Err(error) = task.await {
                log::warn!("could not trash {}: {error}", path.display());
                let _ = cx.update(|window, cx| {
                    window.push_notification(
                        format!("Could not move the session to the trash: {error}"),
                        cx,
                    );
                });
            }
        })
        .detach();
    }

    fn prompt_rename(
        &mut self,
        host: Option<HostId>,
        summary: SessionSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).default_value(summary.title().to_owned());
            input.select_all(window, cx);
            input
        });
        let weak = cx.weak_entity();
        let submit = on_submit(&input, window, cx, move |name, window, cx| {
            window.close_dialog(cx);
            let name = name.trim().to_owned();
            if !name.is_empty() {
                let (host, summary) = (host.clone(), summary.clone());
                let _ = weak.update(cx, |this, cx| {
                    this.rename_session(host, &summary, name, window, cx);
                });
            }
        });
        let focus = input.focus_handle(cx);
        open_form_dialog(Some(focus), window, cx, move |dialog, _, _| {
            let submit = submit.clone();
            dialog
                .title("Rename chat")
                .w(px(420.))
                .child(Input::new(&input))
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("rename-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("rename-ok")
                                .primary()
                                .label("Rename")
                                .on_click(move |_, window, cx| submit(window, cx)),
                        ),
                )
        });
    }

    /// The pi of an open (visible or background) chat showing this session, unless it ended.
    fn open_pi(&self, key: &(Option<HostId>, PathBuf), cx: &App) -> Option<Entity<PiSession>> {
        use crate::chat::session::Lifecycle;
        std::iter::once(&self.chat)
            .chain(self.side_chat())
            .chain(self.background.iter().map(|(chat, _)| chat))
            .find(|chat| {
                session_key(chat, cx).as_ref() == Some(key)
                    && matches!(
                        chat.read(cx).session().read(cx).lifecycle(),
                        Lifecycle::Starting | Lifecycle::Ready
                    )
            })
            .map(|chat| chat.read(cx).session().clone())
    }

    /// Renames a session without showing it. Renames go through pi so the session file stays
    /// pi's (on the host, for remote sessions): the pi of its open chat, else one started just
    /// for this.
    fn rename_session(
        &mut self,
        host: Option<HostId>,
        summary: &SessionSummary,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = (host, summary.path.clone());
        let session = match (self.open_pi(&key, cx), &key.0) {
            (Some(session), _) => session,
            (None, Some(host)) if !crate::hosts::hosts(cx).read(cx).status(host).is_ready() => {
                let label = crate::hosts::hosts(cx).read(cx).label(host);
                window.push_notification(
                    Notification::info(format!("Connect to {label} to rename its sessions.")),
                    cx,
                );
                return;
            }
            (None, host) => {
                let (cwd, file) = (summary.cwd.clone(), summary.path.clone());
                cx.new(|cx| PiSession::new(cwd, Some(file), host.clone(), cx))
            }
        };
        let renamed = session.update(cx, |session, cx| {
            session.rename_when_ready(name.clone(), cx)
        });
        let renamed = renamed.with_timeout(RENAME_TIMEOUT, cx.background_executor());
        let running = key.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let result = renamed
                .await
                .unwrap_or_else(|_| Err("pi did not answer.".to_owned()));
            let _ = this.update_in(cx, |this, window, cx| {
                let open = this.open_pi(&key, cx);
                match (result, open) {
                    (Ok(()), Some(open)) if open != session => {
                        // Opened meanwhile: its pi may have read the file before.
                        open.update(cx, |open, cx| open.rename(name, cx));
                    }
                    (Ok(()), _) => {}
                    (Err(error), _) => window.push_notification(
                        Notification::error(format!("Could not rename the session: {error}")),
                        cx,
                    ),
                }
                if let Some(host) = &key.0 {
                    crate::hosts::hosts(cx)
                        .update(cx, |hosts, cx| hosts.refresh_sessions(host, cx));
                }
                // Lets the pi started for this go.
                drop(session);
                this.renames.remove(&key);
            });
        });
        self.renames.insert(running, task);
    }

    // ---- actions -------------------------------------------------------------

    fn new_chat(&mut self, _: &NewChat, window: &mut Window, cx: &mut Context<Self>) {
        // An untouched conversation of the sidebar's mode is already "new": just focus it.
        let mode = self.sidebar.read(cx).mode();
        if self.welcome {
            self.new_in_default_folder(mode, window, cx);
            return;
        }
        if self.mode(cx) == mode && self.chat.read(cx).session().read(cx).is_empty() {
            // The Skills or Settings page covers the chat: going back to it is the new
            // conversation (otherwise the click seems to do nothing).
            self.close_pages(cx);
            focus_chat(&self.chat, window, cx);
            return;
        }
        // On an SSH host, a new session stays on that host and folder.
        let remote = {
            let session = self.chat.read(cx).session().read(cx);
            session
                .host()
                .cloned()
                .filter(|_| mode == Mode::Code)
                .map(|host| (host, session.cwd().to_string_lossy().into_owned()))
        };
        if let Some((host, folder)) = remote {
            self.new_remote_chat(host, folder, window, cx);
            return;
        }
        self.new_in_default_folder(mode, window, cx);
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_open = !self.sidebar_open;
        cx.notify();
    }

    fn toggle_right_panel(&mut self, _: &ToggleRightPanel, _: &mut Window, cx: &mut Context<Self>) {
        if !self.folder_tools(cx) {
            return;
        }
        self.set_right_open(!self.right_open, cx);
    }

    /// Opens or closes the right panel; a hidden panel stops watching the folder.
    fn set_right_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.right_open = open;
        self.right_panel
            .update(cx, |panel, cx| panel.set_visible(open, cx));
        cx.notify();
    }

    fn show_tab(&mut self, tab: RightTab, cx: &mut Context<Self>) {
        if !self.folder_tools(cx) {
            return;
        }
        let current = self.right_panel.read(cx).tab();
        if self.right_open && current == tab {
            self.set_right_open(false, cx);
        } else {
            self.right_panel
                .update(cx, |panel, cx| panel.set_tab(tab, cx));
            self.set_right_open(true, cx);
        }
    }

    fn show_diff(&mut self, _: &ShowDiff, _: &mut Window, cx: &mut Context<Self>) {
        self.show_tab(RightTab::Diff, cx);
    }

    fn show_files(&mut self, _: &ShowFiles, _: &mut Window, cx: &mut Context<Self>) {
        self.show_tab(RightTab::Files, cx);
    }

    fn toggle_terminal(&mut self, _: &ToggleTerminal, window: &mut Window, cx: &mut Context<Self>) {
        if !self.folder_tools(cx) {
            return;
        }
        if self.terminal_open {
            self.terminal_open = false;
            self.save_terminal_height(cx);
            focus_chat(&self.chat, window, cx);
            cx.notify();
            return;
        }
        let cwd = self.cwd(cx);
        match self.host(cx) {
            Some(host) => self.open_remote_terminal(host, cwd, window, cx),
            None => {
                let reuse = self.terminal.as_ref().is_some_and(|terminal| {
                    terminal.read(cx).cwd() == cwd && terminal.read(cx).host().is_none()
                });
                if !reuse && !self.unpark_terminal(&cwd, None, cx) {
                    let terminal = cx.new(|cx| TerminalPanel::new(cwd, window, cx));
                    self.set_terminal(terminal, window, cx);
                }
                self.show_terminal(window, cx);
            }
        }
    }

    /// Shows a terminal on `host` in `cwd` there (reusing an open one for the same place).
    fn open_remote_terminal(
        &mut self,
        host: HostId,
        cwd: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let reuse = self.terminal.as_ref().is_some_and(|terminal| {
            terminal.read(cx).cwd() == cwd && terminal.read(cx).host() == Some(&host)
        });
        if !reuse && !self.unpark_terminal(&cwd, Some(&host), cx) {
            let (label, ssh) = {
                let hosts = crate::hosts::hosts(cx);
                let hosts = hosts.read(cx);
                (hosts.label(&host), hosts.ssh(&host))
            };
            let Some(ssh) = ssh else { return };
            let terminal =
                cx.new(|cx| TerminalPanel::remote(host, label, &ssh, cwd, None, window, cx));
            self.set_terminal(terminal, window, cx);
        }
        self.show_terminal(window, cx);
    }

    /// `/login` and `/logout`: runs pi's own sign-in in a new terminal, so nothing is typed into
    /// a program running in the user's (kept in the background until this one closes).
    fn open_login(&mut self, login: &OpenLogin, window: &mut Window, cx: &mut Context<Self>) {
        if !self.folder_tools(cx) {
            return;
        }
        let session = self.chat.read(cx).session().clone();
        // pi on a host that uses this computer's models uses its sign-ins too: they are here.
        let hosts = crate::hosts::hosts(cx);
        let host = self
            .host(cx)
            .filter(|host| !hosts.read(cx).uses_local_models(host));
        let terminal = match host {
            Some(host) => {
                let hosts = hosts.read(cx);
                let off = hosts.is_turned_off(&host);
                let (Some(ssh), Some(pi), false) = (hosts.ssh(&host), hosts.pi_command(&host), off)
                else {
                    window.push_notification("This session's SSH connection is turned off.", cx);
                    return;
                };
                let label = hosts.label(&host);
                let command = login.remote_command(&pi);
                let cwd = self.cwd(cx);
                cx.new(|cx| {
                    TerminalPanel::remote(host, label, &ssh, cwd, Some(&command), window, cx)
                })
            }
            None => {
                let config = cx.global::<AppConfig>();
                let home = config.home.clone().unwrap_or_else(std::env::temp_dir);
                let pi = config.pi_program.to_string_lossy().into_owned();
                let cwd = if self.local_code(cx) {
                    self.cwd(cx)
                } else {
                    home
                };
                let command = std::iter::once(pi).chain(login.pi_args()).collect();
                cx.new(|cx| TerminalPanel::running(cwd, command, window, cx))
            }
        };
        self.set_terminal(terminal, window, cx);
        self.show_terminal(window, cx);
        let text = login.notice();
        session.update(cx, |session, cx| {
            session.push_notice(transcript::NoticeKind::Info, text, cx);
        });
    }

    fn set_terminal(
        &mut self,
        terminal: Entity<TerminalPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._subscriptions.push(cx.subscribe_in(
            &terminal,
            window,
            |this, terminal, event, window, cx| match event {
                TerminalPanelEvent::Closed => {
                    // A parked terminal's shell exited: let it go.
                    this.parked_terminals.retain(|parked| parked != terminal);
                    if this.terminal.as_ref() == Some(terminal) {
                        this.terminal_open = false;
                        this.terminal = None;
                        focus_chat(&this.chat, window, cx);
                        cx.notify();
                    }
                }
            },
        ));
        if let Some(previous) = self.terminal.replace(terminal) {
            self.parked_terminals.push(previous);
        }
    }

    /// Makes the parked terminal for `cwd` on `host` the current one (parking the current
    /// one). Returns whether there was one.
    fn unpark_terminal(&mut self, cwd: &std::path::Path, host: Option<&HostId>, cx: &App) -> bool {
        let Some(index) = self.parked_terminals.iter().position(|terminal| {
            let terminal = terminal.read(cx);
            terminal.cwd() == cwd && terminal.host() == host
        }) else {
            return false;
        };
        let terminal = self.parked_terminals.remove(index);
        if let Some(previous) = self.terminal.replace(terminal) {
            self.parked_terminals.push(previous);
        }
        true
    }

    fn show_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal_open = true;
        if let Some(terminal) = &self.terminal {
            let focus = terminal.read(cx).focus_handle(cx);
            focus.focus(window, cx);
        }
        cx.notify();
    }

    /// `/resume`: pick an earlier session of this chat's mode and folder.
    fn resume_session(&mut self, _: &ResumeSession, window: &mut Window, cx: &mut Context<Self>) {
        let (folder, current, remote) = {
            let session = self.chat.read(cx).session().read(cx);
            (
                session.cwd().to_path_buf(),
                session.session_file(),
                session.host().is_some(),
            )
        };
        if remote {
            window.push_notification(
                Notification::info("Open this host's other sessions from the sidebar."),
                cx,
            );
            return;
        }
        let sessions: Vec<SessionSummary> = self
            .sidebar
            .read(cx)
            .sessions_in(&folder)
            .into_iter()
            .filter(|session| Some(&session.path) != current.as_ref())
            .take(50)
            .collect();
        if sessions.is_empty() {
            window.push_notification(Notification::info("No earlier sessions here."), cx);
            return;
        }
        let now = std::time::SystemTime::now();
        let items = sessions
            .iter()
            .map(|session| crate::chat::dialogs::PickerItem {
                label: session.title().to_owned(),
                detail: Some(crate::ui::relative_time(session.modified, now)),
            })
            .collect();
        let view = cx.weak_entity();
        crate::chat::dialogs::open_picker(
            "Open an earlier session",
            items,
            move |index, window, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.open_session(None, &sessions[index], window, cx);
                });
            },
            window,
            cx,
        );
    }

    /// Dragging the terminal's top edge: the terminal takes what is below the pointer, and
    /// the chat keeps at least [`MIN_CHAT_HEIGHT`].
    fn drag_terminal_edge(
        &mut self,
        event: &DragMoveEvent<TerminalEdge>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let most = (event.bounds.size.height - MIN_CHAT_HEIGHT).max(MIN_TERMINAL_HEIGHT);
        let height =
            (event.bounds.bottom() - event.event.position.y).clamp(MIN_TERMINAL_HEIGHT, most);
        if height != self.terminal_height {
            self.terminal_height = height;
            cx.notify();
        }
    }

    fn save_terminal_height(&mut self, cx: &mut Context<Self>) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let height = f32::from(self.terminal_height).round().clamp(0., 10_000.) as u16;
        UiState::update(cx, |state| state.terminal_height = Some(height));
    }

    /// ⌘F: the find bar of the conversation on screen.
    fn find_in_chat(&mut self, _: &FindInChat, window: &mut Window, cx: &mut Context<Self>) {
        if self.welcome {
            return;
        }
        let chat = self.chat.clone();
        chat.update(cx, |chat, cx| chat.open_find(window, cx));
    }

    fn focus_composer(&mut self, _: &FocusComposer, window: &mut Window, cx: &mut Context<Self>) {
        focus_chat(&self.chat, window, cx);
    }

    // ---- rendering -----------------------------------------------------------

    /// `controls` is the room to keep free at the right end for the window's buttons, when the
    /// chat column is the rightmost one.
    fn render_chat_header(
        &self,
        controls: Pixels,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let session = self.chat.read(cx).session().read(cx);
        let title = if self.settings.is_some() {
            "Settings".to_owned()
        } else if self.skills_open {
            "Skills".to_owned()
        } else if self.welcome {
            String::new()
        } else {
            session.display_title().unwrap_or_else(|| {
                match session.mode() {
                    Mode::Chat => "New chat",
                    Mode::Code => "New session",
                }
                .to_owned()
            })
        };
        let home = cx.global::<AppConfig>().home.clone();
        let folder = crate::ui::short_path(session.cwd(), home.as_deref());
        // Code sessions (here or on an SSH host) have folder tools (terminal, diff, files);
        // chats have no folder, and remote folders are shown as `host:path`.
        let code = session.has_folder_tools() && !self.page_open() && !self.welcome;
        let folder = match session.host() {
            Some(host) => {
                let label = crate::hosts::hosts(cx)
                    .read(cx)
                    .get(host)
                    .map_or_else(|| host.0.clone(), |h| h.label.clone());
                format!("{label}:{}", session.cwd().display())
            }
            None => folder,
        };
        let show_folder = session.mode() == Mode::Code && !self.welcome;
        let tab = self.right_panel.read(cx).tab();
        let theme = cx.theme();

        self.drag.apply(
            h_flex()
                .id("chat-top-bar")
                .h(TOP_BAR_HEIGHT)
                .flex_none()
                .w_full()
                .pr(px(8.) + controls)
                .gap_2()
                .border_b_1()
                .border_color(theme.border)
                .map(|this| {
                    if self.sidebar_open {
                        this.pl_4()
                    } else {
                        this.pl(TRAFFIC_LIGHT_INSET)
                    }
                })
                .when(!self.sidebar_open, |this| {
                    this.child(
                        Button::new("open-sidebar")
                            .quiet(cx)
                            .compact()
                            .child(crate::ui::top_bar_icon(IconName::PanelLeftOpen))
                            .accessibility_label("Show sidebar (⌘B)")
                            .tooltip("Show sidebar (⌘B)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_sidebar(&ToggleSidebar, window, cx);
                            })),
                    )
                    .child(
                        Button::new("header-new-chat")
                            .quiet(cx)
                            .compact()
                            .child(crate::ui::top_bar_icon(component::Icon::from(
                                AppIcon::SquarePen,
                            )))
                            .accessibility_label("New chat (⌘N)")
                            .tooltip("New chat (⌘N)")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_chat(&NewChat, window, cx);
                            })),
                    )
                })
                .child(
                    h_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                // The title keeps its width (up to a cap) while there is room;
                                // in a narrow column it truncates too, never running under the
                                // buttons on the right.
                                .min_w_0()
                                .max_w(px(360.))
                                .truncate()
                                .text_color(theme.foreground)
                                .child(title),
                        )
                        .when(show_folder, |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    // No width of its own (flex basis 0): only the room the
                                    // title leaves, so the folder truncates (and goes) first.
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(theme.muted_foreground)
                                    .child(folder),
                            )
                        }),
                )
                .when(self.settings.is_some(), |this| {
                    this.child(
                        Button::new("close-settings")
                            .quiet(cx)
                            .small()
                            .icon(IconName::ArrowLeft)
                            .label("Back")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_settings(window, cx);
                            })),
                    )
                })
                .when(self.skills_open, |this| {
                    this.child(
                        Button::new("close-skills")
                            .quiet(cx)
                            .small()
                            .icon(IconName::ArrowLeft)
                            .label("Back to the session")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_skills(window, cx);
                            })),
                    )
                })
                .child(
                    h_flex()
                        .flex_none()
                        .gap_0p5()
                        .when(!code, |this| this.hidden())
                        .child(
                            Button::new("toggle-terminal")
                                .quiet(cx)
                                .compact()
                                .child(crate::ui::top_bar_icon(IconName::SquareTerminal))
                                .selected(self.terminal_open)
                                .accessibility_label("Terminal (ctrl+`)")
                                .tooltip("Terminal (ctrl+`)")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_terminal(&ToggleTerminal, window, cx);
                                })),
                        )
                        .child(crate::demo::mark(
                            "show-diff",
                            Button::new("show-diff")
                                .quiet(cx)
                                .compact()
                                .child(crate::ui::top_bar_icon(component::Icon::from(
                                    AppIcon::GitCompare,
                                )))
                                .selected(self.right_open && tab == RightTab::Diff)
                                .accessibility_label("Changes (⌘⇧D)")
                                .tooltip("Changes (⌘⇧D)")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.show_diff(&ShowDiff, window, cx);
                                })),
                        ))
                        .child(
                            Button::new("show-files")
                                .quiet(cx)
                                .compact()
                                .child(crate::ui::top_bar_icon(component::Icon::from(
                                    AppIcon::Files,
                                )))
                                .selected(self.right_open && tab == RightTab::Files)
                                .accessibility_label("Files (⌘⇧E)")
                                .tooltip("Files (⌘⇧E)")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.show_files(&ShowFiles, window, cx);
                                })),
                        )
                        .child(
                            Button::new("toggle-right")
                                .quiet(cx)
                                .compact()
                                .child(crate::ui::top_bar_icon(if self.right_open {
                                    IconName::PanelRightClose
                                } else {
                                    IconName::PanelRightOpen
                                }))
                                .accessibility_label("Toggle side panel (⌘⌥B)")
                                .tooltip("Toggle side panel (⌘⌥B)")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_right_panel(&ToggleRightPanel, window, cx);
                                })),
                        ),
                ),
        )
    }
}

/// Saves `chat`'s session as the one to come back to in its mode (also across restarts).
const DEFAULT_TERMINAL_HEIGHT: f32 = 240.;
const MIN_TERMINAL_HEIGHT: Pixels = px(100.);
const MIN_CHAT_HEIGHT: Pixels = px(160.);
/// The chat column is never squeezed narrower than this by the side panels: room for the
/// header's buttons and the message box's model picker, thinking level and send button.
const MIN_CHAT_WIDTH: Pixels = px(400.);

/// What is dragged while resizing the terminal (no visible drag image).
#[derive(Clone)]
struct TerminalEdge;

impl Render for TerminalEdge {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// The terminal's top edge: a line that highlights on hover and drags to resize.
fn render_terminal_edge(cx: &mut Context<MainWindow>) -> impl IntoElement + use<> {
    let theme = cx.theme();
    let (line, hover) = (theme.border, crate::theme::palette(cx).accent);
    div()
        .id("terminal-edge")
        .flex_none()
        .w_full()
        .h(px(7.))
        .flex()
        .items_center()
        .cursor(CursorStyle::ResizeUpDown)
        .group("terminal-edge")
        .on_drag(TerminalEdge, |edge, _, _, cx| {
            cx.stop_propagation();
            cx.new(|_| edge.clone())
        })
        .child(
            div()
                .w_full()
                .h(px(1.))
                .bg(line)
                .group_hover("terminal-edge", move |style| style.h(px(3.)).bg(hover)),
        )
}

/// A chat with work in flight or a question waiting: switching away must not drop it.
/// Whether background chats show toasts, and ask for attention (Dock bounce) — Settings.
fn notify_settings(cx: &App) -> (bool, bool) {
    let settings = cx.try_global::<AppSettings>().cloned().unwrap_or_default();
    (
        settings.background_notifications,
        settings.request_attention,
    )
}

fn keeps_working(chat: &Entity<ChatView>, cx: &App) -> bool {
    let chat = chat.read(cx);
    chat.needs_attention() || chat.session().read(cx).is_busy()
}

/// The session a chat shows, as the sidebar identifies it: its host and file.
fn session_key(chat: &Entity<ChatView>, cx: &App) -> Option<(Option<HostId>, PathBuf)> {
    let session = chat.read(cx).session().read(cx);
    Some((session.host().cloned(), session.session_file()?))
}

/// Empties `chat`'s composer (see [`crate::chat::composer::Composer::take_draft`]).
fn take_draft(chat: &Entity<ChatView>, window: &mut Window, cx: &mut App) -> Draft {
    let composer = chat.read(cx).composer().clone();
    composer.update(cx, |composer, cx| composer.take_draft(window, cx))
}

fn set_draft(chat: &Entity<ChatView>, draft: Draft, window: &mut Window, cx: &mut App) {
    let composer = chat.read(cx).composer().clone();
    composer.update(cx, |composer, cx| composer.set_draft(draft, window, cx));
}

fn remember(chat: &Entity<ChatView>, cx: &mut App) {
    let (mode, last) = {
        let session = chat.read(cx).session().read(cx);
        let Some(file) = session.session_file() else {
            return;
        };
        (
            session.mode(),
            LastSession {
                file,
                host: session.host().map(|host| host.0.clone()),
                folder: session.host().map(|_| session.cwd().to_path_buf()),
            },
        )
    };
    UiState::update(cx, |state| {
        state.last_sessions.insert(mode, last);
    });
}

/// Saves (or finds) the host for an ssh config alias.
fn add_alias_host(alias: String, cx: &mut App) -> HostId {
    let target = remote::Target::Alias {
        alias: alias.clone(),
    };
    crate::hosts::hosts(cx).update(cx, |hosts, cx| hosts.add(alias, target, cx))
}

impl MainWindow {
    /// The start screen: logo and a button that starts a session (Code: in a recent folder,
    /// a chosen folder or on an SSH host; Chat: a new chat).
    fn render_welcome(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mode = self.sidebar.read(cx).mode();
        let theme = cx.theme();
        let (muted, foreground) = (theme.muted_foreground, theme.foreground);
        let accent = crate::theme::palette(cx).accent;
        let (headline, detail, label) = match mode {
            Mode::Code => (
                "Start a coding session",
                "Open a folder on this computer or an SSH host, or pick up a session from the \
                 sidebar.",
                "New session",
            ),
            Mode::Chat => (
                "Start a chat",
                "A plain conversation, without access to your files.",
                "New chat",
            ),
        };
        let weak = cx.weak_entity();
        let button = Button::new("welcome-new")
            .quiet_outline(AppIcon::SquarePen, cx)
            .label(label);
        let button = match mode {
            Mode::Chat => button
                .child(
                    div()
                        .text_sm()
                        .text_color(muted)
                        .child(crate::actions::NEW_CHAT_KEYS),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.new_in_default_folder(Mode::Chat, window, cx);
                }))
                .into_any_element(),
            Mode::Code => button
                .child(
                    component::Icon::new(IconName::ChevronDown)
                        .small()
                        .text_color(muted),
                )
                .dropdown_menu_with_anchor(Anchor::TopCenter, move |menu, _, cx| {
                    welcome_menu(menu, weak.clone(), cx)
                })
                .into_any_element(),
        };
        let content = v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .px_6()
            .child(
                component::Icon::from(AppIcon::PiLogo)
                    .size_12()
                    .text_color(accent),
            )
            .child(div().text_xl().text_color(foreground).child(headline))
            .child(
                div()
                    .max_w(px(420.))
                    .text_sm()
                    .text_center()
                    .text_color(muted)
                    .child(detail),
            )
            .child(div().pt_2().child(crate::demo::mark("welcome-new", button)));
        // The field is its own cached view: its frames re-render only it, and nothing runs
        // while it is still.
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .child(self.pi_field.clone().cached(full())),
            )
            .child(content)
    }
}

/// The welcome screen's "New session" menu: recent folders (checked now, not at startup),
/// "Choose a folder…", and SSH hosts.
fn welcome_menu(
    mut menu: component::menu::PopupMenu,
    window_view: WeakEntity<MainWindow>,
    cx: &mut Context<component::menu::PopupMenu>,
) -> component::menu::PopupMenu {
    let home = cx.global::<AppConfig>().home.clone();
    let recent: Vec<PathBuf> = cx
        .try_global::<crate::config::RecentFolders>()
        .map_or_default(|recent| recent.0.clone())
        .into_iter()
        .filter(|folder| folder.is_dir())
        .take(crate::config::RECENT_FOLDERS)
        .collect();
    menu = menu.min_w(px(300.)).max_w(px(460.));
    if !recent.is_empty() {
        menu = menu.label("Recent folders");
        for folder in recent {
            // Name, then its folder in muted text, cut with an ellipsis: long paths must not
            // stretch the menu.
            let (name, parent) = crate::chat::composer::folder_label(&folder, home.as_deref());
            let view = window_view.clone();
            menu = menu.item(
                component::menu::PopupMenuItem::element(move |_, cx| {
                    let muted = cx.theme().muted_foreground;
                    h_flex()
                        .w_full()
                        .max_w(px(400.))
                        .min_w_0()
                        .gap_2()
                        .child(
                            component::Icon::new(IconName::Folder)
                                .small()
                                .text_color(muted),
                        )
                        .child(
                            div()
                                .flex_none()
                                .max_w(px(180.))
                                .truncate()
                                .child(name.clone()),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(muted)
                                .child(parent.clone()),
                        )
                })
                .on_click(move |_, window, cx| {
                    let folder = folder.clone();
                    let _ = view.update(cx, |this, cx| this.new_chat_in(folder, window, cx));
                }),
            );
        }
        menu = menu.separator();
    }
    let view = window_view.clone();
    menu = menu.item(
        component::menu::PopupMenuItem::new("Choose a folder…")
            .icon(IconName::FolderOpen)
            .on_click(move |_, window, cx| {
                let _ = view.update(cx, |this, cx| this.choose_folder(None, window, cx));
            }),
    );
    let hosts: Vec<(HostId, String, crate::hosts::HostStatus)> = {
        let hosts = crate::hosts::hosts(cx);
        let hosts = hosts.read(cx);
        hosts
            .hosts()
            .iter()
            .map(|host| (host.id.clone(), host.label.clone(), hosts.status(&host.id)))
            .collect()
    };
    menu = menu.separator().label("On an SSH host");
    for (id, label, status) in hosts {
        let view = window_view.clone();
        menu = menu.item(
            component::menu::PopupMenuItem::element(move |_, cx| {
                h_flex()
                    .gap_2()
                    .child(crate::remote_ui::status_dot(Some(&status), cx))
                    .child(label.clone())
            })
            .on_click(move |_, window, cx| {
                let id = id.clone();
                let _ = view.update(cx, |this, cx| this.open_connect_flow(id, window, cx));
            }),
        );
    }
    let view = window_view;
    menu.item(
        component::menu::PopupMenuItem::new("Add SSH connection…")
            .icon(IconName::Plus)
            .on_click(move |_, window, cx| {
                let _ = view.update(cx, |this, cx| this.open_add_host(window, cx));
            }),
    )
}

impl Focusable for MainWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _timer = crate::perf::render("window");
        // The window's own buttons (Windows, Linux without server decorations) sit over the
        // right end of the rightmost column's top bar, which keeps room for them.
        let controls = crate::ui::window_controls::inset(window);
        let side_open = self.side_chat().is_some();
        let (chat_inset, side_inset) = match (self.right_open, side_open) {
            (true, _) => (px(0.), px(0.)),
            (false, true) => (px(0.), controls),
            (false, false) => (controls, px(0.)),
        };
        let (background, foreground, font_family) = (
            cx.theme().background,
            cx.theme().foreground,
            cx.theme().font_family.clone(),
        );
        let settings = self.settings.as_ref().map(|(settings, _)| settings.clone());
        let skills = self
            .skills
            .as_ref()
            .filter(|_| self.skills_open && settings.is_none())
            .map(|(skills, _)| skills.clone());
        let page_open = self.page_open();
        let chat_column = v_flex()
            .size_full()
            .min_w_0()
            // Whatever does not fit a narrow column is clipped here, never drawn over the
            // right panel.
            .overflow_hidden()
            .child(self.render_chat_header(chat_inset, cx))
            .when_some(settings, |this, settings| {
                let pi_field = settings.read(cx).pi_field();
                this.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .overflow_hidden()
                        .child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full()
                                .child(pi_field.cached(full())),
                        )
                        .child(settings.cached(full())),
                )
            })
            .when_some(skills, |this, skills| {
                this.child(div().flex_1().min_h_0().child(skills.cached(full())))
            })
            .when(self.welcome && !page_open, |this| {
                this.child(self.render_welcome(cx))
            })
            .when(!page_open && !self.welcome, |this| {
                let terminal = self.terminal.clone().filter(|_| self.terminal_open);
                this.child(
                    v_flex()
                        .id("chat-and-terminal")
                        .flex_1()
                        .min_h_0()
                        .on_drag_move(cx.listener(Self::drag_terminal_edge))
                        .on_drop(cx.listener(|this, _: &TerminalEdge, _, cx| {
                            this.save_terminal_height(cx);
                        }))
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_h_0()
                                .child(self.chat.clone().cached(full()))
                                .children(self.render_side_tab(cx)),
                        )
                        .when_some(terminal, |this, terminal| {
                            this.child(
                                v_flex()
                                    .flex_none()
                                    .h(self.terminal_height)
                                    // A height remembered on a bigger screen must not
                                    // squeeze out the chat and its message box.
                                    .max_h(relative(0.75))
                                    .child(render_terminal_edge(cx))
                                    .child(div().flex_1().min_h_0().child(terminal.cached(full()))),
                            )
                        }),
                )
            });

        let side_column = self.render_side_column(side_inset, cx);
        div()
            .key_context(WINDOW_CONTEXT)
            .on_key_down(cx.listener(Self::update_prompt_key))
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::new_chat))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::toggle_right_panel))
            .on_action(cx.listener(Self::open_side_chat))
            .on_action(cx.listener(Self::toggle_terminal))
            .on_action(cx.listener(Self::show_diff))
            .on_action(cx.listener(Self::show_files))
            .on_action(cx.listener(Self::open_file_link))
            .on_action(cx.listener(Self::focus_composer))
            .on_action(cx.listener(Self::find_in_chat))
            .on_action(cx.listener(Self::show_chat_mode))
            .on_action(cx.listener(Self::show_code_mode))
            .on_action(cx.listener(Self::resume_session))
            .on_action(cx.listener(Self::reload_session))
            .on_action(cx.listener(Self::open_login))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::open_security_settings))
            .on_action(cx.listener(|this, _: &CheckForUpdates, window, cx| {
                this.check_for_updates(true, window, cx);
            }))
            .on_action(|_: &crate::actions::CloseWindow, window, cx| {
                if cfg!(target_os = "macos") {
                    cx.hide();
                } else {
                    window.remove_window();
                }
            })
            .on_action(|_: &crate::actions::Minimize, window, _| window.minimize_window())
            .on_action(|_: &crate::actions::Zoom, window, _| window.zoom_window())
            .size_full()
            .bg(background)
            .text_color(foreground)
            .font_family(font_family)
            .child(
                h_resizable("main-columns")
                    .child(
                        resizable_panel()
                            .visible(self.sidebar_open)
                            .size(px(272.))
                            .size_range(px(200.)..px(440.))
                            .child(self.sidebar.clone().cached(full())),
                    )
                    .child(
                        resizable_panel()
                            // Dragging the right panel wide stops here, so the chat keeps room
                            // for its header buttons and the message box's controls.
                            .size_range(MIN_CHAT_WIDTH..Pixels::MAX)
                            .child(chat_column),
                    )
                    .child(
                        resizable_panel()
                            .visible(side_column.is_some())
                            .size(px(520.))
                            .size_range(MIN_CHAT_WIDTH..Pixels::MAX)
                            .children(side_column),
                    )
                    .child(
                        resizable_panel()
                            .visible(self.right_open)
                            .size(px(460.))
                            .size_range(px(300.)..px(1200.))
                            .child(self.right_panel.clone().cached(full())),
                    ),
            )
            .children(crate::ui::window_controls::render(window, cx))
            .children(self.render_update_scrim(cx))
            .children(self.render_update_card(cx))
            // Above menus and popovers (deferred at low priorities).
            .when_some(self.demo.as_ref(), |this, (cursor, _)| {
                this.child(deferred(cursor.clone()).with_priority(1000))
            })
    }
}
