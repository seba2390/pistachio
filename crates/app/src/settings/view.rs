//! The Settings page, shown in the chat column: General, pi & models, Web search, SSH
//! connections, Speech to text, Conversation names, Document parsing, Sandboxing & Security and
//! Tools. Every change is saved right away.

use std::path::{Path, PathBuf};

use deepsearch::config::{BackendKind, Config as WebConfig, Mode as WebMode};
use deepsearch::effort::Effort;
use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::setting::{
    RenderOptions, SelectIndex, SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_config::settings::{StartupDefaults, THINKING_LEVELS};
use searxng::{Installed, Probe};

use super::{AppSettings, StartMode, TextSize, ThemeChoice, local_search, web_advanced};
use crate::chat::models_dialog::Open as ModelsOpen;
use crate::code_colors::CodeColors;
use crate::config::AppConfig;
use crate::document_parsers::{DocumentParsers, document_parsers};
use crate::remote_ui::SavedHostAction;
use crate::search_server::{Running, SearchServerEvent, search_server};
use crate::ui::spinner::IndicatorStyle;
use crate::ui::{AppIcon, PiField};

/// What the window does for the page (dialogs and connections live there).
#[derive(Debug, Clone)]
pub enum SettingsEvent {
    /// Opens the dialog for this computer's models.
    Models(ModelsOpen),
    /// A provider was removed from this computer's models.json; says what was done.
    ModelsRemoved(SharedString),
    Host(SavedHostAction),
    CheckForUpdates,
}

/// Web search's `config.json`, loaded when the page opens.
struct WebSearchState {
    dir: Option<PathBuf>,
    config: WebConfig,
    error: Option<String>,
    /// SearXNG was just chosen and may have to be set up first.
    choosing: Option<Choosing>,
}

impl Global for WebSearchState {}

/// Choosing SearXNG instead of `before`, which comes back when it is not set up after all.
struct Choosing {
    before: BackendKind,
    /// The user was asked whether to download it.
    asked: bool,
}

/// The startup model and thinking level in pi's `settings.json` on this computer.
struct PiStartupState {
    path: Option<PathBuf>,
    defaults: StartupDefaults,
    error: Option<String>,
}

impl Global for PiStartupState {}

pub struct SettingsView {
    /// Sandboxing & Security: the field of the allowed sites.
    site_input: Entity<InputState>,
    brave_key: Entity<InputState>,
    tavily_key: Entity<InputState>,
    /// `(provider/id, label)` of the models to choose the startup model from.
    models: Vec<(SharedString, SharedString)>,
    /// The models (`provider`, `id`, `name`) of the visible session, when it uses this
    /// computer's.
    session_models: Vec<(String, String, String)>,
    /// The models (`provider`, `id`) pi lists on this computer, once it has.
    listed_models: Option<Vec<(String, String)>>,
    _list_models: Task<()>,
    /// The providers in models.json on this computer.
    pub(super) connected: super::models::Connected,
    /// `pi --version` of the configured `pi`, once known.
    pi_version: Option<String>,
    /// The sections of web search that few people change.
    pub(super) advanced: web_advanced::Advanced,
    /// The page shown first (scripted runs open Speech to text directly).
    start_page: usize,
    /// The section of it scrolled to first, if any.
    start_group: Option<usize>,
    /// The Code colors palette under the pointer in the open picker; the preview shows it.
    code_colors_hover: Option<CodeColors>,
    /// The working-indicator style under the pointer in the open picker; the preview shows it.
    indicator_hover: Option<IndicatorStyle>,
    /// The indicator preview was drawn since the last tick; its repaint loop runs only while
    /// it keeps being drawn (the General page is open).
    indicator_drawn: std::rc::Rc<std::cell::Cell<bool>>,
    /// The preview's repaint loop is running.
    indicator_ticking: std::rc::Rc<std::cell::Cell<bool>>,
    /// The drifting π signs behind the pages.
    pi_field: Entity<PiField>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    /// `session_models` are the visible session's models (`provider`, `id`, `name`), when it
    /// uses this computer's.
    pub fn new(
        session_models: Vec<(String, String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        load_web_search(cx);
        load_pi_startup(cx);
        let (brave, tavily) = {
            let config = &cx.global::<WebSearchState>().config;
            (
                config.brave.api_key.clone().unwrap_or_default(),
                config.tavily.api_key.clone().unwrap_or_default(),
            )
        };
        let key_input = |value: String, placeholder: &str, window: &mut Window, cx: &mut App| {
            let placeholder = placeholder.to_owned();
            cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder(placeholder)
                    .default_value(value)
            })
        };
        let brave_key = key_input(brave, "From BRAVE_API_KEY if empty", window, cx);
        let site_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("github.com, or github.com/* with its subdomains")
        });
        let tavily_key = key_input(tavily, "From TAVILY_API_KEY if empty", window, cx);
        let parsers = document_parsers(cx);
        parsers.update(cx, DocumentParsers::refresh);
        let subscriptions = vec![
            // Speech to text, conversation names and document parsers: download progress and installed models.
            cx.observe(&crate::stt::models::models(cx), |_, _, cx| cx.notify()),
            cx.observe(&crate::naming::models(cx), |_, _, cx| cx.notify()),
            cx.observe(&parsers, |_, _, cx| cx.notify()),
            cx.subscribe(&brave_key, |_, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let key = input.read(cx).value().trim().to_owned();
                    update_web(cx, |config| {
                        config.brave.api_key = Some(key).filter(|k| !k.is_empty());
                    });
                }
            }),
            cx.subscribe(&tavily_key, |_, input, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    let key = input.read(cx).value().trim().to_owned();
                    update_web(cx, |config| {
                        config.tavily.api_key = Some(key).filter(|k| !k.is_empty());
                    });
                }
            }),
            cx.observe(&crate::hosts::hosts(cx), |_, _, cx| cx.notify()),
            // The chosen search service's field.
            cx.observe_global_in::<WebSearchState>(window, |_, window, cx| {
                check_searxng(false, cx);
                offer_searxng(window, cx);
                cx.notify();
            }),
            cx.observe_in(&search_server(cx), window, |_, _, window, cx| {
                offer_searxng(window, cx);
                cx.notify();
            }),
            cx.subscribe(&search_server(cx), |_, _, event, cx| match event {
                SearchServerEvent::Ready => keep_service(cx),
                SearchServerEvent::Cancelled => download_cancelled(cx),
                SearchServerEvent::Failed(_) => {}
            }),
        ];
        check_searxng(true, cx);
        // Settings → Sandboxing & Security: read again what may have changed elsewhere (/trust, the file).
        let security = crate::security::security(cx);
        security.update(cx, crate::security::Security::refresh);
        // Settings → Tools asks pi again when it is next shown.
        let tools = super::tools::tool_catalog(cx);
        tools.update(cx, |tools, _| tools.mark_stale());
        let mut subscriptions = subscriptions;
        subscriptions.push(cx.observe(&security, |_, _, cx| cx.notify()));
        subscriptions.push(cx.observe(&tools, |_, _, cx| cx.notify()));
        // Local models start and stop from the list of connected models.
        if let Some(models) = crate::local_models::local_models(cx) {
            subscriptions.push(cx.observe(&models, |_, _, cx| cx.notify()));
        }
        // The site field is checked as it is typed; Enter adds it.
        subscriptions.push(cx.subscribe_in(
            &site_input,
            window,
            |_, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    let text = input.read(cx).value().to_string();
                    crate::security::security(cx).update(cx, |s, cx| s.check_site(&text, cx));
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    super::security::add_site(input, window, cx);
                }
                _ => {}
            },
        ));
        let connected =
            super::models::Connected::load(session_models.iter().map(|model| &*model.0), cx);
        let mut this = Self {
            site_input,
            brave_key,
            tavily_key,
            models: Vec::new(),
            session_models,
            listed_models: None,
            _list_models: Task::ready(()),
            connected,
            pi_version: None,
            advanced: web_advanced::Advanced::default(),
            start_page: 0,
            start_group: None,
            pi_field: cx.new(|_| PiField::plain()),
            code_colors_hover: None,
            indicator_hover: None,
            indicator_drawn: std::rc::Rc::default(),
            indicator_ticking: std::rc::Rc::default(),
            _subscriptions: subscriptions,
        };
        this.probe_pi_version(cx);
        this.update_models();
        this.list_models(cx);
        this
    }

    /// models.json was saved.
    pub fn models_changed(&mut self, cx: &mut Context<Self>) {
        self.connected.reload(cx);
        self.listed_models = None;
        self.update_models();
        self.list_models(cx);
        cx.notify();
    }

    /// Sets what the startup model is chosen from.
    fn update_models(&mut self) {
        self.models = pi_config::available::choices(
            self.listed_models.as_deref(),
            self.connected.providers(),
            &self.session_models,
        )
        .into_iter()
        .map(|choice| (choice.value.into(), choice.label.into()))
        .collect();
    }

    /// Asks the configured `pi` in the background which models it has on this computer: there
    /// may be no session, or one that uses the models of its SSH host.
    fn list_models(&mut self, cx: &mut Context<Self>) {
        let (program, env) = {
            let config = cx.global::<AppConfig>();
            (config.pi_program.clone(), config.child_env.clone())
        };
        self._list_models = cx.spawn(async move |this, cx| {
            let listed = cx
                .background_executor()
                .spawn(async move {
                    let mut command = ::platform::command(&program);
                    command
                        .arg("--list-models")
                        .envs(env.vars().iter().cloned())
                        .stdin(std::process::Stdio::null());
                    // No project's settings.
                    if let Some(dir) = pi_sessions::agent_dir().filter(|dir| dir.is_dir()) {
                        command.current_dir(dir);
                    }
                    let output = command.output().ok()?;
                    let text = String::from_utf8_lossy(&output.stdout);
                    output
                        .status
                        .success()
                        .then(|| pi_config::available::parse_list(&text))?
                })
                .await;
            let Some(listed) = listed else {
                log::warn!("pi --list-models did not list the models");
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.connected
                    .set_available(listed.iter().map(|model| &*model.0));
                this.listed_models = Some(listed);
                this.update_models();
                cx.notify();
            });
        });
    }

    /// Asks the configured `pi` for its version in the background.
    fn probe_pi_version(&mut self, cx: &mut Context<Self>) {
        let (program, env) = {
            let config = cx.global::<AppConfig>();
            (config.pi_program.clone(), config.child_env.clone())
        };
        cx.spawn(async move |this, cx| {
            let version = cx
                .background_executor()
                .spawn(async move {
                    ::platform::command(&program)
                        .arg("--version")
                        .envs(env.vars().iter().cloned())
                        .stdin(std::process::Stdio::null())
                        .output()
                        .ok()
                        .filter(|output| output.status.success())
                        .and_then(|output| {
                            String::from_utf8_lossy(&output.stdout)
                                .lines()
                                .last()
                                .map(|line| line.trim().to_owned())
                        })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.pi_version = Some(version.unwrap_or_else(|| "not found".to_owned()));
                cx.notify();
            });
        })
        .detach();
    }

    fn general_page(&self, weak: &WeakEntity<Self>) -> SettingPage {
        let weak = weak.clone();
        SettingPage::new("General")
            .icon(IconName::Settings)
            .default_open(true)
            .group(
                SettingGroup::new()
                    .title("Appearance")
                    .item(SettingItem::new(
                        "Theme",
                        if crate::demo::enabled() {
                            demo_theme_field()
                        } else {
                            SettingField::dropdown(
                                options(&[
                                    ("system", "Follow system"),
                                    ("light", "Light"),
                                    ("dark", "Dark"),
                                ]),
                                |cx| {
                                    match cx.global::<AppSettings>().theme {
                                        ThemeChoice::System => "system",
                                        ThemeChoice::Light => "light",
                                        ThemeChoice::Dark => "dark",
                                    }
                                    .into()
                                },
                                |value, cx| {
                                    let theme = match value.as_ref() {
                                        "light" => ThemeChoice::Light,
                                        "dark" => ThemeChoice::Dark,
                                        _ => ThemeChoice::System,
                                    };
                                    AppSettings::update(cx, |s| s.theme = theme);
                                },
                            )
                        },
                    ))
                    .item(
                        SettingItem::new("Code colors", code_colors_field(weak.clone()))
                            .description(
                                "Syntax colors for code in chat, skills and models.json. \
                             Each has a light and a dark variant.",
                            ),
                    )
                    .item(SettingItem::render({
                        let weak = weak.clone();
                        move |_, _, cx| {
                            let hovered = weak
                                .upgrade()
                                .and_then(|view| view.read(cx).code_colors_hover);
                            let chosen = cx.global::<AppSettings>().code_colors;
                            code_colors_preview(hovered.unwrap_or(chosen), cx)
                        }
                    }))
                    .item(
                        SettingItem::new("Working indicator", indicator_field(weak.clone()))
                            .description(
                                "How a working agent is shown in the chat and the sidebar.",
                            ),
                    )
                    .item(SettingItem::render({
                        let weak = weak.clone();
                        move |_, _, cx| indicator_preview(&weak, cx)
                    }))
                    .item(
                        SettingItem::new(
                            "Text size",
                            SettingField::dropdown(
                                options(&TextSize::ALL.map(|size| (size.id(), size.label()))),
                                |cx| cx.global::<AppSettings>().text_size.id().into(),
                                |value, cx| {
                                    let size = TextSize::ALL
                                        .into_iter()
                                        .find(|size| size.id() == value.as_ref())
                                        .unwrap_or_default();
                                    AppSettings::update(cx, |s| s.text_size = size);
                                },
                            ),
                        )
                        .description("Scales the whole interface."),
                    ),
            )
            .group(
                SettingGroup::new()
                    .title("Start")
                    .item(SettingItem::new(
                        "Open in",
                        SettingField::dropdown(
                            options(&[
                                ("last", "The mode used last"),
                                ("chat", "Chat"),
                                ("code", "Code"),
                            ]),
                            |cx| {
                                match cx.global::<AppSettings>().start_mode {
                                    StartMode::Last => "last",
                                    StartMode::Chat => "chat",
                                    StartMode::Code => "code",
                                }
                                .into()
                            },
                            |value, cx| {
                                let mode = match value.as_ref() {
                                    "chat" => StartMode::Chat,
                                    "code" => StartMode::Code,
                                    _ => StartMode::Last,
                                };
                                AppSettings::update(cx, |s| s.start_mode = mode);
                            },
                        ),
                    ))
                    .item(
                        SettingItem::new(
                            "Reopen the last session",
                            hover_switch(
                                "settings-switch-1",
                                |cx| cx.global::<AppSettings>().reopen_last_session,
                                |on, cx| AppSettings::update(cx, |s| s.reopen_last_session = on),
                            ),
                        )
                        .description(
                            "Instead of the welcome screen, also for sessions on SSH hosts.",
                        ),
                    ),
            )
            .group(
                SettingGroup::new().title("Chat mode").item(
                    SettingItem::new(
                        "Compute with Python",
                        hover_switch(
                            "settings-switch-python",
                            |cx| python_tool::enabled(&cx.global::<AppConfig>().python_dir),
                            |on, cx| {
                                let dir = cx.global::<AppConfig>().python_dir.clone();
                                if let Err(error) = python_tool::set_enabled(&dir, on) {
                                    log::warn!(
                                        "could not save the Python setting in {}: {error}",
                                        dir.display()
                                    );
                                }
                            },
                        ),
                    )
                    .description(
                        "The model works numbers out in Python instead of estimating them, and \
                             uses it for other small jobs like counting or reformatting text. The \
                             code runs in Monty, a sealed interpreter: no files, network or \
                             programs. Applies from the next message, also in open chats.",
                    )
                    .keywords(["python", "monty", "calculate", "math", "code"]),
                ),
            )
            .group(
                SettingGroup::new()
                    .title("Background chats")
                    .item(
                        SettingItem::new(
                            "Notifications",
                            hover_switch(
                                "settings-switch-2",
                                |cx| cx.global::<AppSettings>().background_notifications,
                                |on, cx| {
                                    AppSettings::update(cx, |s| s.background_notifications = on);
                                },
                            ),
                        )
                        .description(
                            "When a chat working in the background finishes or asks something.",
                        ),
                    )
                    .item(
                        SettingItem::new(
                            if cfg!(target_os = "macos") {
                                "Bounce the Dock icon"
                            } else {
                                "Flash the window"
                            },
                            hover_switch(
                                "settings-switch-3",
                                |cx| cx.global::<AppSettings>().request_attention,
                                |on, cx| AppSettings::update(cx, |s| s.request_attention = on),
                            ),
                        )
                        .description("When that happens while Pistachio is not in front."),
                    ),
            )
            .group(
                SettingGroup::new()
                    .title("Updates")
                    .item(
                        SettingItem::new(
                            "Check for updates when Pistachio opens",
                            hover_switch(
                                "settings-switch-4",
                                |cx| cx.global::<AppSettings>().check_for_updates,
                                |on, cx| AppSettings::update(cx, |s| s.check_for_updates = on),
                            ),
                        )
                        .description(
                            "Asks GitHub for the latest release (no personal data is sent). \
                             You are asked before anything is downloaded.",
                        ),
                    )
                    .item(SettingItem::render(move |_, _, cx| {
                        let weak = weak.clone();
                        h_flex()
                            .w_full()
                            .justify_between()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "Pistachio {}",
                                        crate::updates::CURRENT_VERSION
                                    )),
                            )
                            .child(
                                Button::new("check-updates-now")
                                    .small()
                                    .label("Check now")
                                    .on_click(move |_, _, cx| {
                                        let _ = weak.update(cx, |_, cx| {
                                            cx.emit(SettingsEvent::CheckForUpdates);
                                        });
                                    }),
                            )
                    })),
            )
    }

    fn pi_page(&self, weak: &WeakEntity<Self>, cx: &App) -> SettingPage {
        let now = match &self.pi_version {
            Some(version) => version.clone(),
            None => "checking…".to_owned(),
        };
        let pi_description = if std::env::var_os("PISTACHIO_PI_BIN").is_some() {
            "Set by PISTACHIO_PI_BIN, which takes precedence.".to_owned()
        } else {
            format!("Leave empty to find pi on PATH. Version: {now}. Used by new sessions.")
        };
        let mut model_options: Vec<(SharedString, SharedString)> =
            vec![("".into(), "Let pi choose".into())];
        model_options.extend(self.models.iter().cloned());
        // A model set earlier that pi does not have now still shows.
        let current = startup_model(cx);
        if !current.is_empty() && !model_options.iter().any(|(value, _)| *value == current) {
            model_options.push((current.clone().into(), current.into()));
        }
        let thinking_options: Vec<(SharedString, SharedString)> =
            std::iter::once((SharedString::from(""), SharedString::from("Let pi choose")))
                .chain(
                    THINKING_LEVELS
                        .iter()
                        .map(|level| (SharedString::from(*level), SharedString::from(*level))),
                )
                .collect();
        let models_hint = if self.models.is_empty() {
            "Connect models below to choose one."
        } else {
            "The model new sessions start with."
        };
        let weak_models = weak.clone();
        SettingPage::new("pi & models")
            .icon(AppIcon::PiLogo)
            .group(
                SettingGroup::new().title("pi").item(
                    SettingItem::new(
                        "pi program",
                        SettingField::input(
                            |cx| {
                                cx.global::<AppSettings>()
                                    .pi_program
                                    .clone()
                                    .unwrap_or_default()
                                    .into()
                            },
                            |value, cx| {
                                let path = value.trim().to_owned();
                                AppSettings::update(cx, |s| {
                                    s.pi_program = Some(path).filter(|p| !p.is_empty());
                                });
                            },
                        ),
                    )
                    .description(pi_description),
                ),
            )
            .group(
                SettingGroup::new()
                    .title("Startup model")
                    .description(
                        "Saved in pi's settings.json, like pi's /settings in the terminal.",
                    )
                    .item(
                        SettingItem::new(
                            "Model",
                            SettingField::scrollable_dropdown(
                                model_options,
                                |cx| startup_model(cx).into(),
                                |value, cx| {
                                    let (provider, model) = match value.split_once('/') {
                                        Some((provider, model)) => {
                                            (Some(provider.to_owned()), Some(model.to_owned()))
                                        }
                                        None => (None, None),
                                    };
                                    update_pi_startup(cx, |defaults| {
                                        defaults.provider = provider;
                                        defaults.model = model;
                                    });
                                },
                            ),
                        )
                        .description(models_hint),
                    )
                    .item(SettingItem::new(
                        "Thinking level",
                        SettingField::dropdown(
                            thinking_options,
                            |cx| {
                                cx.global::<PiStartupState>()
                                    .defaults
                                    .thinking_level
                                    .clone()
                                    .unwrap_or_default()
                                    .into()
                            },
                            |value, cx| {
                                let level = Some(value.to_string()).filter(|v| !v.is_empty());
                                update_pi_startup(cx, |defaults| defaults.thinking_level = level);
                            },
                        ),
                    ))
                    .item(SettingItem::render(move |_, _, cx| {
                        div().when_some(
                            cx.global::<PiStartupState>().error.clone(),
                            |this, error| this.text_sm().text_color(cx.theme().danger).child(error),
                        )
                    })),
            )
            .group(
                SettingGroup::new()
                    .title("Connected models")
                    .description(
                        "The servers and API keys in models.json on this computer. Sign-ins with \
                         /login are not listed here.",
                    )
                    .item(
                        SettingItem::render(move |_, _, cx| super::models::rows(&weak_models, cx))
                            .keywords(["API key", "provider", "server", "Ollama", "models.json"]),
                    ),
            )
    }

    /// The search service, and what the chosen one needs.
    fn search_service_group(&self, cx: &App) -> SettingGroup {
        let key_field = |name: &'static str, input: &Entity<InputState>| {
            let input = input.clone();
            SettingField::render(move |_, _, _| {
                div()
                    .debug_selector(|| name.into())
                    .child(Input::new(&input).mask_toggle().w(px(260.)))
            })
        };
        let key_hint = "Stored only on this computer, readable by you alone.";
        let service = SettingItem::new(
            "Service",
            SettingField::dropdown(
                options(&[
                    ("duckduckgo", "DuckDuckGo (no key)"),
                    ("searxng", "SearXNG (no key)"),
                    ("brave", "Brave Search (API key)"),
                    ("tavily", "Tavily (API key)"),
                ]),
                |cx| backend_value(cx.global::<WebSearchState>().config.backend).into(),
                |value, cx| {
                    let backend = match value.as_ref() {
                        "searxng" => BackendKind::Searxng,
                        "brave" => BackendKind::Brave,
                        "tavily" => BackendKind::Tavily,
                        _ => BackendKind::Duckduckgo,
                    };
                    choose_service(backend, cx);
                },
            ),
        )
        // Found by the settings search also while another one is chosen.
        .keywords([
            "DuckDuckGo",
            "SearXNG address",
            "Brave API key",
            "Tavily API key",
        ]);
        let needs = match web_config(cx).backend {
            BackendKind::Duckduckgo => Vec::new(),
            BackendKind::Searxng => vec![local_search::item()],
            BackendKind::Brave => vec![
                SettingItem::new("Brave API key", key_field("brave-key", &self.brave_key))
                    .description(key_hint),
            ],
            BackendKind::Tavily => vec![
                SettingItem::new("Tavily API key", key_field("tavily-key", &self.tavily_key))
                    .description(key_hint),
            ],
        };
        SettingGroup::new()
            .title("Search service")
            .item(service)
            .items(needs)
    }

    fn web_search_page(&self, weak: &WeakEntity<Self>, cx: &App) -> SettingPage {
        let page = weak.clone();
        SettingPage::new("Web search")
            .icon(AppIcon::Globe)
            .description("For chats with web search on (the globe next to the paperclip).")
            .group(self.search_service_group(cx))
            .group(
                SettingGroup::new().title("New chats").item(SettingItem::new(
                    "Web search",
                    SettingField::dropdown(
                        options(&[("off", "Off"), ("search", "Search"), ("deep", "Deep search")]),
                        |cx| cx.global::<WebSearchState>().config.default_mode.as_str().into(),
                        |value, cx| {
                            if let Some(mode) = WebMode::parse(&value) {
                                update_web(cx, |config| config.default_mode = mode);
                            }
                        },
                    ),
                ))
                .item(SettingItem::new(
                    "Deep search effort",
                    SettingField::dropdown(
                        Effort::ALL
                            .map(|effort| (effort.as_str().into(), effort.label().into()))
                            .into(),
                        |cx| web_config(cx).default_effort.as_str().into(),
                        |value, cx| {
                            if let Some(effort) = Effort::parse(&value) {
                                update_web(cx, |config| config.default_effort = effort);
                            }
                        },
                    ),
                )),
            )
            .group(web_advanced::search_group(&self.advanced, weak))
            .group(web_advanced::deep_group(&self.advanced, weak))
            .group(
                SettingGroup::new()
                    .title("Network")
                    .item(
                        SettingItem::new(
                            "Allow local network addresses",
                            hover_switch("settings-switch-5",
                                |cx| cx.global::<WebSearchState>().config.fetch.allow_private_networks,
                                |on, cx| update_web(cx, |c| c.fetch.allow_private_networks = on),
                            ),
                        )
                        .description(
                            "Off by default: pages the model asks for can then reach devices on your network.",
                        ),
                    )
                    .item(SettingItem::render(move |_, window, cx| {
                        let room = page
                            .upgrade()
                            .map(|view| view.read(cx).advanced.room(window));
                        div()
                            .debug_selector(|| "web-network".into())
                            .pb(room.unwrap_or_default())
                            .when_some(cx.global::<WebSearchState>().error.clone(), |this, error| {
                            this.text_sm().text_color(cx.theme().danger).child(error)
                        })
                    })),
            )
    }

    fn ssh_page(&self, weak: &WeakEntity<Self>) -> SettingPage {
        let weak = weak.clone();
        SettingPage::new("SSH connections")
            .icon(IconName::Network)
            .description("Run pi on other machines.")
            .group(
                SettingGroup::new().item(
                    SettingItem::new(
                        "Connect saved connections at start",
                        hover_switch(
                            "settings-switch-hosts",
                            |cx| cx.global::<AppSettings>().connect_hosts_at_start,
                            |on, cx| AppSettings::update(cx, |s| s.connect_hosts_at_start = on),
                        ),
                    )
                    .description(
                        "Connects quietly in the background (no password prompts), so status and \
                         sessions are ready. Takes effect the next time Pistachio opens.",
                    ),
                ),
            )
            .group(
                SettingGroup::new()
                    .title("Saved connections")
                    .item(SettingItem::render(move |_, _, cx| {
                        let weak = weak.clone();
                        crate::remote_ui::saved_hosts(
                            move |action, _, cx| {
                                let _ = weak.update(cx, |_, cx| {
                                    cx.emit(SettingsEvent::Host(action));
                                });
                            },
                            cx,
                        )
                    })),
            )
    }
}

