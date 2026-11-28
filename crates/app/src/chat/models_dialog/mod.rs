//! The Models dialog: connects pi to models, and changes or removes what is connected.
//!
//! It starts with two choices: a model hosted on one's own devices ([`hosted_screen`]: on this
//! device or another one, where known servers are looked for and any server can be connected by its
//! address), or one of a provider pi knows, which needs no more than a key ([`key_screen`]). A
//! server is asked what it offers ([`probe`]), and models are ticked in its list
//! ([`server_screen`]). What is connected can be opened from the first screen, and `models.json`
//! can also be edited as text.
//!
//! The file is the one the session's pi reads: in pi's agent folder on this computer (also for
//! a remote session, whose pi uses this computer's models; see [`crate::relay`]), or on the
//! SSH host when pi there uses the host's own models. pi only reads it when it starts, so
//! after saving the window restarts pi on the same session and selects the new model.

mod hosted_screen;
mod key_screen;
mod probe;
mod server_screen;
mod start_screen;

use std::collections::HashMap;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Editor, EditorState, Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_config::providers::BuiltIn;
use pi_config::{ModelsFile, ProviderInfo};
use remote::HostId;

pub use probe::answers;
pub use start_screen::describe;

use self::hosted_screen::{DeployStep, Device, Scan, Way};
use self::key_screen::KeyScreen;
use self::server_screen::ServerScreen;
use self::start_screen::Back;
use crate::pi_place::PiPlace;
use crate::ui::{FadingText, QuietButton as _};

#[derive(Debug, Clone)]
pub enum ModelsDialogEvent {
    /// `models.json` was saved; restart pi and select `select` (`(provider, model id)`). `note` is
    /// what to tell the user.
    Saved {
        select: Option<(String, String)>,
        note: Option<SharedString>,
    },
    /// Sign in to a provider with pi's `/login`.
    SignIn(String),
}

/// What the dialog starts with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Open {
    #[default]
    Start,
    /// A provider in models.json, or one of pi's own.
    Provider(String),
    /// Connects to a server.
    Server(String),
    Json,
}

/// The models.json pi reads, where pi runs.
#[derive(Clone)]
struct Place {
    pi: PiPlace,
    path: String,
}

impl Place {
    fn describe(&self, home: Option<&std::path::Path>) -> String {
        self.pi.describe(&self.path, home)
    }

    fn read(&self) -> Result<String, String> {
        self.pi.read(&self.path).map(Option::unwrap_or_default)
    }

    /// Writes atomically, keeping the previous file as `models.json.bak-pistachio`.
    fn write(&self, text: &str) -> Result<(), String> {
        self.pi.write(&self.path, text, true)
    }

    /// The key in an `apiKey` value, to ask a server with. pi looks up `!command` itself, and
    /// environment variables too where they are not this app's.
    fn key(&self, value: &str) -> Option<String> {
        if let Some(key) = pi_config::literal_key(value) {
            return Some(key.to_owned());
        }
        let name = value.trim().strip_prefix('$')?;
        let name = name
            .strip_prefix('{')
            .and_then(|name| name.strip_suffix('}'))
            .unwrap_or(name);
        match self.pi {
            PiPlace::Local { .. } => std::env::var(name).ok().filter(|key| !key.is_empty()),
            PiPlace::Remote { .. } => None,
        }
    }
}

enum Screen {
    Start,
    /// Locally hosted: on which device.
    Hosted,
    /// API providers.
    Providers,
    Key(KeyScreen),
    Server(Box<ServerScreen>),
    Json,
}

