//! SearXNG in Settings → Web search: one row with a light and a "⋮" menu, like the saved SSH
//! connections, and the dialogs to download, remove and change it.

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::setting::SettingItem;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use searxng::{Installed, Probe, catalog};

use super::view::{go_back_to_previous_service, keep_service, update_web, web_config};
use crate::search_server::{Running, SearchServer, search_server};
use crate::theme::palette;
use crate::ui::dialog::{on_submit, open_form_dialog};
use crate::ui::download::{Details, Part, download_label, offline_aware};
use crate::ui::{AppIcon, QuietButton as _, light};

/// What an installation takes on disk, about.
const DISK_SIZE: u64 = 190_000_000;

/// The setting below the service when SearXNG is chosen.
pub(super) fn item() -> SettingItem {
    SettingItem::render(|_, _, cx| row(cx)).keywords(["SearXNG", "address"])
}

/// Whether chats search with the SearXNG on this computer.
pub(super) fn is_local(cx: &App) -> bool {
    web_config(cx).searxng.local && catalog::platform().is_some()
}

/// What answers at the address, once that is known.
pub(super) fn probed(cx: &App) -> Option<Probe> {
    let url = &web_config(cx).searxng.url;
    search_server(cx).read(cx).probed(url)
}

/// What the row shows.
#[derive(Debug, Clone, PartialEq)]
struct State {
    light: Light,
    title: String,
    text: String,
    /// How far the download is.
    progress: Option<f32>,
    actions: Vec<Action>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Light {
    On,
    Busy,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Start,
    Stop,
    Update,
    CancelDownload,
    /// Asks whether to download it, or uses the one that is installed.
    RunHere,
    CheckAgain,
    ChangeAddress,
    Remove,
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::Start => "Start",
            Self::Stop => "Stop",
            Self::Update => "Update",
            Self::CancelDownload => "Cancel download",
            Self::RunHere => "Run on this computer…",
            Self::CheckAgain => "Check again",
            Self::ChangeAddress => "Change address…",
            Self::Remove => "Remove…",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Start => AppIcon::Play.into(),
            Self::Stop => IconName::Square.into(),
            Self::Update | Self::RunHere => AppIcon::Download.into(),
            Self::CancelDownload => IconName::Close.into(),
            Self::CheckAgain => IconName::RefreshCw.into(),
            Self::ChangeAddress => AppIcon::Link.into(),
            Self::Remove => AppIcon::Trash.into(),
        }
    }

    fn run(self, window: &mut Window, cx: &mut App) {
        let server = search_server(cx);
        match self {
            Self::Start => server.update(cx, SearchServer::start),
            Self::Stop => server.update(cx, SearchServer::stop),
            Self::Update => crate::online::when_online(window, cx, move |_, cx| {
                server.update(cx, SearchServer::install);
            }),
            Self::CancelDownload => server.update(cx, |server, _| server.cancel_download()),
            Self::RunHere => run_here(window, cx),
            Self::CheckAgain => {
                let url = web_config(cx).searxng.url.clone();
                server.update(cx, |server, cx| server.probe(&url, true, cx));
            }
            Self::ChangeAddress => open_address_dialog(window, cx),
            Self::Remove => confirm_remove(window, cx),
        }
    }
}

fn state(cx: &App) -> State {
    if is_local(cx) {
        local_state(search_server(cx).read(cx))
    } else {
        address_state(&web_config(cx).searxng.url, probed(cx))
    }
}

