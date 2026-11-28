//! Locally hosted models: on this device or on another one. Nothing is assumed to run anywhere:
//! once a device is chosen, Ollama, LM Studio and the other known servers are looked for on their
//! usual ports there, and any server can also be connected by its address or port.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pi_config::server::{self, KNOWN_SERVERS, Kind, KnownServer, Lookup, THIS_DEVICE};

use super::start_screen::{Back, available, grid};
use super::{ModelsDialog, card, label, probe};
use crate::local_models::{LocalModels, Scan as Machine, Status};
use crate::model_store::size_label;
use crate::pi_place::PiPlace;
use crate::ui::download::{download_and_run_button, progress_bar};
use crate::ui::spinner::{FRAME, turning_loader};
use crate::ui::{AppIcon, FadingText, QuietButton as _, web_link};
use local_models::hardware::Fact;
use local_models::{Choice, Suggestion};

/// How far deploying a new model on this device got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DeployStep {
    Start,
    /// The hardware scan was asked for, so what it found shows.
    Hardware,
    /// Models are suggested for the hardware found.
    Suggestions,
}

/// Where the server runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Device {
    This,
    Other,
}

/// What to do on this device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Way {
    /// Connect to a server that runs here already.
    Attach,
    /// Download a model suggested for this device's hardware, and run it.
    Deploy,
}

/// Whether a known server answers on its usual port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Found {
    Checking,
    Missing,
    /// `models` is `None` when it wants a key first.
    Running {
        kind: Kind,
        models: Option<usize>,
    },
}

/// The known servers on a device, as far as they answer.
pub(super) struct Scan {
    pub host: String,
    pub servers: Vec<(KnownServer, Found)>,
    /// How to start the server last clicked that does not answer.
    pub hint: Option<SharedString>,
}

impl Scan {
    fn nothing(&self) -> bool {
        self.servers
            .iter()
            .all(|(_, found)| *found == Found::Missing)
    }
}

impl ModelsDialog {
    pub(super) fn choose_device(&mut self, device: Device, cx: &mut Context<Self>) {
        self.device = Some(device);
        self.address_error = None;
        if device == Device::This {
            self.this_way = None;
            // Nothing to deploy where pi does not run on this computer.
            if !self.suggests_models() {
                self.choose_way(Way::Attach, cx);
            }
        }
        cx.notify();
    }

    fn choose_way(&mut self, way: Way, cx: &mut Context<Self>) {
        self.this_way = Some(way);
        self.address_error = None;
        if way == Way::Attach && self.this_device.is_none() {
            self.scan(Device::This, THIS_DEVICE.to_owned(), cx);
        }
        cx.notify();
    }

    /// Looks at this computer's hardware (again); models are suggested once asked for.
    fn scan_hardware(&mut self, cx: &mut Context<Self>) {
        self.deploy = DeployStep::Hardware;
        self.scans += 1;
        if let Some(models) = crate::local_models::local_models(cx) {
            models.update(cx, LocalModels::scan_now);
        }
        cx.notify();
    }

    /// Whether something on the page moves: the hardware scan, or a suggestion downloading.
    fn moving(&self, cx: &App) -> bool {
        let on_page = matches!(self.screen, super::Screen::Hosted)
            && self.device == Some(Device::This)
            && self.this_way == Some(Way::Deploy);
        let Some(models) = crate::local_models::local_models(cx).filter(|_| on_page) else {
            return false;
        };
        let models = models.read(cx);
        let scanning =
            self.deploy != DeployStep::Start && matches!(models.scan(), Machine::Scanning { .. });
        scanning || models.is_busy()
    }