impl SettingsView {
    /// The π signs, for the window to draw behind this view: drawn in it, each of their frames
    /// would render the pages again.
    pub fn pi_field(&self) -> Entity<PiField> {
        self.pi_field.clone()
    }

    /// Opens on the Web search page (before the first render).
    pub fn start_on_web_search(&mut self) {
        self.start_page = 2;
    }

    /// Opens every section of the Web search page.
    pub fn open_advanced(&mut self) {
        self.advanced.open_all();
    }

    /// Opens on the SSH connections page (before the first render).
    pub fn start_on_ssh(&mut self) {
        self.start_page = 3;
    }

    /// Opens on the Speech to text page (before the first render).
    pub fn start_on_speech(&mut self) {
        self.start_page = 4;
    }

    /// Opens on the Conversation names page (before the first render).
    pub fn start_on_names(&mut self) {
        self.start_page = 5;
    }

    /// Opens on the pi & models page (before the first render).
    pub fn start_on_models(&mut self) {
        self.start_page = 1;
    }

    /// Opens on the Document parsing page (before the first render).
    pub fn start_on_documents(&mut self) {
        self.start_page = 6;
    }

    /// Opens on the Security page (before the first render).
    pub fn start_on_security(&mut self) {
        self.start_page = 7;
    }

    /// Opens on the Tools page (before the first render).
    pub fn start_on_tools(&mut self) {
        self.start_page = 8;
    }