fn local_state(server: &SearchServer) -> State {
    let state = |light, text: String, actions: &[Action]| State {
        light,
        title: "SearXNG on this computer".into(),
        text,
        progress: None,
        actions: actions.to_vec(),
    };
    if server.is_downloading() {
        let progress = server.progress();
        let text = progress.map_or_else(
            || "Downloading…".into(),
            |progress| {
                format!(
                    "Downloading {}… {:.0}%",
                    progress.stage.label(),
                    progress.fraction() * 100.
                )
            },
        );
        return State {
            progress: Some(progress.map_or(0., searxng::Progress::fraction)),
            ..state(Light::Busy, text, &[Action::CancelDownload])
        };
    }
    if server.installed() == Installed::No {
        let text = server.error().unwrap_or("Not downloaded yet").to_owned();
        return state(Light::Off, text, &[Action::RunHere, Action::ChangeAddress]);
    }
    let update = (server.installed() == Installed::Outdated).then_some(Action::Update);
    let installed = |first: Action| -> Vec<Action> {
        [
            Some(first),
            update,
            Some(Action::ChangeAddress),
            Some(Action::Remove),
        ]
        .into_iter()
        .flatten()
        .collect()
    };
    match server.running() {
        Running::Here(here) => state(
            Light::On,
            format!(
                "Running on port {} · only this computer can reach it",
                here.port()
            ),
            &installed(Action::Stop),
        ),
        // It is the other one's to stop, and its files are in use.
        Running::Elsewhere => state(
            Light::On,
            "Running, started by another Pistachio".into(),
            &[Action::ChangeAddress],
        ),
        Running::Starting => state(Light::Busy, "Starting…".into(), &[]),
        Running::No => state(
            Light::Off,
            server
                .error()
                .unwrap_or("Stopped · starts when a chat searches the web")
                .to_owned(),
            &installed(Action::Start),
        ),
    }
}

fn address_state(url: &str, probed: Option<Probe>) -> State {
    let (light, text) = match probed {
        None => (Light::Busy, "Checking…".to_owned()),
        Some(Probe::Ready) => (Light::On, "Ready to search".to_owned()),
        Some(problem) => (Light::Off, problem_text(url, problem)),
    };
    let text = if light == Light::Off && catalog::platform().is_some() {
        format!("{text} “Run on this computer…” in the menu gets one.")
    } else {
        text
    };
    let here = catalog::platform().map(|_| Action::RunHere);
    State {
        light,
        title: format!("SearXNG at {url}"),
        text,
        progress: None,
        actions: [Some(Action::CheckAgain), Some(Action::ChangeAddress), here]
            .into_iter()
            .flatten()
            .collect(),
    }
}

fn problem_text(url: &str, probe: Probe) -> String {
    match probe {
        Probe::Ready => String::new(),
        Probe::NoJson => format!(
            "SearXNG at {url} refuses the json format: add it to search.formats in its \
             settings.yml."
        ),
        Probe::Unexpected(status) => {
            format!("What answers at {url} is not a SearXNG that can be used (HTTP {status}).")
        }
        Probe::Nothing => format!("Nothing answers at {url}."),
    }
}

/// The light, what it is and how it is, and "⋮".
fn row(cx: &App) -> AnyElement {
    let state = state(cx);
    let colors = palette(cx);
    let theme = cx.theme();
    let light_color = match state.light {
        Light::On => colors.success,
        Light::Busy => colors.warning,
        Light::Off => colors.error,
    };
    let label = format!("More actions for {}", state.title);
    let actions = state.actions;
    // A content-derived id: gpui-kit keeps the menu it built for an id.
    let menu_id = format!("searxng-menu:{actions:?}");
    h_flex()
        .w_full()
        .gap_2()
        .py_1()
        .debug_selector(|| "searxng-row".into())
        .child(light(light_color).debug_selector(|| match state.light {
            Light::On => "searxng-on".into(),
            Light::Busy => "searxng-busy".into(),
            Light::Off => "searxng-off".into(),
        }))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(div().text_sm().truncate().child(state.title))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(state.text),
                )
                .when_some(state.progress, |this, fraction| {
                    this.child(
                        div()
                            .mt_1()
                            .w_full()
                            .h(px(4.))
                            .rounded_full()
                            .bg(theme.border)
                            .child(
                                div()
                                    .h_full()
                                    .rounded_full()
                                    .bg(colors.accent)
                                    .w(relative(fraction)),
                            ),
                    )
                }),
        )
        .when(!actions.is_empty(), |this| {
            this.child(
                div()
                    .flex_none()
                    .debug_selector(|| "searxng-menu".into())
                    .child(
                        Button::new(SharedString::from(menu_id))
                            .quiet(cx)
                            .small()
                            .icon(IconName::EllipsisVertical)
                            .accessibility_label(label)
                            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                                actions.iter().copied().fold(menu, add_action)
                            }),
                    ),
            )
        })
        .into_any_element()
}

