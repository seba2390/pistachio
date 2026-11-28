//! Speech-to-text UI: the download dialog, the progress banner and the settings rows.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::catalog::{self, DEFAULT_MODEL, MODELS, Model, ModelFile};
use super::models::models;
use crate::model_store::Downloadable as _;
use crate::settings::AppSettings;
use crate::theme::palette;
use crate::ui::AppIcon;
use crate::ui::dialog::open_form_dialog;
use crate::ui::download::{self, Details, Part, download_button, download_label};

/// Asks whether to download a model, showing each one's size, speed and languages.
pub fn open_download_dialog(window: &mut Window, cx: &mut App) {
    let dialog = cx.new(|_| DownloadDialog {
        selected: catalog::model(DEFAULT_MODEL).unwrap_or(&MODELS[0]),
    });
    open_form_dialog(None, window, cx, move |dialog_view, _, _| {
        dialog_view
            .title("Download a speech-to-text model?")
            .w(px(560.))
            .child(dialog.clone())
    });
}

pub struct DownloadDialog {
    selected: &'static Model,
}

impl Render for DownloadDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::online::refresh(cx);
        let muted = cx.theme().muted_foreground;
        let rows: Vec<AnyElement> = MODELS
            .iter()
            .map(|model| {
                let selected = model.id == self.selected.id;
                model_card(model, selected, cx)
                    .id(SharedString::from(format!("speech-model-{}", model.id)))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = model;
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect();
        let selected = self.selected;
        v_flex()
            .gap_3()
            .child(div().text_sm().text_color(muted).child(
                "Speak instead of typing. The model runs on this computer, so what you say is \
                 never sent anywhere. It is downloaded once from Hugging Face; you can change \
                 or remove it later in Settings → Speech to text.",
            ))
            .child(v_flex().gap_2().children(rows))
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .pt_1()
                    .child(
                        Button::new("speech-download-cancel")
                            .label("Not now")
                            .ghost()
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        download::offline_aware(
                            Button::new("speech-download-start")
                                .icon(AppIcon::Download)
                                .label(download_label(selected.size())),
                            cx,
                        )
                        .primary()
                        .on_click(move |_, window, cx| {
                            crate::online::when_online(window, cx, move |window, cx| {
                                start_download(selected, cx);
                                window.close_dialog(cx);
                            });
                        }),
                    ),
            )
    }
}

/// Asks whether to download `model`, showing what that is.
pub fn ask_to_download(model: &'static Model, window: &mut Window, cx: &mut App) {
    download::ask_to_download(
        model.name,
        details(model),
        move |cx| start_download(model, cx),
        window,
        cx,
    );
}

fn details(model: &Model) -> Details {
    Details {
        intro: format!(
            "{} by {} turns what you say into text on this computer. {} · {}.",
            model.name,
            model.maker,
            model.languages,
            model.speed.label()
        ),
        parts: model.files.iter().map(part).collect(),
        size: model.size(),
        // The files are used as they are.
        disk_size: model.size(),
        note: format!(
            "It comes from Hugging Face ({}) and goes into Pistachio's own folder. What you say is \
             never sent anywhere, and you can remove it at any time. License: {}.",
            model.repo, model.license
        ),
    }
}

/// What a file of a model is for.
fn part(file: &ModelFile) -> Part {
    let has = |word| file.name.contains(word);
    let (icon, name, what): (Icon, _, _) = if has("encoder") {
        (IconName::Mic.into(), "Encoder", "Listens to what you say")
    } else if has("decoder") {
        (AppIcon::Brain.into(), "Decoder", "Finds the words")
    } else if has("joiner") {
        (AppIcon::Link.into(), "Joiner", "Combines the other two")
    } else {
        (
            AppIcon::Library.into(),
            "Vocabulary",
            "The pieces of words it writes with",
        )
    };
    Part {
        icon,
        name: name.into(),
        note: format!("{what} · {}", file.name),
        size: file.size,
        link: None,
    }
}

/// Makes `model` the one dictation uses and starts downloading it.
pub fn start_download(model: &'static Model, cx: &mut App) {
    AppSettings::update(cx, |s| model.id.clone_into(&mut s.speech_model));
    models(cx).update(cx, |models, cx| models.start_download(model, cx));
}

/// A model's name, languages, speed and size; highlighted when `selected`.
fn model_card(model: &'static Model, selected: bool, cx: &App) -> Div {
    let theme = cx.theme();
    let accent = palette(cx).accent;
    h_flex()
        .w_full()
        .gap_3()
        .px_3()
        .py_2()
        .rounded_lg()
        .border_1()
        .border_color(if selected { accent } else { theme.border })
        .when(selected, |this| this.bg(accent.opacity(0.08)))
        .child(
            // A radio dot.
            div()
                .flex_none()
                .size(px(14.))
                .rounded_full()
                .border_1()
                .border_color(if selected {
                    accent
                } else {
                    theme.muted_foreground
                })
                .when(selected, |this| {
                    this.child(div().m(px(3.)).size(px(6.)).rounded_full().bg(accent))
                }),
        )
        .child(model_summary(model, cx))
}