    /// Opens on page `page`, scrolled to its section `group` (before the first render; for
    /// scripted screenshots, `PISTACHIO_OPEN=settings-at:<page>:<group>`).
    pub fn start_on(&mut self, page: usize, group: Option<usize>) {
        self.start_page = page;
        self.start_group = group;
    }

    /// Opens on the Security page's Git and GitHub section (before the first render).
    pub fn start_on_git(&mut self) {
        self.start_page = 7;
        self.start_group = Some(3);
    }

    fn speech_page(&self) -> SettingPage {
        use crate::stt::{AutoStop, catalog};
        SettingPage::new("Speech to text")
            .icon(IconName::Mic)
            .description(
                "Dictate messages with the microphone button. Speech is transcribed by a model \
                 that runs on this computer; nothing is sent anywhere.",
            )
            .group(
                SettingGroup::new()
                    .title("Models")
                    .item(SettingItem::render(|_, _, cx| {
                        crate::online::refresh(cx);
                        crate::stt::ui::settings_model_rows(cx)
                    })),
            )
            .group(
                SettingGroup::new()
                    .title("Dictation")
                    .item(
                        SettingItem::new(
                            "Stop listening",
                            SettingField::dropdown(
                                AutoStop::ALL
                                    .into_iter()
                                    .map(|stop| (stop.id().into(), stop.label().into()))
                                    .collect(),
                                |cx| cx.global::<AppSettings>().speech_auto_stop.id().into(),
                                |value, cx| {
                                    let stop = AutoStop::from_id(value.as_ref());
                                    AppSettings::update(cx, |s| s.speech_auto_stop = stop);
                                },
                            ),
                        )
                        .description(
                            "Dictation ends by itself after this much silence. Enter or the stop \
                             button end it at any time; Esc discards it.",
                        ),
                    )
                    .item(
                        SettingItem::new(
                            "Language (Whisper)",
                            SettingField::dropdown(
                                catalog::LANGUAGES
                                    .iter()
                                    .map(|(code, label)| ((*code).into(), (*label).into()))
                                    .collect(),
                                |cx| cx.global::<AppSettings>().speech_language.clone().into(),
                                |value, cx| {
                                    let language = value.to_string();
                                    AppSettings::update(cx, |s| s.speech_language = language);
                                },
                            ),
                        )
                        .description(
                            "Whisper is more accurate when told the language. Parakeet detects \
                             it by itself; Moonshine understands English only.",
                        ),
                    ),
            )
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _timer = crate::perf::render("settings");
        let weak = cx.weak_entity();
        // No background: the π signs are behind it.
        div().size_full().child(
            Settings::new("app-settings")
                .default_selected_index(SelectIndex {
                    page_ix: self.start_page,
                    group_ix: self.start_group,
                })
                .pages([
                    self.general_page(&weak),
                    self.pi_page(&weak, cx),
                    self.web_search_page(&weak, cx),
                    self.ssh_page(&weak),
                    self.speech_page(),
                    naming_page(cx),
                    super::documents::page(),
                    super::security::page(self.site_input.clone()),
                    super::tools::page(),
                ]),
        )
    }
}