fn add_action(menu: PopupMenu, action: Action) -> PopupMenu {
    let item = PopupMenuItem::new(action.label())
        .icon(action.icon())
        .on_click(move |_, window, cx| action.run(window, cx));
    if action == Action::Remove {
        menu.separator().item(item)
    } else {
        menu.item(item)
    }
}

/// Uses the SearXNG on this computer, or asks whether to download it.
fn run_here(window: &mut Window, cx: &mut App) {
    if search_server(cx).read(cx).installed() == Installed::No {
        open_setup(window, cx);
    } else {
        use_local(cx);
    }
}

/// Makes chats search with the SearXNG on this computer, downloading it first if it is missing.
/// The Settings page must have been opened.
pub fn use_local(cx: &mut App) {
    update_web(cx, |config| config.searxng.local = true);
    search_server(cx).update(cx, |server, cx| {
        if server.installed() == Installed::No {
            server.install(cx);
        } else {
            server.start(cx);
        }
    });
}

/// Asks whether to download what is missing to run SearXNG here.
pub fn open_setup(window: &mut Window, cx: &mut App) {
    let problem = match probed(cx) {
        Some(Probe::Ready) | None => String::new(),
        Some(problem) => {
            let mut text = problem_text(&web_config(cx).searxng.url, problem);
            text.push(' ');
            text
        }
    };
    let details = setup_details(&problem);
    crate::online::refresh(cx);
    open_form_dialog(None, window, cx, move |dialog, _, cx| {
        dialog
            .title("Download SearXNG?")
            .w(px(540.))
            .on_cancel(|_, _, cx| {
                go_back_to_previous_service(cx);
                true
            })
            .child(details.render(cx))
            .footer(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        Button::new("searxng-setup-address")
                            .quiet(cx)
                            .h(px(30.))
                            .icon(AppIcon::Link)
                            .label("Use another address…")
                            .on_click(|_, window, cx| {
                                go_back_to_previous_service(cx);
                                window.close_dialog(cx);
                                open_address_dialog(window, cx);
                            }),
                    )
                    .child(div().flex_1())
                    .child(cancel_button("searxng-setup-cancel", cx))
                    .child(
                        offline_aware(
                            Button::new("searxng-setup-run")
                                .form_action(cx)
                                .icon(AppIcon::Download)
                                .label(format!("{} and run", download_label(details.size))),
                            cx,
                        )
                        .on_click(|_, window, cx| {
                            crate::online::when_online(window, cx, |window, cx| {
                                window.close_dialog(cx);
                                use_local(cx);
                            });
                        }),
                    ),
            )
    });
}

fn cancel_button(id: &'static str, cx: &App) -> Button {
    Button::new(id)
        .form_other(cx)
        .icon(IconName::Close)
        .label("Cancel")
        .on_click(|_, window, cx| {
            go_back_to_previous_service(cx);
            window.close_dialog(cx);
        })
}

fn setup_details(problem: &str) -> Details {
    let platform = catalog::platform();
    let size = |of: fn(&catalog::Platform) -> u64| platform.as_ref().map_or(0, of);
    Details {
        intro: format!(
            "{problem}Pistachio can download SearXNG and run it on this computer: a private search \
             engine that needs no account, no key and no Docker."
        ),
        parts: vec![
            Part {
                link: None,
                icon: AppIcon::Globe.into(),
                name: "SearXNG".into(),
                note: "The search engine".into(),
                size: catalog::SEARXNG_SIZE,
            },
            Part {
                link: None,
                icon: AppIcon::CodeXml.into(),
                name: format!("Python {}", catalog::PYTHON_VERSION),
                note: "A private copy; your own Python is not touched".into(),
                size: size(|platform| platform.python.python_size),
            },
            Part {
                link: None,
                icon: AppIcon::Package.into(),
                name: format!("{} Python packages", catalog::package_count()),
                note: "What SearXNG is built on".into(),
                size: size(|platform| platform.packages_size),
            },
            Part {
                link: None,
                icon: AppIcon::Wrench.into(),
                name: format!("uv {}", catalog::UV_VERSION),
                note: "Installs the rest, and is deleted afterwards".into(),
                size: size(|platform| platform.python.uv_size),
            },
        ],
        size: size(catalog::Platform::download_size),
        disk_size: DISK_SIZE,
        note: "It goes into Pistachio's own folder, only this computer can reach it, and you can \
               remove it at any time. License: AGPL-3.0."
            .into(),
    }
}