    /// Repaints every [`FRAME`] while something on the page moves, and not otherwise. Driven by
    /// the clock rather than GPUI's animations, which stand still with reduced motion: these
    /// are waits the user follows.
    pub(super) fn keep_moving(&mut self, cx: &mut Context<Self>) {
        if self.ticking.is_some() || !self.moving(cx) {
            return;
        }
        self.ticking = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(FRAME).await;
                let moving = this.update(cx, |this, cx| {
                    let moving = this.moving(cx);
                    if moving {
                        cx.notify();
                    } else {
                        this.ticking = None;
                    }
                    moving
                });
                if !moving.unwrap_or(false) {
                    break;
                }
            }
        }));
    }

    fn scan_of(&mut self, device: Device) -> &mut Option<Scan> {
        match device {
            Device::This => &mut self.this_device,
            Device::Other => &mut self.other_device,
        }
    }

    /// Looks for the known servers on `host`, each on its usual port.
    fn scan(&mut self, device: Device, host: String, cx: &mut Context<Self>) {
        let Some(place) = self.place.clone() else {
            return;
        };
        let slot = self.scan_of(device);
        let hint = slot
            .take()
            .filter(|scan| scan.host == host)
            .and_then(|scan| scan.hint);
        *slot = Some(Scan {
            host: host.clone(),
            servers: KNOWN_SERVERS
                .iter()
                .map(|server| (*server, Found::Checking))
                .collect(),
            hint,
        });
        // An SSH server allows only so many commands at once (usually 10).
        let at_once = match place.pi {
            PiPlace::Local { .. } => KNOWN_SERVERS.len(),
            PiPlace::Remote { .. } => 4,
        };
        for first in 0..at_once {
            let (pi, host) = (place.pi.clone(), host.clone());
            let task = cx.spawn(async move |this, cx| {
                for (ix, server) in KNOWN_SERVERS
                    .iter()
                    .enumerate()
                    .skip(first)
                    .step_by(at_once)
                {
                    let found = cx
                        .background_executor()
                        .spawn({
                            let (pi, host) = (pi.clone(), host.clone());
                            async move { probe::find(&pi, server, &host) }
                        })
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        // Another device may have been asked for since.
                        if let Some(scan) = this.scan_of(device).as_mut()
                            && scan.host == host
                        {
                            scan.servers[ix].1 = match found {
                                Some((kind, models)) => Found::Running { kind, models },
                                None => Found::Missing,
                            };
                            cx.notify();
                        }
                    });
                }
            });
            self._tasks.push(task);
        }
    }

    /// Connects to what is typed for another device: looks around on a device, or asks a server.
    pub(super) fn connect_to_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let address = self.address.read(cx).value().trim().to_owned();
        match server::lookup(&address) {
            Ok(Lookup::Device(host)) => {
                self.scan(Device::Other, host, cx);
                cx.notify();
            }
            Ok(Lookup::Address(urls)) => {
                self.back_to = Back::Hosted;
                self.open_new_server(urls, window, cx);
            }
            Err(error) => {
                self.address_error = Some(error.to_string().into());
                cx.notify();
            }
        }
    }

    /// Connects to a server on another port of this device.
    pub(super) fn connect_to_port(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let port = self
            .port
            .read(cx)
            .value()
            .trim()
            .trim_start_matches(':')
            .to_owned();
        match port.parse::<u16>() {
            Ok(port) if port > 0 => {
                if let Ok(urls) = server::base_urls(&format!("{THIS_DEVICE}:{port}")) {
                    self.back_to = Back::Hosted;
                    self.open_new_server(urls, window, cx);
                }
            }
            _ => {
                self.address_error =
                    Some("Enter the port the server listens on, like 8000.".into());
                cx.notify();
            }
        }
    }

    fn open_found(
        &mut self,
        device: Device,
        server: KnownServer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(scan) = self.scan_of(device).as_mut() else {
            return;
        };
        let found = scan
            .servers
            .iter()
            .find(|(known, _)| *known == server)
            .map(|(_, found)| *found);
        match found {
            Some(Found::Running { .. }) => {
                scan.hint = None;
                let url = server.base_url(&scan.host);
                self.back_to = Back::Hosted;
                self.open_new_server(vec![url], window, cx);
            }
            Some(Found::Checking) | None => {}
            Some(Found::Missing) => {
                // It may run by now.
                scan.hint = server.help.map(|help| match device {
                    Device::This => help.start.into(),
                    Device::Other => format!("{} {}", help.start, help.share).into(),
                });
                let host = scan.host.clone();
                self.scan(device, host, cx);
                cx.notify();
            }
        }
    }

    pub(super) fn render_hosted(&self, cx: &mut Context<Self>) -> AnyElement {
        let this_detail: SharedString = match self.place.as_ref().map(|place| &place.pi) {
            Some(PiPlace::Remote { label, .. }) => format!("{label}, where pi runs").into(),
            _ => "Where Pistachio runs".into(),
        };
        let device_card = |device: Device, icon: AppIcon, title, detail: SharedString| {
            let selector = match device {
                Device::This => "device:this",
                Device::Other => "device:other",
            };
            div()
                .debug_selector(|| selector.into())
                .relative()
                .children(crate::demo::probe(selector))
                .w(relative(0.49))
                .child(
                    card(
                        selector.to_owned(),
                        icon,
                        title,
                        detail,
                        None,
                        self.device == Some(device),
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_device(device, cx))),
                )
        };
        let devices = h_flex()
            .justify_between()
            .child(device_card(
                Device::This,
                AppIcon::Server,
                "This device",
                this_detail,
            ))
            .child(device_card(
                Device::Other,
                AppIcon::Globe,
                "Another device",
                "By its address".into(),
            ));
        // Each choice opens a page of its own; Back goes to the one before.
        let (title, body) = match (self.device, self.this_way) {
            (None, _) => (
                "Locally hosted model",
                v_flex()
                    .gap_3()
                    .child(devices)
                    .child(label(
                        "Ollama, LM Studio, llama.cpp, MLX, vLLM, Jan, Docker Model Runner, or \
                         any server that speaks OpenAI's, Anthropic's or Gemini's API.",
                        cx,
                    ))
                    .into_any_element(),
            ),
            (Some(Device::Other), _) => ("Another device", self.render_other_device(cx)),
            (Some(Device::This), None) => ("This device", self.render_ways(cx).into_any_element()),
            (Some(Device::This), Some(Way::Attach)) if !self.suggests_models() => {
                ("This device", self.render_attach(cx))
            }
            (Some(Device::This), Some(Way::Attach)) => {
                ("Attach to existing model", self.render_attach(cx))
            }
            (Some(Device::This), Some(Way::Deploy)) => ("Deploy new model", self.render_deploy(cx)),
        };
        v_flex()
            .gap_3()
            .child(Self::render_top(title, None, cx))
            .child(body)
            .into_any_element()
    }

    /// Attach to a server that runs here, or deploy a new model.
    fn render_ways(&self, cx: &mut Context<Self>) -> Div {
        let way_card = |way: Way, icon: AppIcon, title| {
            let selector = match way {
                Way::Attach => "way:attach",
                Way::Deploy => "way:deploy",
            };
            div()
                .debug_selector(|| selector.into())
                .relative()
                .children(crate::demo::probe(selector))
                .w(relative(0.49))
                .child(
                    card(
                        selector.to_owned(),
                        icon,
                        title,
                        SharedString::default(),
                        None,
                        false,
                        cx,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_way(way, cx))),
                )
        };
        v_flex()
            .gap_1p5()
            .child(
                h_flex()
                    .justify_between()
                    .child(way_card(
                        Way::Attach,
                        AppIcon::Plug,
                        "Attach to existing model",
                    ))
                    .child(way_card(Way::Deploy, AppIcon::Download, "Deploy new model")),
            )
            .child(label(
                "Attach connects to a server that already runs here, like Ollama or LM Studio. \
                 Deploy looks at this device's hardware, suggests models that run well on it, \
                 and downloads and runs the one you pick.",
                cx,
            ))
    }

    /// The known servers on this device, and a server on another port.
    fn render_attach(&self, cx: &mut Context<Self>) -> AnyElement {
        let port = v_flex()
            .gap_1p5()
            .child(label("Another server on this device", cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(Input::new(&self.port).aria_label("Port"))
                    .child(connect_button(
                        "connect-port",
                        cx.listener(|this, _, window, cx| this.connect_to_port(window, cx)),
                    )),
            )
            .child(self.address_help(
                "Any server that speaks OpenAI's, Anthropic's or Gemini's API, by its port.",
                cx,
            ));
        v_flex()
            .gap_4()
            .children(
                self.this_device
                    .as_ref()
                    .map(|scan| self.render_scan(Device::This, scan, cx)),
            )
            .child(port)
            .into_any_element()
    }

    /// Models are suggested where pi runs on this computer: they run here.
    pub(super) fn suggests_models(&self) -> bool {
        matches!(
            self.place.as_ref().map(|place| &place.pi),
            Some(PiPlace::Local { .. })
        )
    }

    /// Scans the hardware when asked, shows what it found, and then suggests models for it.
    fn render_deploy(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(models) = crate::local_models::local_models(cx) else {
            return div().into_any_element();
        };
        // What the app scanned before shows only once asked for here.
        let asked = self.deploy != DeployStep::Start;
        let (scanning, facts) = match models.read(cx).scan() {
            Machine::Scanning { since, found } if asked => (Some((*since, *found)), None),
            Machine::Done { hardware, .. } if asked => (None, Some(hardware.facts())),
            Machine::NotYet | Machine::Scanning { .. } | Machine::Done { .. } => (None, None),
        };
        let accent = crate::theme::palette(cx).accent_icon;
        let scan_button = div()
            .flex_none()
            .debug_selector(|| "scan-hardware".into())
            .relative()
            .children(crate::demo::probe("scan-hardware"))
            .child(
                Button::new("scan-hardware")
                    .icon(Icon::new(IconName::Cpu).text_color(accent))
                    .label(if facts.is_some() {
                        "Scan again"
                    } else {
                        "Scan local hardware"
                    })
                    .when(facts.is_some(), |button| button.outline().xsmall())
                    .when(facts.is_none(), |button| button.form_other(cx))
                    .disabled(scanning.is_some())
                    .on_click(cx.listener(|this, _, _, cx| this.scan_hardware(cx))),
            );
        let facts_found = facts.is_some();
        let hardware: AnyElement = match (scanning, facts) {
            (Some((since, found)), _) => render_scanning(since, found, cx),
            (None, None) => v_flex()
                .gap_3()
                .child(label(
                    "Pistachio looks at this device's processor, memory, graphics and free \
                     disk to suggest models that run well here. Nothing leaves this device.",
                    cx,
                ))
                .child(h_flex().w_full().justify_center().child(scan_button))
                .into_any_element(),
            (None, Some(facts)) => v_flex()
                .gap_1p5()
                .child(
                    h_flex()
                        .justify_between()
                        .child(label("Found on this device", cx))
                        .child(scan_button),
                )
                .child(render_facts(&facts, cx))
                .into_any_element(),
        };
        let suggest = facts_found.then(|| -> AnyElement {
            if self.deploy == DeployStep::Suggestions {
                self.render_suggestions(&models, cx)
            } else {
                h_flex()
                    .w_full()
                    .justify_center()
                    .child(
                        div()
                            .debug_selector(|| "suggest-models".into())
                            .relative()
                            .children(crate::demo::probe("suggest-models"))
                            .child(
                                Button::new("suggest-models")
                                    .form_other(cx)
                                    .icon(Icon::from(AppIcon::Sparkles).text_color(accent))
                                    .label("Suggest models")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.deploy = DeployStep::Suggestions;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .into_any_element()
            }
        });
        v_flex()
            .gap_4()
            .child(hardware)
            .children(suggest)
            .into_any_element()
    }

    /// Four models for this computer (tiny, fast, balanced, most capable), each with the engine
    /// that runs it best here and, where one is faster, a specialized engine.
    fn render_suggestions(
        &self,
        models: &Entity<LocalModels>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (picks, busy, error) = {
            let state = models.read(cx);
            let picks: Vec<_> = match state.scan() {
                Machine::NotYet | Machine::Scanning { .. } => Vec::new(),
                Machine::Done { suggestions, .. } => suggestions
                    .iter()
                    .map(|suggestion| {
                        let status = |choice: &Option<Choice>| {
                            choice.as_ref().map(|choice| state.status(choice))
                        };
                        (
                            suggestion.clone(),
                            status(&suggestion.default),
                            status(&suggestion.specialized),
                        )
                    })
                    .collect(),
            };
            (picks, state.is_busy(), state.error().map(str::to_owned))
        };
        crate::online::refresh(cx);
        let body = if picks.is_empty() {
            label(
                "No model Pistachio knows runs well enough on this device.",
                cx,
            )
            .into_any_element()
        } else {
            v_flex()
                .gap_2()
                .children(picks.into_iter().map(|(suggestion, default, special)| {
                    self.render_suggestion(models, &suggestion, default, special, busy, cx)
                }))
                .into_any_element()
        };
        v_flex()
            .gap_1p5()
            .child(label("Suggested for this device", cx))
            .child(body)
            .children(error.map(|error| {
                div()
                    .text_xs()
                    .text_color(crate::theme::palette(cx).error)
                    .child(error)
            }))
            .into_any_element()
    }

    fn render_suggestion(
        &self,
        models: &Entity<LocalModels>,
        suggestion: &Suggestion,
        default: Option<Status>,
        special: Option<Status>,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let (border, muted) = (theme.border, theme.muted_foreground);
        let accent = crate::theme::palette(cx).accent;
        let tier = suggestion.tier.label();
        let expanded = self.expanded.contains(&tier);
        let toggle = div()
            .w_full()
            .debug_selector(move || format!("expand:{tier}"))
            .relative()
            .children(crate::demo::probe(format!("expand:{tier}")))
            .child(
                Button::new(SharedString::from(format!("expand:{tier}")))
                    .quiet(cx)
                    .w_full()
                    .h(px(24.))
                    .rounded_none()
                    .rounded_b_lg()
                    .border_t_1()
                    .border_color(border)
                    .icon(
                        Icon::new(if expanded {
                            IconName::ChevronUp
                        } else {
                            IconName::ChevronDown
                        })
                        .text_color(muted),
                    )
                    .accessibility_label(if expanded {
                        format!("Hide how {} runs", suggestion.model.name)
                    } else {
                        format!("Show how {} runs", suggestion.model.name)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(ix) = this.expanded.iter().position(|open| *open == tier) {
                            this.expanded.remove(ix);
                        } else {
                            this.expanded.push(tier);
                        }
                        cx.notify();
                    })),
            );
        let rows = [
            (suggestion.default.as_ref(), default, false),
            (suggestion.specialized.as_ref(), special, true),
        ];
        // The way it runs by default (else the specialized one) is downloaded and run from the
        // top of the card, also while it is closed.
        let main = rows
            .iter()
            .position(|(choice, status, _)| choice.is_some() && status.is_some());
        let main_action = main.and_then(|ix| {
            let (choice, status, specialized) = &rows[ix];
            let role = if *specialized {
                "specialized"
            } else {
                "default"
            };
            let id = format!("local-{tier}-{role}");
            let installing = matches!(status, Some(Status::Installing(_)));
            let action = self.choice_action(models, &id, (*choice)?, status.clone()?, busy, cx);
            Some(
                h_flex().w_full().justify_center().child(
                    div()
                        .when(installing, |this| this.w_full())
                        .debug_selector(move || id)
                        .child(action),
                ),
            )
        });
        let details = v_flex()
            .gap_1p5()
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(suggestion.model.why.clone()),
            )
            .children(rows.into_iter().enumerate().filter_map(
                |(ix, (choice, status, specialized))| {
                    Some(self.render_choice(
                        models,
                        tier,
                        choice?,
                        status?,
                        specialized,
                        Some(ix) != main,
                        busy,
                        cx,
                    ))
                },
            ));
        v_flex()
            .debug_selector(move || format!("suggestion:{tier}"))
            .w_full()
            .rounded_lg()
            .border_1()
            .border_color(border)
            .child(
                v_flex()
                    .w_full()
                    .gap_2()
                    .px_3()
                    .pt_2()
                    .pb_2()
                    .child(
                        h_flex()
                            .w_full()
                            .justify_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(accent)
                                    .child(tier),
                            )
                            .child(div().text_sm().child(suggestion.model.name.clone()))
                            .child(
                                div()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(suggestion.model.params.clone()),
                            ),
                    )
                    .children(main_action)
                    .when(expanded, |this| this.child(details)),
            )
            .child(toggle)
    }

    /// A way to run a suggested model: the engine, the checkpoint, the memory it takes and its
    /// speed here, and what to do with it.
    #[allow(clippy::too_many_arguments, clippy::cast_precision_loss)]
    fn render_choice(
        &self,
        models: &Entity<LocalModels>,
        tier: &'static str,
        choice: &Choice,
        status: Status,
        specialized: bool,
        with_action: bool,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let muted = cx.theme().muted_foreground;
        let role = if specialized {
            "specialized"
        } else {
            "default"
        };
        let id = format!("local-{tier}-{role}");
        let speed = if choice.measured.is_some() {
            "measured"
        } else {
            "estimated"
        };
        let mut notes = Vec::new();
        if choice.streams {
            notes.push("Reads the model from the disk as it goes".to_owned());
        }
        if specialized {
            notes.push(if choice.missing_tools.is_empty() {
                "Faster engine, built on this computer".to_owned()
            } else {
                format!(
                    "Faster engine, built on this computer; needs {}",
                    choice.missing_tools.join(" and ")
                )
            });
        }
        let engine = match choice.engine.home_page() {
            Some(url) => web_link(format!("{id}-engine"), choice.engine.name.clone(), url, cx)
                .into_any_element(),
            None => FadingText::new(choice.engine.name.clone()).into_any_element(),
        };
        let checkpoint = web_link(
            format!("{id}-checkpoint"),
            choice.build.repo.clone(),
            choice.build.page(),
            cx,
        )
        .debug_selector({
            let id = id.clone();
            move || format!("{id}-checkpoint")
        })
        .into_any_element();
        let quant = format!("{} {}", choice.build.quant, choice.build.format_label());
        let memory = format!("~{:.1} GB of RAM", choice.memory as f64 / 1e9);
        let tps = format!("~{:.0} tokens/s ({speed})", choice.tps);
        let tokens = |n: u64| format!("{}K", n / 1024);
        let context = if choice.context < choice.model.context {
            format!(
                "{} tokens (the model takes up to {})",
                tokens(choice.context),
                tokens(choice.model.context)
            )
        } else {
            format!("{} tokens", tokens(choice.context))
        };
        let images = if choice.model.vision {
            "Yes"
        } else {
            "No, text only"
        }
        .to_owned();
        let levels = local_models::thinking::levels(choice.model, choice.engine);
        let thinking = match choice.model.reasoning {
            Some(true) if levels.is_empty() => format!("Not with {}", choice.engine.name),
            Some(true) if levels == ["off", "high"] => "Yes · on or off".to_owned(),
            Some(true) => format!("Yes · {}", levels.join(", ")),
            Some(false) => "No".to_owned(),
            None => "Unknown".to_owned(),
        };
        let fact = |icon: Icon, name: &'static str, value: AnyElement| {
            h_flex()
                .flex_1()
                .min_w_0()
                .gap_2()
                .child(icon.small().text_color(muted))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_xs().text_color(muted).child(name))
                        .child(div().text_xs().min_w_0().truncate().child(value)),
                )
        };
        let text = |value: String| FadingText::new(value).into_any_element();
        h_flex()
            .w_full()
            .gap_3()
            .pt_1p5()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(fact(AppIcon::Server.into(), "Inference engine", engine))
                            .child(fact(AppIcon::Package.into(), "Quantization", text(quant))),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(fact(AppIcon::MemoryStick.into(), "Memory", text(memory)))
                            .child(fact(AppIcon::Gauge.into(), "Speed here", text(tps))),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(fact(AppIcon::ScrollText.into(), "Context", text(context)))
                            .child(fact(AppIcon::Image.into(), "Image input", text(images))),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(fact(AppIcon::Brain.into(), "Thinking", text(thinking)))
                            .child(fact(AppIcon::Link.into(), "Checkpoint", checkpoint)),
                    )
                    .children((!notes.is_empty()).then(|| label(notes.join(" · "), cx))),
            )
            .when(with_action, |this| {
                let installing = matches!(status, Status::Installing(_));
                let action = self.choice_action(models, &id, choice, status, busy, cx);
                this.child(
                    div()
                        .flex_none()
                        .when(installing, |this| this.w(px(220.)))
                        .debug_selector(move || id)
                        .child(action),
                )
            })
    }

    /// What can be done with `choice` as it stands: download and run it, run it, connect to it.
    #[allow(clippy::too_many_arguments)]
    fn choice_action(
        &self,
        models: &Entity<LocalModels>,
        id: &str,
        choice: &Choice,
        status: Status,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = id.to_owned();
        match status {
            Status::Missing(size) => {
                let (models, choice) = (models.clone(), choice.clone());
                let name = choice.model.name.clone();
                let button = if size > 0 {
                    download_and_run_button(id.clone(), &name, size, cx)
                } else {
                    Button::new(SharedString::from(id.clone()))
                        .label("Set up")
                        .outline()
                        .xsmall()
                };
                button
                    .disabled(busy)
                    .on_click(move |_, window, cx| {
                        LocalModels::ask_and_set_up(&models, &choice, window, cx);
                    })
                    .into_any_element()
            }
            Status::Installing(progress) => {
                let models = models.clone();
                let muted = cx.theme().muted_foreground;
                let accent = crate::theme::palette(cx).accent;
                let (stage, amount, fraction) = match &progress {
                    None => ("Starting the download…".to_owned(), String::new(), 0.),
                    Some(progress) => (
                        progress.stage.label(),
                        format!(
                            "{} of {} · {:.0}%",
                            size_label(progress.done),
                            size_label(progress.total),
                            progress.fraction() * 100.
                        ),
                        progress.fraction(),
                    ),
                };
                v_flex()
                    .w_full()
                    .gap_1p5()
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .text_xs()
                            .child(turning_loader(accent).small())
                            .child(div().flex_1().min_w_0().child(FadingText::new(stage)))
                            .child(div().flex_none().text_color(muted).child(amount))
                            .child(
                                Button::new(SharedString::from(format!("{id}-cancel")))
                                    .label("Cancel")
                                    .ghost()
                                    .xsmall()
                                    .on_click(move |_, _, cx| {
                                        models.update(cx, LocalModels::cancel);
                                    }),
                            ),
                    )
                    .child(progress_bar(fraction, cx))
                    .into_any_element()
            }
            Status::Installed => {
                let (models, choice) = (models.clone(), choice.clone());
                Button::new(SharedString::from(id.clone()))
                    .label("Run")
                    .outline()
                    .xsmall()
                    .on_click(move |_, _, cx| models.update(cx, |m, cx| m.run(&choice, cx)))
                    .into_any_element()
            }
            Status::Starting => label("Starting…", cx).into_any_element(),
            Status::Running => {
                let url = models.read(cx).url(choice).unwrap_or_default();
                Button::new(SharedString::from(id.clone()))
                    .label("Connect")
                    .outline()
                    .xsmall()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.back_to = Back::Hosted;
                        this.open_new_server(vec![url.clone()], window, cx);
                    }))
                    .into_any_element()
            }
        }
    }

    fn render_other_device(&self, cx: &mut Context<Self>) -> AnyElement {
        let address = v_flex()
            .gap_1p5()
            .child(label("Address of the device, or of the server", cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(Input::new(&self.address).aria_label("Address"))
                    .child(connect_button(
                        "connect",
                        cx.listener(|this, _, window, cx| this.connect_to_address(window, cx)),
                    )),
            )
            .child(self.address_help(
                "Without a port, Ollama, LM Studio, llama.cpp, MLX, vLLM and others are looked for \
                 on their usual ports. With one, any server that speaks OpenAI's, Anthropic's or \
                 Gemini's API.",
                cx,
            ));
        v_flex()
            .gap_4()
            .child(address)
            .children(
                self.other_device
                    .as_ref()
                    .map(|scan| self.render_scan(Device::Other, scan, cx)),
            )
            .into_any_element()
    }

    /// What went wrong with what was typed, else how it works.
    fn address_help(&self, help: &'static str, cx: &App) -> Div {
        match &self.address_error {
            Some(error) => div()
                .text_xs()
                .text_color(crate::theme::palette(cx).error)
                .child(error.clone()),
            None => label(help, cx),
        }
    }

    /// The known servers on a device: the common ones always, the others when they answer.
    fn render_scan(&self, device: Device, scan: &Scan, cx: &mut Context<Self>) -> Div {
        let prefix = match device {
            Device::This => "local",
            Device::Other => "other",
        };
        let cards: Vec<Div> = scan
            .servers
            .iter()
            .filter(|(server, found)| {
                server.help.is_some() || matches!(found, Found::Running { .. })
            })
            .map(|(server, found)| {
                let server = *server;
                let (name, end): (SharedString, _) = match *found {
                    Found::Running { kind, models } => (
                        kind.label().map_or_else(
                            || format!("Server on port {}", server.port).into(),
                            Into::into,
                        ),
                        available(models, cx),
                    ),
                    Found::Checking => (
                        server.label.into(),
                        label("Looking…", cx).flex_none().into_any_element(),
                    ),
                    Found::Missing => (
                        server.label.into(),
                        label("Not running", cx).flex_none().into_any_element(),
                    ),
                };
                let selector = format!("{prefix}:{}", server.label);
                div()
                    .debug_selector(|| selector.clone())
                    .w(relative(0.49))
                    .child(
                        card(
                            selector.clone(),
                            IconName::Cpu,
                            name,
                            SharedString::default(),
                            Some(end),
                            false,
                            cx,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.open_found(device, server, window, cx);
                            },
                        )),
                    )
            })
            .collect();
        let title = match device {
            Device::This => "On this device".to_owned(),
            Device::Other => format!("On {}", scan.host),
        };
        v_flex()
            .gap_1p5()
            .child(label(title, cx))
            .child(grid(cards))
            .children(scan.hint.clone().map(|hint| label(hint, cx)))
            .when(device == Device::Other && scan.nothing(), |this| {
                this.child(label(
                    format!(
                        "Nothing answers on the usual ports. If the server listens on another \
                         one, enter it too, like {}:8888.",
                        scan.host
                    ),
                    cx,
                ))
            })
    }
}