/// Settings → Conversation names: automatic names, what makes them, and the local models.
fn naming_page(cx: &App) -> SettingPage {
    use crate::naming::Namer;
    let settings = cx.global::<AppSettings>();
    let (enabled, namer) = (settings.auto_names, settings.namer);
    let local_missing = namer == Namer::Local && crate::naming::active_model(cx).is_none();
    SettingPage::new("Conversation names")
        .icon(AppIcon::Sparkles)
        .description(
            "Give each new conversation a short name made from its first message, instead of \
             the message's first line. The name is saved in the session, so the terminal pi \
             shows it too. Names you give a conversation yourself (Rename, or /name) are never \
             changed.",
        )
        .group(
            SettingGroup::new()
                .title("Automatic names")
                .item(
                    SettingItem::new(
                        "Name new conversations",
                        hover_switch(
                            "auto-names",
                            |cx| cx.global::<AppSettings>().auto_names,
                            |on, cx| AppSettings::update(cx, |s| s.auto_names = on),
                        ),
                    )
                    .description(
                        "The first message is sent right away; its name arrives a moment later \
                         and is typed out in the sidebar. Very short messages are their own \
                         name.",
                    ),
                )
                .item(
                    SettingItem::new(
                        "Made by",
                        SettingField::dropdown(
                            Namer::ALL
                                .into_iter()
                                .map(|namer| (namer.id().into(), namer.label().into()))
                                .collect(),
                            |cx| cx.global::<AppSettings>().namer.id().into(),
                            |value, cx| {
                                let namer = Namer::from_id(value.as_ref());
                                AppSettings::update(cx, |s| s.namer = namer);
                            },
                        ),
                    )
                    .disabled(!enabled)
                    .description(if local_missing {
                        "Download one of the models below first; until then new conversations \
                         keep their first line."
                    } else {
                        match namer {
                            Namer::Conversation => {
                                "One short extra request to the conversation's model, made by a \
                                 separate pi that keeps no session: the conversation, its \
                                 history and the model's context stay exactly as in the \
                                 terminal. On an SSH host it runs on the host."
                            }
                            Namer::Local => {
                                "A small model on this computer names conversations: nothing is \
                                 sent anywhere, and it is loaded only for the second or so it \
                                 takes."
                            }
                        }
                    }),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Models on this computer")
                .description(
                    "Used when names are made by a small model on this computer. Downloaded \
                     once from Hugging Face; nothing ships with the app.",
                )
                .item(SettingItem::render(|_, _, cx| {
                    crate::online::refresh(cx);
                    crate::naming::ui::settings_model_rows(cx)
                })),
        )
}

/// An on/off setting: gpui-kit's switch with a hover ring, so it reads as clickable.
pub(super) fn hover_switch(
    id: &'static str,
    value: impl Fn(&App) -> bool + 'static,
    set_value: impl Fn(bool, &mut App) + 'static,
) -> SettingField<SharedString> {
    let set_value = std::rc::Rc::new(set_value);
    SettingField::render(move |_, _, cx| {
        let set_value = set_value.clone();
        crate::ui::hover_switch(
            id,
            value(cx),
            move |checked, _, cx| set_value(checked, cx),
            None,
            cx,
        )
    })
}

/// The Theme menu in demo mode: the stock dropdown's look, with items the demo cursor can
/// point at (see [`crate::demo::menu_item`]).
fn demo_theme_field() -> SettingField<SharedString> {
    SettingField::render(|options: &RenderOptions, _: &mut Window, cx: &mut App| {
        const CHOICES: [(ThemeChoice, &str, &str); 3] = [
            (ThemeChoice::System, "system", "Follow system"),
            (ThemeChoice::Light, "light", "Light"),
            (ThemeChoice::Dark, "dark", "Dark"),
        ];
        let current = cx.global::<AppSettings>().theme;
        let index = CHOICES
            .iter()
            .position(|(choice, ..)| *choice == current)
            .unwrap_or_default();
        let button = Button::new(("demo-theme", index))
            .label(CHOICES[index].2)
            .dropdown_caret(true)
            .outline()
            .with_size(options.size())
            .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, _| {
                for (choice, value, label) in CHOICES {
                    menu = menu.item(
                        crate::demo::menu_item(label, format!("theme:{value}"))
                            .checked(choice == current)
                            .on_click(move |_, _, cx| {
                                AppSettings::update(cx, |s| s.theme = choice);
                            }),
                    );
                }
                menu
            });
        crate::demo::mark("theme-picker", button)
    })
}