/// Asks for the address of a SearXNG and searches with that one.
pub(super) fn open_address_dialog(window: &mut Window, cx: &mut App) {
    let url = web_config(cx).searxng.url.clone();
    let input = cx.new(|cx| InputState::new(window, cx).default_value(url));
    let submit = on_submit(&input, window, cx, |text, window, cx| {
        if use_address(&text, cx) {
            window.close_dialog(cx);
        }
    });
    let focus = input.focus_handle(cx);
    open_form_dialog(Some(focus), window, cx, move |dialog, _, cx| {
        let submit = submit.clone();
        dialog
            .title("SearXNG address")
            .w(px(460.))
            .on_cancel(|_, _, cx| {
                go_back_to_previous_service(cx);
                true
            })
            .child(
                v_flex().gap_2().child(Input::new(&input)).child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Its settings.yml must allow the json format."),
                ),
            )
            .footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(cancel_button("searxng-address-cancel", cx))
                    .child(
                        Button::new("searxng-address-use")
                            .form_action(cx)
                            .icon(IconName::Check)
                            .label("Use this address")
                            .on_click(move |_, window, cx| submit(window, cx)),
                    ),
            )
    });
}

/// Searches with the SearXNG at `address`, unless that is empty.
fn use_address(address: &str, cx: &mut App) -> bool {
    let address = address.trim();
    if address.is_empty() {
        return false;
    }
    let url = if address.contains("://") {
        address.to_owned()
    } else {
        format!("http://{address}")
    };
    keep_service(cx);
    update_web(cx, |config| {
        config.backend = deepsearch::config::BackendKind::Searxng;
        config.searxng.local = false;
        config.searxng.url.clone_from(&url);
    });
    // Also when it was this one already.
    search_server(cx).update(cx, |server, cx| server.probe(&url, true, cx));
    true
}

/// Asks before deleting the SearXNG on this computer.
fn confirm_remove(window: &mut Window, cx: &mut App) {
    open_form_dialog(None, window, cx, move |dialog, _, cx| {
        dialog
            .title("Remove SearXNG?")
            .w(px(420.))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "It stops, and everything that was downloaded for it is deleted (about \
                         {DISK_SIZE}). Chats search with DuckDuckGo instead."
                    )),
            )
            .footer(
                h_flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(cancel_button("searxng-remove-cancel", cx))
                    .child(
                        Button::new("searxng-remove")
                            .form_danger(cx)
                            .icon(AppIcon::Trash)
                            .label("Remove")
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                remove(cx);
                            }),
                    ),
            )
    });
}

fn remove(cx: &mut App) {
    search_server(cx).update(cx, SearchServer::remove);
    update_web(cx, |config| {
        config.searxng.local = false;
        config.backend = deepsearch::config::BackendKind::Duckduckgo;
    });
}

#[cfg(test)]
mod tests {
    use searxng::Probe;

    use super::{Light, address_state, setup_details};

    #[test]
    fn the_details_are_all_that_is_downloaded() {
        if super::catalog::platform().is_some() {
            setup_details("").assert_adds_up();
        }
    }

    #[test]
    fn the_light_says_whether_an_address_can_be_used() {
        let light = |probe| address_state("http://box", probe).light;
        assert_eq!(light(None), Light::Busy);
        assert_eq!(light(Some(Probe::Ready)), Light::On);
        for problem in [Probe::Nothing, Probe::NoJson, Probe::Unexpected(200)] {
            let state = address_state("http://box", Some(problem));
            assert_eq!(state.light, Light::Off);
            assert!(state.text.contains("http://box"), "{}", state.text);
        }
    }
}