pub struct ModelsDialog {
    /// The window it is in, for answers that come later.
    window: AnyWindowHandle,
    place: Option<Place>,
    loaded: Option<Result<ModelsFile, SharedString>>,
    providers: Vec<ProviderInfo>,
    /// How many models pi has of each provider, as far as the visible session knows.
    available: HashMap<String, usize>,
    /// The visible session has a terminal for `/login`.
    can_sign_in: bool,
    /// What to show once the file is loaded.
    open: Open,
    screen: Screen,
    /// Where the screen with one server or provider goes back to.
    back_to: Back,
    /// Where a locally hosted server runs, once chosen.
    device: Option<Device>,
    /// On this device: attach to a server that runs, or deploy a new model.
    this_way: Option<Way>,
    /// How far deploying a new model on this device got.
    deploy: DeployStep,
    /// Hardware scans asked for, so each one's progress starts from the beginning.
    scans: usize,
    /// The suggestions opened to show how they run (by tier).
    expanded: Vec<&'static str>,
    /// The known servers on this device, once it was chosen.
    this_device: Option<Scan>,
    /// The known servers on the other device last asked for.
    other_device: Option<Scan>,
    /// The address of another device or server.
    address: Entity<InputState>,
    /// The port of another server on this device.
    port: Entity<InputState>,
    address_error: Option<SharedString>,
    editor: Entity<EditorState>,
    json_error: Option<SharedString>,
    saving: bool,
    /// Checking how servers take thinking levels, before saving.
    checking_thinking: bool,
    /// Counts what is asked of servers, so an answer that comes too late is left out.
    asked: usize,
    /// What is being asked of servers; a new question replaces it.
    task: Option<Task<()>>,
    /// Repaints while the hardware scan or a download shows its progress.
    ticking: Option<Task<()>>,
    _tasks: Vec<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ModelsDialogEvent> for ModelsDialog {}

impl ModelsDialog {
    /// The dialog for the `models.json` that pi on `host` (or this computer) reads.
    pub fn new(
        host: Option<HostId>,
        open: Open,
        available: HashMap<String, usize>,
        can_sign_in: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let place = PiPlace::for_host(host.as_ref(), cx).map(|pi| Place {
            path: pi.agent_path("models.json"),
            pi,
        });
        let address = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("192.168.1.89, 192.168.1.89:8888 or https://example.com/v1")
        });
        let port = cx.new(|cx| InputState::new(window, cx).placeholder("Port, like 8000"));
        let enter = cx.subscribe_in(
            &address,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.connect_to_address(window, cx),
                InputEvent::Change => {
                    this.address_error = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        let enter_port = cx.subscribe_in(
            &port,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.connect_to_port(window, cx),
                InputEvent::Change => {
                    this.address_error = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        let mut subscriptions = vec![enter, enter_port];
        if let Some(models) = crate::local_models::local_models(cx) {
            subscriptions.push(cx.observe(&models, |_, _, cx| cx.notify()));
            // A suggested model that started is connected like any server.
            subscriptions.push(cx.subscribe_in(
                &models,
                window,
                |this, _, event: &crate::local_models::LocalModelsEvent, window, cx| {
                    if let crate::local_models::LocalModelsEvent::Ready { url } = event
                        && matches!(this.screen, Screen::Hosted)
                    {
                        this.back_to = Back::Hosted;
                        this.open_new_server(vec![url.clone()], window, cx);
                    }
                },
            ));
        }
        let mut this = Self {
            window: window.window_handle(),
            loaded: None,
            providers: Vec::new(),
            available,
            can_sign_in,
            screen: if open == Open::Json {
                Screen::Json
            } else {
                Screen::Start
            },
            open,
            back_to: Back::Start,
            device: None,
            this_way: None,
            deploy: DeployStep::Start,
            scans: 0,
            expanded: Vec::new(),
            this_device: None,
            other_device: None,
            address,
            port,
            address_error: None,
            editor: cx.new(|cx| EditorState::new(window, cx).language("json")),
            json_error: None,
            saving: false,
            checking_thinking: false,
            asked: 0,
            task: None,
            ticking: None,
            _tasks: Vec::new(),
            _subscriptions: subscriptions,
            place,
        };
        // Servers are only looked for once a device is chosen: none is assumed to run anywhere.
        this.load(window, cx);
        this
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(place) = self.place.clone() else {
            self.loaded = Some(Err(
                "Connect to the host first: pi's settings there are not known yet.".into(),
            ));
            return;
        };
        self.loaded = None;
        let task = cx.spawn_in(window, async move |this, cx| {
            let text = cx
                .background_executor()
                .spawn(async move { place.read() })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                match text {
                    Ok(text) => {
                        this.editor.update(cx, |editor, cx| {
                            editor.set_value(text.clone(), window, cx);
                        });
                        match ModelsFile::parse(&text) {
                            Ok(file) => this.file_loaded(file, window, cx),
                            Err(error) => {
                                // Still editable as text.
                                this.loaded = Some(Err(format!(
                                    "models.json has a problem; fix it in its text. {error}"
                                )
                                .into()));
                                this.screen = Screen::Json;
                                this.json_error = Some(error.to_string().into());
                            }
                        }
                    }
                    Err(error) => this.loaded = Some(Err(error.into())),
                }
                cx.notify();
            });
        });
        self._tasks.push(task);
    }

    fn file_loaded(&mut self, file: ModelsFile, window: &mut Window, cx: &mut Context<Self>) {
        self.providers = file.providers();
        self.loaded = Some(Ok(file));
        match std::mem::take(&mut self.open) {
            Open::Start | Open::Json => {}
            Open::Provider(id) => {
                self.back_to = Back::Start;
                self.open_provider(&id, window, cx);
            }
            Open::Server(address) => {
                self.screen = Screen::Hosted;
                self.device = Some(Device::Other);
                self.address
                    .update(cx, |input, cx| input.set_value(address, window, cx));
                self.connect_to_address(window, cx);
            }
        }
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        // The pages of a locally hosted model: attaching or deploying, then this device or
        // another, then the device's choice.
        if matches!(self.screen, Screen::Hosted) && self.device.is_some() {
            if self.device == Some(Device::This)
                && self.this_way.is_some()
                && self.suggests_models()
            {
                self.this_way = None;
            } else {
                self.device = None;
                self.this_way = None;
            }
            self.address_error = None;
            cx.notify();
            return;
        }
        let screen = match (&self.screen, self.back_to) {
            (Screen::Key(_) | Screen::Server(_), Back::Hosted) => Screen::Hosted,
            (Screen::Key(_) | Screen::Server(_), Back::Providers) => Screen::Providers,
            _ => Screen::Start,
        };
        self.stop_asking();
        self.show(screen, cx);
    }

    fn stop_asking(&mut self) {
        self.asked += 1;
        self.task = None;
    }

    /// Opens a provider in models.json, or one of pi's own.
    fn open_provider(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let saved = self.providers.iter().find(|provider| provider.id == id);
        let built_in = match saved {
            Some(saved) => saved.built_in(),
            None => pi_config::providers::built_in(id),
        };
        if let Some(provider) = built_in {
            self.open_key(provider, window, cx);
        } else if let Some(saved) = saved.cloned() {
            self.open_saved_server(saved, window, cx);
        }
    }

    fn open_key(
        &mut self,
        provider: &'static BuiltIn,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stop_asking();
        let key = self
            .providers
            .iter()
            .find(|saved| saved.id == provider.id)
            .and_then(|saved| saved.api_key.clone());
        self.screen = Screen::Key(KeyScreen::new(provider, key, window, cx));
        cx.notify();
    }

    /// Writes the file and tells the window.
    fn save(
        &mut self,
        text: String,
        select: Option<(String, String)>,
        note: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let Some(place) = self.place.clone() else {
            return;
        };
        self.saving = true;
        self.set_error(None);
        cx.notify();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { place.write(&text) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.saving = false;
                match result {
                    Ok(()) => cx.emit(ModelsDialogEvent::Saved { select, note }),
                    Err(error) => {
                        this.set_error(Some(error.into()));
                        cx.notify();
                    }
                }
            });
        });
        self._tasks.push(task);
    }

    /// What went wrong on the screen that shows.
    fn set_error(&mut self, error: Option<SharedString>) {
        match &mut self.screen {
            Screen::Start | Screen::Hosted | Screen::Providers => self.address_error = error,
            Screen::Key(key) => key.error = error,
            Screen::Server(server) => server.error = error,
            Screen::Json => self.json_error = error,
        }
    }

    fn save_json(&mut self, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).value().to_string();
        match ModelsFile::parse(&text) {
            Ok(_) => {
                let text = if text.ends_with('\n') {
                    text
                } else {
                    format!("{text}\n")
                };
                self.save(text, None, None, cx);
            }
            Err(error) => {
                self.json_error = Some(error.to_string().into());
                cx.notify();
            }
        }
    }

    fn render_json(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let error_color = crate::theme::palette(cx).error;
        let home = cx.global::<crate::config::AppConfig>().home.clone();
        let place = self
            .place
            .as_ref()
            .map_or_default(|place| place.describe(home.as_deref()));
        v_flex()
            .gap_2()
            .child(Self::render_top(
                "models.json",
                Some(
                    div()
                        .min_w_0()
                        .text_xs()
                        .text_color(muted)
                        .child(FadingText::new(place))
                        .into_any_element(),
                ),
                cx,
            ))
            .child(
                div()
                    .h(px(400.))
                    .rounded_md()
                    .border_1()
                    .border_color(border)
                    .overflow_hidden()
                    .child(Editor::new(&self.editor).size_full()),
            )
            .child(div().text_xs().text_color(muted).child(
                "Format: {\"providers\": {\"<id>\": {\"baseUrl\", \"api\", \"apiKey\", \"models\": [{\"id\"}]}}}. \
                 See pi's docs/models.md. The previous file is kept as models.json.bak-pistachio.",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .when_some(self.json_error.clone(), |this, error| {
                        this.child(div().flex_1().text_xs().text_color(error_color).child(error))
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("revert-json")
                            .quiet(cx)
                            .label("Revert")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.json_error = None;
                                this.load(window, cx);
                            })),
                    )
                    .child(
                        Button::new("save-json")
                            .primary()
                            .label(if self.saving { "Saving…" } else { "Save" })
                            .disabled(self.saving)
                            .on_click(cx.listener(|this, _, _, cx| this.save_json(cx))),
                    ),
            )
            .into_any_element()
    }

    /// The top of a screen that is not the first: back, what it is about and how that is.
    fn render_top(
        title: impl Into<SharedString>,
        detail: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> Div {
        h_flex()
            .w_full()
            .gap_2()
            .child(
                div().debug_selector(|| "models-back".into()).child(
                    Button::new("models-back")
                        .quiet(cx)
                        .small()
                        .icon(IconName::ArrowLeft)
                        .accessibility_label("Back")
                        .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::MEDIUM)
                    .child(FadingText::new(title)),
            )
            .children(detail)
    }
}