/// A bar that fills while the hardware is scanned (scanning `since`; `found` once it ended),
/// saying what is looked at, beside a turning loader.
fn render_scanning(since: std::time::Instant, found: bool, cx: &App) -> AnyElement {
    const STEPS: [&str; 5] = [
        "Reading the processor…",
        "Measuring the memory…",
        "Looking for graphics…",
        "Checking the free disk…",
        "Looking for build tools…",
    ];
    let theme = cx.theme();
    let (border, muted) = (theme.border, theme.muted_foreground);
    let fill = crate::theme::palette(cx).accent;
    let progress = crate::local_models::scan_progress(since.elapsed(), found);
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let step = ((progress * STEPS.len() as f32) as usize).min(STEPS.len() - 1);
    v_flex()
        .debug_selector(|| "scanning-hardware".into())
        .w_full()
        .gap_2()
        .px_3()
        .py_3()
        .rounded_lg()
        .border_1()
        .border_color(border)
        .child(
            h_flex()
                .w_full()
                .gap_2()
                .text_xs()
                .child(turning_loader(fill).small())
                .child(div().flex_1().child(STEPS[step]))
                .child(
                    div()
                        .text_color(muted)
                        .child(format!("{:.0}%", progress * 100.)),
                ),
        )
        .child(progress_bar(progress, cx))
        .into_any_element()
}