/// The Code colors picker: a dropdown whose rows preview their palette while the pointer is on
/// them; picking one saves it.
fn code_colors_field(weak: WeakEntity<SettingsView>) -> SettingField<SharedString> {
    SettingField::render(
        move |options: &RenderOptions, _: &mut Window, cx: &mut App| {
            let current = cx.global::<AppSettings>().code_colors;
            let weak = weak.clone();
            Button::new("code-colors-picker")
                .label(current.label())
                .dropdown_caret(true)
                .outline()
                .with_size(options.size())
                .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, cx| {
                    // Back to the chosen palette when the menu closes, however it closes.
                    let on_dismiss = weak.clone();
                    cx.subscribe_self(move |_, _: &DismissEvent, cx| {
                        set_code_colors_hover(&on_dismiss, None, cx);
                    })
                    .detach();
                    for colors in CodeColors::ALL {
                        let (on_hover, on_click) = (weak.clone(), weak.clone());
                        menu = menu.item(
                            PopupMenuItem::element(move |_, _| {
                                let on_hover = on_hover.clone();
                                div()
                                    .id(SharedString::from(format!("code-colors-{}", colors.id())))
                                    .w_full()
                                    .on_hover(move |hovered, _, cx| {
                                        if *hovered {
                                            set_code_colors_hover(&on_hover, Some(colors), cx);
                                        }
                                    })
                                    .child(colors.label())
                            })
                            .checked(colors == current)
                            .on_click(move |_, _, cx| {
                                set_code_colors_hover(&on_click, None, cx);
                                AppSettings::update(cx, |s| s.code_colors = colors);
                            }),
                        );
                    }
                    menu
                })
        },
    )
}

/// The working-indicator picker: a menu of the styles; the preview below follows the pointer.
fn indicator_field(weak: WeakEntity<SettingsView>) -> SettingField<SharedString> {
    SettingField::render(
        move |options: &RenderOptions, _: &mut Window, cx: &mut App| {
            let current = cx.global::<AppSettings>().working_indicator;
            let weak = weak.clone();
            Button::new("indicator-picker")
                .label(current.label())
                .dropdown_caret(true)
                .outline()
                .with_size(options.size())
                .dropdown_menu_with_anchor(Anchor::TopRight, move |mut menu, _, cx| {
                    let on_dismiss = weak.clone();
                    cx.subscribe_self(move |_, _: &DismissEvent, cx| {
                        set_indicator_hover(&on_dismiss, None, cx);
                    })
                    .detach();
                    for style in IndicatorStyle::ALL {
                        let (on_hover, on_click) = (weak.clone(), weak.clone());
                        menu = menu.item(
                            PopupMenuItem::element(move |_, _| {
                                let on_hover = on_hover.clone();
                                div()
                                    .id(SharedString::from(format!("indicator-{}", style.id())))
                                    .w_full()
                                    .on_hover(move |hovered, _, cx| {
                                        if *hovered {
                                            set_indicator_hover(&on_hover, Some(style), cx);
                                        }
                                    })
                                    .child(style.label())
                            })
                            .checked(style == current)
                            .on_click(move |_, _, cx| {
                                set_indicator_hover(&on_click, None, cx);
                                AppSettings::update(cx, |s| s.working_indicator = style);
                            }),
                        );
                    }
                    menu
                })
        },
    )
}

fn set_indicator_hover(
    weak: &WeakEntity<SettingsView>,
    style: Option<IndicatorStyle>,
    cx: &mut App,
) {
    let _ = weak.update(cx, |view, cx| {
        if view.indicator_hover != style {
            view.indicator_hover = style;
            cx.notify();
        }
    });
}

/// The working indicator, live, as the chat shows it: the style under the pointer in the
/// picker, else the chosen one. It animates only while it is on screen: each draw marks it
/// drawn, and its repaint loop stops at the first tick it wasn't.
fn indicator_preview(weak: &WeakEntity<SettingsView>, cx: &mut App) -> AnyElement {
    let Some(view) = weak.upgrade() else {
        return div().into_any_element();
    };
    let (hovered, drawn, ticking) = {
        let view = view.read(cx);
        (
            view.indicator_hover,
            view.indicator_drawn.clone(),
            view.indicator_ticking.clone(),
        )
    };
    drawn.set(true);
    if !ticking.replace(true) {
        let weak = weak.clone();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor()
                    .timer(crate::ui::spinner::FRAME)
                    .await;
                if !drawn.replace(false) || weak.update(cx, |_, cx| cx.notify()).is_err() {
                    ticking.set(false);
                    break;
                }
            }
        })
        .detach();
    }
    let style = hovered.unwrap_or(cx.global::<AppSettings>().working_indicator);
    let theme = cx.theme();
    let accent = crate::theme::palette(cx).accent;
    h_flex()
        .w_full()
        .justify_center()
        .items_center()
        .gap_1p5()
        .py_4()
        .rounded_lg()
        .bg(theme.background)
        .text_sm()
        .child(crate::ui::spinner::working_indicator(
            style,
            px(13.),
            accent,
        ))
        .child(crate::ui::spinner::working_label(
            style,
            "Working…".into(),
            accent,
        ))
        .child(
            div()
                .text_color(theme.muted_foreground)
                .child("12s · esc to interrupt"),
        )
        .into_any_element()
}

