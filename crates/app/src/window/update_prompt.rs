//! The update prompt and the "What's new" card shown after an update: a card centered on the
//! window, with the window blurred behind it.

use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::updater::{Install, Step};
use crate::updates::{CURRENT_VERSION, Release};
use gpui_kit::component::Disableable as _;

const RELEASES_URL: &str = "https://github.com/seba2390/pistachio/releases";

/// What the card over the chat column shows.
pub(super) enum Prompt {
    /// A newer release, and how far installing it got.
    Update {
        release: Release,
        /// How to install it, or why this copy can't (then it is downloaded in the browser).
        install: Result<Install, SharedString>,
        state: UpdateState,
    },
    /// pi is not on this computer: install it, or point at it (see [`super::pi_setup`]).
    PiMissing { state: pi_setup::PiState },
    /// What changed since the version that ran before this one.
    WhatsNew {
        version: SharedString,
        from: SharedString,
        notes: SharedString,
    },
}

/// What is drawn behind the card.
pub(super) enum Backdrop {
    None,
    /// Waiting for a picture of the window without the card (the card shows once it is
    /// there, so it is never in the picture).
    Capturing,
    /// The window as it was, blurred.
    Blurred(Arc<RenderImage>),
    /// No picture (other systems, or capturing failed): a dense veil instead.
    Veil,
}

pub(super) enum UpdateState {
    Offer,
    Working(Step),
    Failed(SharedString),
}

impl MainWindow {
    /// Shows `release` with an Update button; nothing happens without a click.
    pub(super) fn offer_update(
        &mut self,
        release: Release,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let working = matches!(
            self.update_prompt,
            Some(Prompt::Update {
                state: UpdateState::Working(_),
                ..
            })
        );
        if working {
            return;
        }
        let install = crate::updater::detect().map_err(SharedString::from);
        if let Err(reason) = &install {
            log::info!("this copy can't update itself: {reason}");
        }
        self.show_prompt(
            Prompt::Update {
                release,
                install,
                state: UpdateState::Offer,
            },
            window,
            cx,
        );
    }

    /// After an update (a newer version than the one that ran last): what changed. Records
    /// this version as the one that ran.
    pub(super) fn show_whats_new_if_updated(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let last = cx.global::<AppSettings>().last_run_version.clone();
        if last.as_deref() != Some(CURRENT_VERSION) {
            AppSettings::update(cx, |settings| {
                settings.last_run_version = Some(CURRENT_VERSION.to_owned());
            });
        }
        // The first start knows no earlier version: nothing to compare with.
        let Some(last) = last.filter(|last| crate::updates::is_newer(CURRENT_VERSION, last)) else {
            return;
        };
        let Some(notes) = crate::whats_new::since(&last, CURRENT_VERSION) else {
            return;
        };
        self.show_prompt(
            Prompt::WhatsNew {
                version: CURRENT_VERSION.into(),
                from: last.into(),
                notes: notes.into(),
            },
            window,
            cx,
        );
    }

    /// Shows `prompt` over the window, once a blurred picture of the window is ready to go
    /// behind it.
    pub(super) fn show_prompt(
        &mut self,
        prompt: Prompt,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let replacing = self.update_prompt.is_some();
        self.update_prompt = Some(prompt);
        // Escape reaches the window, not a message box behind the card.
        self.focus_handle.focus(window, cx);
        cx.notify();
        // A card already up keeps its backdrop (a picture now would show that card).
        if replacing && !matches!(self.update_backdrop, Backdrop::None) {
            return;
        }
        if !cfg!(target_os = "macos") {
            self.update_backdrop = Backdrop::Veil;
            return;
        }
        self.update_backdrop = Backdrop::Capturing;
        request_capture();
        window.refresh();
        let background = cx.theme().background.to_rgb();
        let tint = [background.r, background.g, background.b];
        cx.spawn(async move |this, cx| {
            let mut frame = None;
            for _ in 0..30 {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                frame = take_capture();
                if frame.is_some() {
                    break;
                }
            }
            let blurred = match frame {
                Some(frame) => Some(
                    cx.background_executor()
                        .spawn(async move { crate::ui::backdrop::blur(&frame, tint, 0.35) })
                        .await,
                ),
                None => None,
            };
            let _ = this.update(cx, |this, cx| {
                if !matches!(this.update_backdrop, Backdrop::Capturing) {
                    return;
                }
                this.update_backdrop = match blurred {
                    Some(image) => Backdrop::Blurred(image),
                    None => {
                        log::info!("no picture of the window to blur; using a veil");
                        Backdrop::Veil
                    }
                };
                cx.notify();
            });
        })
        .detach();
    }