/// What the hardware scan found, a line each.
fn render_facts(facts: &[(Fact, String)], cx: &App) -> Div {
    let theme = cx.theme();
    let (border, muted) = (theme.border, theme.muted_foreground);
    v_flex()
        .w_full()
        .gap_1p5()
        .px_3()
        .py_2()
        .rounded_lg()
        .border_1()
        .border_color(border)
        .children(facts.iter().map(|(fact, value)| {
            let icon: Icon = match fact {
                Fact::Chip => IconName::Cpu.into(),
                Fact::Memory => AppIcon::MemoryStick.into(),
                Fact::Graphics => AppIcon::Gpu.into(),
                Fact::ForModels => AppIcon::Brain.into(),
                Fact::Disk => AppIcon::HardDrive.into(),
                Fact::System => AppIcon::Laptop.into(),
                Fact::Tools => AppIcon::Wrench.into(),
            };
            let name = fact.label();
            h_flex()
                .debug_selector(move || format!("hardware:{name}"))
                .w_full()
                .gap_2()
                .text_xs()
                .child(icon.small().text_color(muted))
                .child(div().w(px(84.)).flex_none().text_color(muted).child(name))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(FadingText::new(value.clone())),
                )
        }))
}

/// A Connect button next to a field.
fn connect_button(
    id: &'static str,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    div()
        .flex_none()
        .debug_selector(|| id.into())
        .child(Button::new(id).label("Connect").on_click(on_click))
}