fn set_code_colors_hover(
    weak: &WeakEntity<SettingsView>,
    colors: Option<CodeColors>,
    cx: &mut App,
) {
    let _ = weak.update(cx, |view, cx| {
        if view.code_colors_hover != colors {
            view.code_colors_hover = colors;
            cx.notify();
        }
    });
}

/// The Code colors preview: a Python snippet in `palette`, drawn like a chat code block but
/// without its language label.
fn code_colors_preview(palette: CodeColors, cx: &App) -> AnyElement {
    const SNIPPET: &str = r#"from dataclasses import dataclass

@dataclass
class Model:
    """A model entry from models.json."""
    id: str
    context_window: int = 131_072
    reasoning: bool = True

    def label(self, short: bool = False) -> str | None:
        name = self.id.split("/")[-1]  # drop the provider
        return f"{name} ({self.context_window // 1024}k)" if not short else None"#;
    let theme = cx.theme();
    let syntax = crate::theme::code_colors_syntax(palette, theme.mode.is_dark(), cx);
    div()
        .w_full()
        .px_4()
        .py_3()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .font_family(theme.mono_font_family.clone())
        .text_size(theme.mono_font_size)
        .child(StyledText::new(SNIPPET).with_highlights(highlight_python(SNIPPET, &syntax)))
        .into_any_element()
}

/// Syntax highlights for `code` (Python) in `syntax`'s colors.
fn highlight_python(
    code: &'static str,
    syntax: &component::highlighter::SyntaxColors,
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    use component::highlighter::SyntaxHighlighter;
    use component::input::Rope;

    struct Colors<'a>(&'a component::highlighter::SyntaxColors);

    impl base::input::HighlightStyleResolver for Colors<'_> {
        fn style(&self, name: &str) -> Option<HighlightStyle> {
            self.0.style(name)
        }
    }

    thread_local! {
        // Parsed once: the snippet never changes, only the colors.
        static PYTHON: std::cell::RefCell<Option<SyntaxHighlighter>> =
            const { std::cell::RefCell::new(None) };
    }
    PYTHON.with(|cell| {
        let mut cell = cell.borrow_mut();
        let highlighter = cell.get_or_insert_with(|| {
            let mut highlighter = SyntaxHighlighter::new("python");
            highlighter.update(None, &Rope::from(code), None);
            highlighter
        });
        highlighter.styles(&(0..code.len()), &Colors(syntax))
    })
}

fn options(pairs: &[(&'static str, &'static str)]) -> Vec<(SharedString, SharedString)> {
    pairs
        .iter()
        .map(|(value, label)| (SharedString::from(*value), SharedString::from(*label)))
        .collect()
}

fn backend_value(backend: BackendKind) -> &'static str {
    match backend {
        BackendKind::Duckduckgo => "duckduckgo",
        BackendKind::Searxng => "searxng",
        BackendKind::Brave => "brave",
        BackendKind::Tavily => "tavily",
    }
}

pub(super) fn web_config(cx: &App) -> &WebConfig {
    &cx.global::<WebSearchState>().config
}

/// Makes chats search with `backend`. SearXNG may have to be set up for that: [`offer_searxng`].
fn choose_service(backend: BackendKind, cx: &mut App) {
    let before = web_config(cx).backend;
    // Also when it was chosen before: it may not be set up.
    let choosing = (backend == BackendKind::Searxng).then_some(Choosing {
        before,
        asked: false,
    });
    cx.global_mut::<WebSearchState>().choosing = choosing;
    if backend == BackendKind::Searxng {
        // What answered the last time it was chosen may have changed.
        search_server(cx).update(cx, |server, _| server.forget_probe());
    }
    update_web(cx, |config| config.backend = backend);
}

/// Once it is known that the SearXNG just chosen cannot be used: uses the one that is installed
/// on this computer, or asks whether to download it.
fn offer_searxng(window: &mut Window, cx: &mut App) {
    let state = cx.global::<WebSearchState>();
    let Some(choosing) = &state.choosing else {
        return;
    };
    if choosing.asked || state.config.backend != BackendKind::Searxng {
        return;
    }
    let installed = search_server(cx).read(cx).installed() != Installed::No;
    if !local_search::is_local(cx) {
        match local_search::probed(cx) {
            None => return,
            Some(Probe::Ready) => return keep_service(cx),
            Some(_) => {}
        }
    }
    if searxng::catalog::platform().is_some() && installed {
        keep_service(cx);
        return local_search::use_local(cx);
    }
    if let Some(choosing) = &mut cx.global_mut::<WebSearchState>().choosing {
        choosing.asked = true;
    }
    if searxng::catalog::platform().is_some() {
        local_search::open_setup(window, cx);
    } else {
        local_search::open_address_dialog(window, cx);
    }
}

/// The SearXNG just chosen is set up.
pub(super) fn keep_service(cx: &mut App) {
    cx.global_mut::<WebSearchState>().choosing = None;
}

/// The user did not set up the SearXNG they just chose: the service from before searches again.
pub(super) fn go_back_to_previous_service(cx: &mut App) {
    let state = cx.global_mut::<WebSearchState>();
    if let Some(choosing) = state.choosing.take() {
        let missing = is_missing(cx);
        update_web(cx, |config| {
            config.backend = choosing.before;
            config.searxng.local &= !missing;
        });
    }
}

/// [`go_back_to_previous_service`], also when the page it was chosen on is gone.
fn download_cancelled(cx: &mut App) {
    go_back_to_previous_service(cx);
    if is_missing(cx) {
        update_web(cx, |config| {
            config.backend = BackendKind::Duckduckgo;
            config.searxng.local = false;
        });
    }
}

/// Chats are to search with the SearXNG on this computer, and there is none.
fn is_missing(cx: &App) -> bool {
    local_search::is_local(cx) && search_server(cx).read(cx).installed() == Installed::No
}

/// Checks the SearXNG that is used, now or after a change of the settings: whether the address
/// answers, or what state the one on this computer is in. That one stops when it is not used.
fn check_searxng(opened: bool, cx: &mut App) {
    let config = web_config(cx);
    let used = config.backend == BackendKind::Searxng;
    let (local, url) = (config.searxng.local, config.searxng.url.clone());
    search_server(cx).update(cx, |server, cx| {
        if opened {
            server.refresh(cx);
        }
        if used && !local {
            server.probe(&url, opened, cx);
        }
        if !(used && local) && matches!(server.running(), Running::Here(_)) {
            server.stop(cx);
        }
    });
}

fn startup_model(cx: &App) -> String {
    let defaults = &cx.global::<PiStartupState>().defaults;
    match (&defaults.provider, &defaults.model) {
        (Some(provider), Some(model)) => format!("{provider}/{model}"),
        _ => String::new(),
    }
}

fn load_web_search(cx: &mut App) {
    let dir = cx.global::<AppConfig>().deepsearch_dir.clone();
    let (config, error) = match deepsearch::config::load(&dir) {
        Ok((config, _)) => (config, None),
        Err(error) => (
            WebConfig::default(),
            Some(format!("{error}; changes are not saved.")),
        ),
    };
    let dir = error.is_none().then_some(dir);
    cx.set_global(WebSearchState {
        dir,
        config,
        error,
        choosing: None,
    });
}