    /// Closes the card, unless an update or pi is being installed.
    pub(super) fn dismiss_update_prompt(&mut self, cx: &mut Context<Self>) {
        let working = matches!(
            self.update_prompt,
            Some(
                Prompt::Update {
                    state: UpdateState::Working(_),
                    ..
                } | Prompt::PiMissing {
                    state: pi_setup::PiState::Working(_)
                }
            )
        );
        if !working && self.update_prompt.take().is_some() {
            if let Backdrop::Blurred(image) =
                std::mem::replace(&mut self.update_backdrop, Backdrop::None)
            {
                cx.drop_image(image, None);
            }
            self.update_backdrop = Backdrop::None;
            cx.notify();
        }
    }

    pub(super) fn update_prompt_key(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.update_prompt.is_some() && event.keystroke.key == "escape" {
            cx.stop_propagation();
            self.dismiss_update_prompt(cx);
        }
    }

    /// Installs the offered release in the background, then quits and opens the new version.
    fn start_update(&mut self, cx: &mut Context<Self>) {
        let Some(Prompt::Update {
            release,
            install: Ok(install),
            state,
        }) = self.update_prompt.as_mut()
        else {
            return;
        };
        if matches!(state, UpdateState::Working(_)) {
            return;
        }
        *state = UpdateState::Working(Step::Preparing("Starting…"));
        cx.notify();
        let (release, install) = (release.clone(), install.clone());
        let (steps, progress) = async_channel::unbounded::<Step>();
        let task = cx.background_executor().spawn(async move {
            crate::updater::install(&install, &release, &|step| {
                let _ = steps.send_blocking(step);
            })
        });
        cx.spawn(async move |this, cx| {
            while let Ok(step) = progress.recv().await {
                let updated = this.update(cx, |this, cx| {
                    if let Some(Prompt::Update { state, .. }) = this.update_prompt.as_mut() {
                        *state = UpdateState::Working(step);
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    return;
                }
            }
            let result = task.await.and_then(|relaunch| {
                crate::updater::relaunch(&relaunch)?;
                Ok(())
            });
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => {
                    log::info!("updated; restarting");
                    crate::perf::phase("quit");
                    cx.quit();
                }
                Err(error) => {
                    log::warn!("update failed: {error}");
                    if let Some(Prompt::Update { state, .. }) = this.update_prompt.as_mut() {
                        *state = UpdateState::Failed(error.into());
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The blurred window behind the card; a click on it closes the card.
    pub(super) fn render_update_scrim(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.update_prompt.as_ref()?;
        let scrim = div().id("update-scrim").absolute().inset_0().occlude();
        let scrim = match &self.update_backdrop {
            Backdrop::None | Backdrop::Capturing => return None,
            Backdrop::Blurred(image) => scrim.child(
                img(image.clone())
                    .absolute()
                    .inset_0()
                    .size_full()
                    .object_fit(ObjectFit::Fill),
            ),
            // GPUI cannot blur what is behind an element without a picture of it: a dense
            // veil of the background color hides the detail instead.
            Backdrop::Veil => scrim.bg(cx.theme().background.alpha(0.82)),
        };
        Some(
            scrim
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.dismiss_update_prompt(cx)),
                )
                .with_animation(
                    "update-scrim-in",
                    Animation::new(Duration::from_millis(220)).with_easing(ease_out_quint()),
                    |this, delta| this.opacity(delta),
                )
                .into_any_element(),
        )
    }

    /// The card, centered on the window.
    pub(super) fn render_update_card(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if matches!(self.update_backdrop, Backdrop::None | Backdrop::Capturing) {
            return None;
        }
        let card = match self.update_prompt.as_ref()? {
            Prompt::Update {
                release,
                install,
                state,
            } => self.render_update_offer(release, install, state, cx),
            Prompt::PiMissing { state } => self.render_pi_missing(state, cx),
            Prompt::WhatsNew {
                version,
                from,
                notes,
            } => render_whats_new(version, from, notes, cx),
        };
        let card = v_flex()
            .id("update-card")
            .occlude()
            .w(px(540.))
            .max_w(relative(0.92))
            .p_5()
            .gap_4()
            .rounded(px(14.))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .shadow_2xl()
            .child(card)
            .with_animation(
                "update-card-in",
                Animation::new(Duration::from_millis(260)).with_easing(ease_out_quint()),
                |this, delta| this.opacity(delta).mt(px(16. * (1. - delta))),
            );
        Some(
            v_flex()
                .absolute()
                .inset_0()
                .items_center()
                .justify_center()
                .child(card)
                .into_any_element(),
        )
    }

    fn render_update_offer(
        &self,
        release: &Release,
        install: &Result<Install, SharedString>,
        state: &UpdateState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (muted, border) = (theme.muted_foreground, theme.border);
        let palette = crate::theme::palette(cx);
        let (accent, error) = (palette.accent, palette.error);
        let working = matches!(state, UpdateState::Working(_));
        let notes: SharedString = release.notes.trim().to_owned().into();
        let current = crate::updates::compared_version();

        let status: AnyElement = match (state, install) {
            (UpdateState::Working(step), _) => render_progress(step, accent, border, muted),
            (UpdateState::Failed(message), _) => div()
                .p_3()
                .rounded_lg()
                .border_1()
                .border_color(error.alpha(0.5))
                .bg(error.alpha(0.08))
                .text_sm()
                .text_color(error)
                .child(message.clone())
                .into_any_element(),
            (UpdateState::Offer, Ok(_)) => div()
                .text_xs()
                .text_color(muted)
                .child("Pistachio closes, installs the update and opens again.")
                .into_any_element(),
            (UpdateState::Offer, Err(reason)) => div()
                .text_xs()
                .text_color(muted)
                .child(format!("{reason} The download opens in your browser."))
                .into_any_element(),
        };

        let page = release.page_url.clone();
        let download = release
            .download_url
            .clone()
            .unwrap_or_else(|| release.page_url.clone());
        let version = release.version.clone();
        let failed = matches!(state, UpdateState::Failed(_));
        crate::online::refresh(cx);
        let primary = match install {
            Ok(_) => crate::ui::download::offline_aware(
                Button::new("update-now").label(if failed { "Try again" } else { "Update" }),
                cx,
            )
            .primary()
            .loading(working)
            .disabled(working)
            .on_click(cx.listener(|_, _, window, cx| {
                let this = cx.weak_entity();
                crate::online::when_online(window, cx, move |_, cx| {
                    let _ = this.update(cx, |this, cx| this.start_update(cx));
                });
            })),
            Err(_) => Button::new("update-download")
                .primary()
                .label("Download")
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.open_url(&download);
                    this.dismiss_update_prompt(cx);
                })),
        };
        let browser = failed && install.is_ok();
        let fallback = release
            .download_url
            .clone()
            .unwrap_or_else(|| release.page_url.clone());

        v_flex()
            .gap_4()
            .child(header(
                AppIcon::PiLogo,
                format!("Pistachio {} is available", release.version),
                format!("You have version {current}."),
                cx,
            ))
            .when(!notes.is_empty() && !working, |this| {
                this.child(
                    div()
                        .id("update-notes")
                        .max_h(px(260.))
                        .overflow_y_scroll()
                        .text_sm()
                        .child(component::text::TextView::markdown(
                            "update-notes-text",
                            notes,
                        )),
                )
            })
            .child(status)
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .child(div().when(!working, |this| {
                        this.child(
                            Button::new("update-skip")
                                .quiet(cx)
                                .label("Skip this version")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let version = version.clone();
                                    AppSettings::update(cx, |s| s.skipped_version = Some(version));
                                    this.dismiss_update_prompt(cx);
                                })),
                        )
                    }))
                    .child(
                        h_flex()
                            .gap_2()
                            .when(browser, |this| {
                                this.child(
                                    Button::new("update-browser")
                                        .label("Download instead")
                                        .on_click(move |_, _, cx| cx.open_url(&fallback)),
                                )
                            })
                            .when(!browser, |this| {
                                this.child(
                                    Button::new("update-notes-page")
                                        .label("What's new")
                                        .on_click(move |_, _, cx| cx.open_url(&page)),
                                )
                            })
                            .child(
                                Button::new("update-later")
                                    .label("Later")
                                    .disabled(working)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.dismiss_update_prompt(cx);
                                    })),
                            )
                            .child(primary),
                    ),
            )
            .into_any_element()
    }
}