/// Name (and "Recommended"), then languages · speed · size, then the note.
fn model_summary(model: &'static Model, cx: &App) -> Div {
    let theme = cx.theme();
    let accent = palette(cx).accent;
    v_flex()
        .flex_1()
        .min_w_0()
        .gap_0p5()
        .child(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(model.name),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(model.maker),
                )
                .when(model.id == DEFAULT_MODEL, |this| {
                    this.child(
                        div()
                            .text_xs()
                            .px_1p5()
                            .rounded_md()
                            .bg(accent.opacity(0.15))
                            .text_color(accent)
                            .child("Recommended"),
                    )
                }),
        )
        .child(div().text_xs().text_color(theme.foreground).child(format!(
            "{} · {} · {}",
            model.languages,
            model.speed.label(),
            model.size_label()
        )))
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(format!("{} License: {}.", model.note, model.license)),
        )
}

/// The bar at the top of the chat while a model downloads; chatting goes on underneath.
pub fn download_banner(cx: &App) -> Option<AnyElement> {
    let models = models(cx);
    let models = models.read(cx);
    let download = models.download()?;
    let theme = cx.theme();
    let accent = palette(cx).accent;
    let fraction = download.fraction();
    Some(
        h_flex()
            .w_full()
            .gap_3()
            .px_4()
            .py_1p5()
            .border_b_1()
            .border_color(theme.border)
            .bg(palette(cx).surface)
            .child(
                Icon::new(IconName::Mic)
                    .small()
                    .text_color(theme.muted_foreground),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(theme.foreground)
                    .child(format!(
                        "Downloading {} for speech to text · {} of {}",
                        download.model.name,
                        catalog::size_label(download.done),
                        catalog::size_label(download.total),
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .h(px(4.))
                    .rounded_full()
                    .bg(theme.border)
                    .child(
                        div()
                            .h_full()
                            .rounded_full()
                            .bg(accent)
                            .w(relative(fraction)),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .w(px(36.))
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("{:.0}%", fraction * 100.)),
            )
            .child(
                Button::new("speech-download-banner-cancel")
                    .label("Cancel")
                    .ghost()
                    .xsmall()
                    .on_click(|_, _, cx| {
                        super::models::models(cx).update(cx, |models, _| models.cancel_download());
                    }),
            )
            .into_any_element(),
    )
}

/// The settings list of models: each with its state and what can be done with it.
pub fn settings_model_rows(cx: &App) -> AnyElement {
    let models = models(cx);
    let models = models.read(cx);
    let chosen = &cx.global::<AppSettings>().speech_model;
    let downloading = models.download().map(|download| download.model.id);
    let theme = cx.theme();
    let rows: Vec<AnyElement> = MODELS
        .iter()
        .map(|model| {
            let installed = models.is_installed(model.id);
            let in_use = installed && *chosen == model.id;
            let actions: AnyElement = if downloading == Some(model.id) {
                let percent = models.download().map_or(0., |d| d.fraction() * 100.);
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("Downloading… {percent:.0}%")),
                    )
                    .child(
                        Button::new(SharedString::from(format!("speech-cancel-{}", model.id)))
                            .label("Cancel")
                            .ghost()
                            .xsmall()
                            .on_click(|_, _, cx| {
                                super::models::models(cx)
                                    .update(cx, |models, _| models.cancel_download());
                            }),
                    )
                    .into_any_element()
            } else if installed {
                h_flex()
                    .gap_2()
                    .child(if in_use {
                        div()
                            .text_xs()
                            .text_color(palette(cx).accent)
                            .child("In use")
                            .into_any_element()
                    } else {
                        Button::new(SharedString::from(format!("speech-use-{}", model.id)))
                            .label("Use")
                            .outline()
                            .xsmall()
                            .on_click(move |_, _, cx| {
                                AppSettings::update(cx, |s| {
                                    model.id.clone_into(&mut s.speech_model);
                                });
                            })
                            .into_any_element()
                    })
                    .child(
                        Button::new(SharedString::from(format!("speech-remove-{}", model.id)))
                            .label("Remove")
                            .ghost()
                            .xsmall()
                            .on_click(move |_, _, cx| {
                                super::models::models(cx)
                                    .update(cx, |models, cx| models.remove(model.id, cx));
                            }),
                    )
                    .into_any_element()
            } else {
                download_button(
                    format!("speech-get-{}", model.id),
                    model.name,
                    model.size(),
                    cx,
                )
                .disabled(downloading.is_some())
                .on_click(move |_, window, cx| ask_to_download(model, window, cx))
                .into_any_element()
            };
            h_flex()
                .w_full()
                .gap_3()
                .py_2()
                .child(model_summary(model, cx))
                .child(div().flex_none().child(actions))
                .into_any_element()
        })
        .collect();
    v_flex().w_full().children(rows).into_any_element()
}

/// A notification when a model download ends.
pub fn notify_download(
    event: &super::models::SpeechModelsEvent,
    window: &mut Window,
    cx: &mut App,
) {
    use super::models::SpeechModelsEvent;
    use gpui_kit::component::notification::Notification;
    let name = |id: &str| catalog::model(id).map_or("The model", |model| model.name);
    let notification = match event {
        SpeechModelsEvent::Installed(id) => Notification::success(format!(
            "{} is ready. Press the microphone in the message box to dictate.",
            name(id)
        )),
        SpeechModelsEvent::Failed(id, error) => {
            Notification::error(format!("Downloading {} failed: {error}", name(id)))
        }
        SpeechModelsEvent::Cancelled => return,
    };
    window.push_notification(notification, cx);
}

#[cfg(test)]
mod tests {
    use super::{MODELS, details};

    #[test]
    fn the_details_are_all_that_is_downloaded() {
        for model in MODELS {
            details(model).assert_adds_up();
        }
    }
}