impl Render for ModelsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match (&self.loaded, &self.screen) {
            (None, _) => div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Reading models.json…")
                .into_any_element(),
            (Some(_), Screen::Json) if self.place.is_some() => self.render_json(cx),
            (Some(Err(error)), _) => div()
                .text_sm()
                .text_color(crate::theme::palette(cx).error)
                .child(error.clone())
                .into_any_element(),
            (Some(Ok(_)), Screen::Start) => self.render_start(cx),
            (Some(Ok(_)), Screen::Hosted) => self.render_hosted(cx),
            (Some(Ok(_)), Screen::Providers) => self.render_providers(cx),
            (Some(Ok(_)), Screen::Key(key)) => self.render_key(key, cx),
            (Some(Ok(_)), Screen::Server(server)) => self.render_server(server, cx),
            (Some(Ok(_)), Screen::Json) => self.render_json(cx),
        };
        self.keep_moving(cx);
        v_flex()
            .w_full()
            // Text painted by itself ([`FadingText`]) has no color in a dialog otherwise.
            .text_color(cx.theme().foreground)
            .child(body)
    }
}

/// A small label above something.
fn label(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// A labeled field.
fn field(name: &'static str, input: &Entity<InputState>, cx: &App) -> Div {
    v_flex()
        .flex_1()
        .min_w_0()
        .gap_1()
        .child(label(name, cx))
        .child(Input::new(input).aria_label(name))
}

/// The bottom of a screen: what is wrong, and the buttons.
fn bottom(error: Option<&SharedString>, cx: &App) -> Div {
    h_flex().w_full().pt_1().gap_2().child(
        div()
            .flex_1()
            .min_w_0()
            .text_xs()
            .text_color(crate::theme::palette(cx).error)
            .children(error.cloned()),
    )
}

/// Something to choose: what it is, more about it, and how it is at the end.
fn card(
    id: String,
    icon: impl Into<Icon>,
    title: impl Into<SharedString>,
    detail: SharedString,
    end: Option<AnyElement>,
    chosen: bool,
    cx: &App,
) -> Button {
    let muted = cx.theme().muted_foreground;
    let title = title.into();
    Button::new(SharedString::from(id))
        .choice_card(chosen, cx)
        .h(px(40.))
        .accessibility_label(if detail.is_empty() {
            title.to_string()
        } else {
            format!("{title}, {detail}")
        })
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .gap_2p5()
                .child(icon.into().small().text_color(muted))
                .child(div().min_w_0().child(FadingText::new(title)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(muted)
                        .child(FadingText::new(detail)),
                )
                .children(end),
        )
}

/// `12345` as `12,345`.
fn grouped(number: u64) -> String {
    let digits = number.to_string();
    let mut text = String::with_capacity(digits.len() + digits.len() / 3);
    for (ix, digit) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

/// "1 model", "2 models".
fn count(number: usize, what: &str) -> String {
    format!(
        "{} {what}{}",
        grouped(number as u64),
        if number == 1 { "" } else { "s" }
    )
}

#[cfg(test)]
mod tests;