/// Asks for a picture of the next frame the window draws (macOS; see
/// `vendor/gpui-pre-macos`).
fn request_capture() {
    #[cfg(target_os = "macos")]
    gpui_macos::request_frame_capture();
}

fn take_capture() -> Option<image::RgbaImage> {
    #[cfg(target_os = "macos")]
    return gpui_macos::take_frame_capture();
    #[cfg(not(target_os = "macos"))]
    None
}

fn render_whats_new(
    version: &SharedString,
    from: &SharedString,
    notes: &SharedString,
    cx: &mut Context<MainWindow>,
) -> AnyElement {
    let page = format!("{RELEASES_URL}/tag/v{version}");
    v_flex()
        .gap_4()
        .child(header(
            AppIcon::Sparkles,
            format!("What's new in Pistachio {version}"),
            format!("Updated from version {from}."),
            cx,
        ))
        .child(
            div()
                .id("whats-new-notes")
                .max_h(px(420.))
                .overflow_y_scroll()
                .pr_2()
                .text_sm()
                .child(component::text::TextView::markdown(
                    "whats-new-text",
                    notes.clone(),
                )),
        )
        .child(
            h_flex()
                .w_full()
                .justify_between()
                .child(
                    Button::new("whats-new-page")
                        .quiet(cx)
                        .label("Release page")
                        .on_click(move |_, _, cx| cx.open_url(&page)),
                )
                .child(
                    Button::new("whats-new-done")
                        .primary()
                        .label("Continue")
                        .on_click(cx.listener(|this, _, _, cx| this.dismiss_update_prompt(cx))),
                ),
        )
        .into_any_element()
}