/// Changes web search's settings and saves them (unless the file could not be read: then
/// saving would replace what the user wrote).
pub(super) fn update_web(cx: &mut App, f: impl FnOnce(&mut WebConfig)) {
    let state = cx.global_mut::<WebSearchState>();
    let before = state.config.clone();
    f(&mut state.config);
    if state.config == before {
        return;
    }
    let Some(dir) = state.dir.clone() else { return };
    let config = state.config.clone();
    let path = dir.join(deepsearch::config::CONFIG_FILE);
    crate::persist::save_in_background(cx, path, move |_| {
        deepsearch::config::save(&dir, &config).map_err(std::io::Error::other)
    });
}

fn pi_settings_path() -> Option<PathBuf> {
    pi_sessions::agent_dir().map(|agent| agent.join("settings.json"))
}

fn load_pi_startup(cx: &mut App) {
    let path = pi_settings_path();
    let text = path
        .as_deref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    let state = match pi_config::settings::startup_defaults(&text) {
        Ok(defaults) => PiStartupState {
            path,
            defaults,
            error: None,
        },
        Err(error) => PiStartupState {
            path: None,
            defaults: StartupDefaults::default(),
            error: Some(format!("pi's {error}; fix the file to change these here.")),
        },
    };
    cx.set_global(state);
}

/// Changes the startup defaults and writes them into pi's `settings.json`, keeping everything
/// else in the file (read again right before writing, in case pi changed it meanwhile).
fn update_pi_startup(cx: &mut App, f: impl FnOnce(&mut StartupDefaults)) {
    let state = cx.global_mut::<PiStartupState>();
    let before = state.defaults.clone();
    f(&mut state.defaults);
    if state.defaults == before {
        return;
    }
    let Some(path) = state.path.clone() else {
        return;
    };
    let defaults = state.defaults.clone();
    crate::persist::save_in_background(cx, path, move |path| write_pi_settings(path, &defaults));
}

fn write_pi_settings(path: &Path, defaults: &StartupDefaults) -> std::io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let updated = pi_config::settings::with_startup_defaults(&text, defaults)
        .map_err(std::io::Error::other)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.pistachio-tmp");
    std::fs::write(&tmp, updated)?;
    #[cfg(unix)]
    if let Ok(meta) = std::fs::metadata(path) {
        std::fs::set_permissions(&tmp, meta.permissions())?;
    }
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use deepsearch::config::BackendKind;
    use gpui_kit::component::Root;
    use gpui_kit::{AppContext as _, TestAppContext, VisualTestContext};

    use super::{
        AppConfig, AppSettings, Path, PathBuf, SettingsView, choose_service, update_web, web_config,
    };
    // Only for the tests of the SearXNG that can be downloaded.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use super::{download_cancelled, search_server};
    use crate::ui::dialog::testing as dialog;

    /// The Web search page, with its settings in `dir`.
    fn open<'a>(dir: &Path, cx: &'a mut TestAppContext) -> &'a mut VisualTestContext {
        open_with("/nonexistent/pi".into(), dir, cx)
    }

    fn open_with<'a>(
        pi: PathBuf,
        dir: &Path,
        cx: &'a mut TestAppContext,
    ) -> &'a mut VisualTestContext {
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_reduce_motion(true);
            let mut config = AppConfig::for_tests(pi);
            config.deepsearch_dir = dir.to_path_buf();
            cx.set_global(config);
            cx.set_global(AppSettings::default());
            crate::search_server::init(cx);
            crate::stt::init(cx);
            crate::naming::init(cx);
            let hosts = cx.new(|_| crate::hosts::Hosts::for_tests(Vec::new()));
            cx.set_global(crate::hosts::HostsHandle(hosts));
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                let mut view = SettingsView::new(Vec::new(), window, cx);
                view.start_on_web_search();
                view
            });
            Root::new(view, window, cx)
        });
        cx.run_until_parked();
        cx
    }

    /// Scrolling over a long list on the Sandboxing page moves the list, not the page.
    #[gpui_kit::test]
    fn a_long_list_scrolls_by_itself(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().expect("temp dir");
        let settings = sandbox::Settings {
            tool_folders: false,
            allowed: (0..8)
                .map(|n| sandbox::Allowed::new(format!("~/folder-{n}"), false))
                .collect(),
            ..sandbox::Settings::default()
        };
        settings.save(dir.path()).expect("save");
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::component::init(cx);
            cx.set_reduce_motion(true);
            let mut config = AppConfig::for_tests("/nonexistent/pi".into());
            config.deepsearch_dir = dir.path().to_path_buf();
            config.sandbox_dir = dir.path().to_path_buf();
            cx.set_global(config);
            cx.set_global(AppSettings::default());
            crate::search_server::init(cx);
            crate::stt::init(cx);
            crate::naming::init(cx);
            let hosts = cx.new(|_| crate::hosts::Hosts::for_tests(Vec::new()));
            cx.set_global(crate::hosts::HostsHandle(hosts));
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| {
                let mut view = SettingsView::new(Vec::new(), window, cx);
                view.start_on_security();
                view
            });
            Root::new(view, window, cx)
        });
        // Short enough that the page scrolls too, tall enough to show the whole list.
        cx.simulate_resize(gpui_kit::size(gpui_kit::px(1100.), gpui_kit::px(950.)));
        cx.run_until_parked();
        let bounds = cx
            .debug_bounds("sandbox-allowed")
            .expect("the list is drawn");
        let over = gpui_kit::point(bounds.center().x, bounds.bottom() - gpui_kit::px(80.));
        let offset = |cx: &mut VisualTestContext| {
            cx.update(|_, cx| {
                crate::security::existing(cx)
                    .and_then(|s| s.read(cx).list_offsets.get("sandbox-allowed").copied())
                    .unwrap_or(0.)
            })
        };
        for _ in 0..3 {
            cx.simulate_event(gpui_kit::ScrollEvent {
                position: over,
                delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                    gpui_kit::px(0.),
                    gpui_kit::px(-40.),
                )),
                ..Default::default()
            });
            cx.run_until_parked();
        }
        assert!(
            (offset(cx) - 120.).abs() < 0.01,
            "scrolling down moved the list by as much"
        );
        let after = cx
            .debug_bounds("sandbox-allowed")
            .expect("the list is drawn");
        assert_eq!(after.origin, bounds.origin, "the page stayed where it was");
    }

    /// Every model pi has can be the startup model, also when there is no session.
    #[cfg(unix)]
    #[gpui_kit::test]
    fn the_startup_models_do_not_need_a_session(cx: &mut TestAppContext) {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let pi = dir.path().join("pi");
        std::fs::write(
            &pi,
            "#!/bin/sh\n[ \"$1\" = --list-models ] || exit 1\n\
             echo 'provider  model    context  max-out  thinking  images'\n\
             echo 'test-box  model-a  262.1K   32.8K    yes       yes   '\n\
             echo 'test-box  model-b  1.0M     65.5K    yes       yes   '\n",
        )
        .unwrap();
        std::fs::set_permissions(&pi, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cx = open_with(pi, dir.path(), cx);
        let values: Vec<String> = cx.update(|window, cx| {
            let root = window.root::<Root>().flatten().unwrap();
            let view = root.read(cx).view().clone();
            let view = view.downcast::<SettingsView>().unwrap();
            let models = &view.read(cx).models;
            models.iter().map(|(value, _)| value.to_string()).collect()
        });
        assert_eq!(values, ["test-box/model-a", "test-box/model-b"]);
    }

    /// Choosing a search service shows what it needs, and only that, right away; the others
    /// keep what was entered.
    #[gpui_kit::test]
    fn only_the_chosen_search_service_is_shown(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let cx = open(dir.path(), cx);
        for (backend, shown) in [
            (BackendKind::Duckduckgo, (false, false)),
            (BackendKind::Brave, (true, false)),
            (BackendKind::Tavily, (false, true)),
            (BackendKind::Searxng, (false, false)),
            (BackendKind::Brave, (true, false)),
        ] {
            cx.update(|_, cx| {
                update_web(cx, |config| {
                    config.backend = backend;
                    config.searxng.url = "http://127.0.0.1:9".into();
                    config.brave.api_key.get_or_insert_with(|| "key".into());
                });
            });
            cx.run_until_parked();
            let visible = ["brave-key", "tavily-key"].map(|key| cx.debug_bounds(key).is_some());
            assert_eq!((visible[0], visible[1]), shown, "{backend:?}");
        }
        let key = cx.read(|cx| web_config(cx).brave.api_key.clone());
        assert_eq!(key.as_deref(), Some("key"));
    }

    /// The advanced sections are closed until their title is clicked, which stays where it is.
    #[gpui_kit::test]
    fn the_advanced_sections_open_and_close(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let cx = open(dir.path(), cx);
        for (title, inside) in [
            ("web-search-options", "text-per-page"),
            ("deep-search-options", "deep-searches-standard"),
        ] {
            let place = cx.debug_bounds(title).unwrap();
            for open in [true, false] {
                let row = cx.debug_bounds(title).unwrap();
                cx.simulate_click(row.center(), gpui_kit::Modifiers::default());
                dialog::draw(cx);
                assert_eq!(cx.debug_bounds(inside).is_some(), open, "{inside}");
                assert_eq!(cx.debug_bounds(title).unwrap(), place, "{title}");
            }
        }
    }

    /// A slider saves where it is let go.
    #[gpui_kit::test]
    fn a_slider_sets_the_text_per_page(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let cx = open(dir.path(), cx);
        let title = cx.debug_bounds("web-search-options").unwrap();
        cx.simulate_click(title.center(), gpui_kit::Modifiers::default());
        dialog::draw(cx);
        let chars = |cx: &mut VisualTestContext| cx.read(|cx| web_config(cx).fetch.max_chars);
        assert_eq!(chars(cx), 12_000);
        let slider = cx.debug_bounds("text-per-page").unwrap();
        // The slider's middle: the value is beside it.
        let at = gpui_kit::point(slider.left() + gpui_kit::px(95.), slider.center().y);
        cx.simulate_click(at, gpui_kit::Modifiers::default());
        dialog::draw(cx);
        let set = chars(cx);
        assert!((45_000..=55_000).contains(&set), "{set}");
        let (saved, _) = deepsearch::config::load(dir.path()).unwrap();
        assert_eq!(saved.fetch.max_chars, set);
    }

    /// Addresses where a SearXNG answers, and where nothing does.
    fn addresses() -> (String, String) {
        use std::io::{Read as _, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let running = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let _ = stream.read(&mut [0; 2048]);
                let body = r#"{"error": "No query"}"#;
                let _ = write!(
                    stream,
                    "HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        (running, format!("http://{}", free.local_addr().unwrap()))
    }

    /// Chooses SearXNG, with `url` as its address, after Brave.
    fn choose_searxng(url: &str, cx: &mut VisualTestContext) {
        cx.update(|_, cx| {
            update_web(cx, |config| {
                config.backend = BackendKind::Brave;
                config.searxng.local = false;
                config.searxng.url = url.into();
            });
            choose_service(BackendKind::Searxng, cx);
        });
        dialog::draw(cx);
    }

    fn service(cx: &mut VisualTestContext) -> (BackendKind, bool) {
        cx.read(|cx| (web_config(cx).backend, web_config(cx).searxng.local))
    }

    #[gpui_kit::test]
    fn a_searxng_that_answers_is_used(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let cx = open(dir.path(), cx);
        let (running, nothing) = addresses();
        choose_searxng(&running, cx);
        assert!(!dialog::is_open(cx));
        assert!(cx.debug_bounds("searxng-on").is_some());
        assert!(cx.debug_bounds("searxng-menu").is_some());
        assert_eq!(service(cx), (BackendKind::Searxng, false));

        // Losing it later shows, but does not interrupt.
        cx.update(|_, cx| update_web(cx, |config| config.searxng.url = nothing));
        dialog::draw(cx);
        assert!(!dialog::is_open(cx));
        assert!(cx.debug_bounds("searxng-on").is_none());
        assert!(cx.debug_bounds("searxng-off").is_some());
    }

    /// Without one the user is asked whether to download it, and Cancel and Esc go back to the
    /// service from before.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[gpui_kit::test]
    fn a_missing_searxng_is_offered(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let cx = open(dir.path(), cx);
        let (running, nothing) = addresses();
        for cancel in [true, false] {
            choose_searxng(&nothing, cx);
            assert!(dialog::is_open(cx));
            assert_eq!(service(cx), (BackendKind::Searxng, false));
            if cancel {
                dialog::click("searxng-setup-cancel", cx);
            } else {
                dialog::press("escape", cx);
            }
            assert!(!dialog::is_open(cx));
            assert_eq!(service(cx), (BackendKind::Brave, false));
        }

        // Choosing it again asks again.
        cx.update(|_, cx| update_web(cx, |config| config.backend = BackendKind::Searxng));
        dialog::draw(cx);
        assert!(!dialog::is_open(cx));
        cx.update(|_, cx| choose_service(BackendKind::Searxng, cx));
        dialog::draw(cx);
        assert!(dialog::is_open(cx));
        dialog::click("searxng-setup-cancel", cx);
        assert_eq!(service(cx), (BackendKind::Searxng, false));

        // Another address instead.
        choose_searxng(&nothing, cx);
        dialog::click("searxng-setup-address", cx);
        assert!(dialog::is_open(cx));
        dialog::click("searxng-address-cancel", cx);
        assert_eq!(service(cx), (BackendKind::Brave, false));
        choose_searxng(&nothing, cx);
        dialog::click("searxng-setup-address", cx);
        cx.simulate_keystrokes("secondary-a backspace");
        // Without "http://", which is added.
        cx.simulate_input(running.trim_start_matches("http://"));
        dialog::press("enter", cx);
        assert!(!dialog::is_open(cx));
        assert_eq!(service(cx), (BackendKind::Searxng, false));
        assert_eq!(cx.read(|cx| web_config(cx).searxng.url.clone()), running);
        assert!(cx.debug_bounds("searxng-on").is_some());
    }

    /// The one that was downloaded before is used without asking; a cancelled download leaves
    /// nothing chosen that is not there.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[gpui_kit::test]
    fn an_installed_searxng_is_used(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let cx = open(dir.path(), cx);
        let (_, nothing) = addresses();
        cx.update(|_, cx| {
            update_web(cx, |config| {
                config.backend = BackendKind::Searxng;
                config.searxng.local = true;
            });
            download_cancelled(cx);
        });
        assert_eq!(service(cx), (BackendKind::Duckduckgo, false));

        let app = dir.path().join("searxng/app");
        std::fs::create_dir_all(app.join("python/bin")).unwrap();
        std::fs::write(app.join("python/bin/python3"), "").unwrap();
        std::fs::write(app.join(".installed"), "older").unwrap();
        cx.update(|_, cx| {
            search_server(cx).update(cx, crate::search_server::SearchServer::refresh);
        });
        choose_searxng(&nothing, cx);
        assert!(!dialog::is_open(cx));
        assert_eq!(service(cx), (BackendKind::Searxng, true));
        // This one cannot start: the light is red, and the row says why.
        assert!(cx.debug_bounds("searxng-off").is_some());
        cx.update(|_, cx| {
            let server = search_server(cx).read(cx);
            assert!(server.error().unwrap().contains("could not start"));
        });

        // "⋮" has Start, Update, Change address… and Remove….
        let choose = |downs: usize, cx: &mut VisualTestContext| {
            let menu = cx.debug_bounds("searxng-menu").unwrap();
            cx.simulate_click(menu.center(), gpui_kit::Modifiers::default());
            dialog::draw(cx);
            cx.simulate_keystrokes(&vec!["down"; downs].join(" "));
            cx.simulate_keystrokes("enter");
            dialog::draw(cx);
        };
        choose(3, cx);
        assert!(dialog::is_open(cx));
        dialog::click("searxng-address-cancel", cx);
        assert_eq!(service(cx), (BackendKind::Searxng, true));
        choose(4, cx);
        assert!(dialog::is_open(cx));
        dialog::click("searxng-remove", cx);
        assert!(!dialog::is_open(cx));
        assert_eq!(service(cx), (BackendKind::Duckduckgo, false));
        assert!(!dir.path().join("searxng").exists());
    }
}