/// Icon badge, title and a muted line under it.
pub(super) fn header(
    icon: AppIcon,
    title: String,
    detail: String,
    cx: &mut Context<MainWindow>,
) -> impl IntoElement {
    let theme = cx.theme();
    let (foreground, muted) = (theme.foreground, theme.muted_foreground);
    let accent = crate::theme::palette(cx).accent;
    h_flex()
        .gap_3()
        .child(
            div()
                .flex_none()
                .size(px(40.))
                .rounded(px(10.))
                .bg(accent.alpha(0.14))
                .flex()
                .items_center()
                .justify_center()
                .child(component::Icon::from(icon).size_5().text_color(accent)),
        )
        .child(
            v_flex()
                .min_w_0()
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(foreground)
                        .child(title),
                )
                .child(div().text_sm().text_color(muted).child(detail)),
        )
}

/// How the bar of [`render_progress`] is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Bar {
    /// Nothing measured yet: a piece moving along it.
    Moving,
    /// The share that is done.
    Filled(f32),
    /// Downloaded, not finished: full and pulsing.
    Pulsing,
}

fn progress_bar(step: &Step) -> Bar {
    match step {
        Step::Downloading { received, total } if *total > 0 => {
            #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)] // display only
            Bar::Filled((*received as f64 / *total as f64).clamp(0., 1.) as f32)
        }
        Step::Preparing(_) | Step::Downloading { .. } => Bar::Moving,
        Step::Installing(_) => Bar::Pulsing,
    }
}

/// A progress bar and what is happening.
pub(super) fn render_progress(step: &Step, accent: Hsla, track: Hsla, muted: Hsla) -> AnyElement {
    let label = match step {
        Step::Downloading { received, total } if *total > 0 => format!(
            "Downloading… {} of {}",
            crate::model_store::size_label(*received),
            crate::model_store::size_label(*total)
        ),
        Step::Downloading { received, .. } => {
            format!("Downloading… {}", crate::model_store::size_label(*received))
        }
        Step::Preparing(text) | Step::Installing(text) => (*text).to_owned(),
    };
    let fill = div().h_full().rounded_full().bg(accent);
    let fill = match progress_bar(step) {
        Bar::Filled(fraction) => fill.w(relative(fraction)).into_any_element(),
        Bar::Moving => {
            const WIDTH: f32 = 0.3;
            fill.absolute()
                .top_0()
                .w(relative(WIDTH))
                .with_animation(
                    "update-moving",
                    Animation::new(Duration::from_millis(1400))
                        .repeat()
                        .with_easing(ease_in_out),
                    |this, delta| this.left(relative(delta * (1. + WIDTH) - WIDTH)),
                )
                .into_any_element()
        }
        Bar::Pulsing => fill
            .w_full()
            .with_animation(
                "update-pulse",
                Animation::new(Duration::from_millis(1100))
                    .repeat()
                    .with_easing(pulsating_between(0.25, 0.9)),
                |this, delta| this.opacity(delta),
            )
            .into_any_element(),
    };
    let bar = div()
        .relative()
        .w_full()
        .h(px(6.))
        .rounded_full()
        .bg(track)
        .overflow_hidden()
        .child(fill);
    v_flex()
        .gap_2()
        .child(bar)
        .child(div().text_xs().text_color(muted).child(label))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{Bar, Step, progress_bar};

    #[test]
    fn the_bar_fills_as_the_download_goes_on() {
        let at = |received, total| progress_bar(&Step::Downloading { received, total });
        assert_eq!(progress_bar(&Step::Preparing("")), Bar::Moving);
        assert_eq!(at(0, 200), Bar::Filled(0.));
        assert_eq!(at(50, 200), Bar::Filled(0.25));
        assert_eq!(at(300, 200), Bar::Filled(1.));
        assert_eq!(at(50, 0), Bar::Moving, "an unknown size");
        assert_eq!(progress_bar(&Step::Installing("")), Bar::Pulsing);
    }
}
